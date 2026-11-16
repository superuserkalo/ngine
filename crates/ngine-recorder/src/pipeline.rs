use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::thread::JoinHandle;
use std::time::Duration;

use ngine_capture::{AudioBuffer, Event, Timestamp, VideoFrame};
use ngine_media::{EncoderInfo, Packet, StreamInfo, SurfaceKind, imports_dma_bufs};

use crate::audio::AudioStage;
use crate::clip::{self, ClipJob, EpochStreams};
use crate::error::RecorderError;
use crate::event::{Events, RecorderEvent};
use crate::levels::Peaks;
use crate::replay::{
    AfterSave, BackendChoice, BackendKind, ReplayBuffer, ReplayConfig, ReplayLength, SaveRange,
};
use crate::settings::{AudioSettings, ChosenEncoder, VideoSettings, bit_rate_cap, choose_encoder};
use crate::sink::Sink;
use crate::time::{
    Rebase, VIDEO_UNITS_PER_SECOND, audio_frames_from_nanos, duration_from_nanos,
    duration_from_video_units, micros_from_video_units, replay_packet, video_units_from_nanos,
    video_units_from_ticks,
};
use crate::video::{VideoStage, cannot_be_copied, frame_period, surface_kind};

const KEYFRAME_INTERVAL_WITH_REPLAY: i64 = VIDEO_UNITS_PER_SECOND;
const KEYFRAME_INTERVAL_WITHOUT_REPLAY: i64 = 2 * VIDEO_UNITS_PER_SECOND;
const STILL_PICTURE_LAG: i64 = VIDEO_UNITS_PER_SECOND / 10;
const LONG_GAP: i64 = 2 * VIDEO_UNITS_PER_SECOND;
const STEP_ACROSS_A_LONG_GAP: i64 = VIDEO_UNITS_PER_SECOND;
const LEAD_IN_PACKETS: usize = 16;
const EPOCHS_REMEMBERED: usize = 32;
const NANOS_PER_VIDEO_UNIT: i64 = 1_000_000_000 / VIDEO_UNITS_PER_SECOND;
const LONGEST_JUMP_NANOS: i64 = 60 * 1_000_000_000;
const MOST_GPU_ROUTE_FAILURES: u32 = 3;

#[derive(Clone, Debug)]
pub struct RecorderConfig {
    pub video: VideoSettings,
    pub audio: AudioSettings,
    /// The encoders that work on this machine, as
    /// [`ngine_media::working_video_encoders`] reports them.
    pub encoders: Vec<EncoderInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingStart {
    /// The file to write. Its folder is created when missing. When the
    /// picture changes size the recording continues in `name (part 2).mp4`
    /// next to it, and so on.
    pub file: PathBuf,
    /// With replay on, begin the file with up to this much of what the
    /// replay buffer holds.
    pub include_buffered: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaySetup {
    pub length: ReplayLength,
    pub backend: BackendChoice,
    pub disk_folder: Option<PathBuf>,
    pub clear_after_save: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaySaveRequest {
    pub range: SaveRange,
    pub folder: PathBuf,
    /// The clip's file name without its extension. A number is added when
    /// the name is taken.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingStatus {
    pub file: PathBuf,
    pub elapsed: Duration,
    pub markers: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayStatus {
    pub held: Duration,
    pub length: Duration,
    pub on_disk: bool,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub recording: Option<RecordingStatus>,
    pub replay: Option<ReplayStatus>,
    /// The encoder in use, once the first frame has arrived.
    pub encoder: Option<String>,
    /// Whether the encoder reads the captured GPU surface without a copy.
    pub zero_copy: bool,
    pub frames_captured: u64,
    pub frames_repeated: u64,
    /// Where the last captured frame lived when it arrived.
    pub frames_arrive_as: Option<SurfaceKind>,
    /// How loud each sound source was lately.
    pub levels: Peaks,
}

struct Stage {
    video: VideoStage,
    audio: AudioStage,
    streams: Arc<[StreamInfo]>,
    origin_nanos: i128,
    now_nanos: i64,
}

impl Stage {
    fn nanos_at(&self, timestamp: Timestamp) -> i64 {
        let nanos = i128::from(timestamp.as_nanos()) - self.origin_nanos;
        i64::try_from(nanos).unwrap_or(if nanos < 0 { i64::MIN } else { i64::MAX })
    }

    fn now_units(&self) -> i64 {
        video_units_from_nanos(self.now_nanos)
    }

    fn now_frames(&self) -> u64 {
        u64::try_from(audio_frames_from_nanos(self.now_nanos)).unwrap_or(0)
    }

    fn jumps_ahead(&self, timestamp: Timestamp) -> bool {
        self.nanos_at(timestamp).saturating_sub(self.now_nanos) > LONGEST_JUMP_NANOS
    }

    fn continue_at(&mut self, timestamp: Timestamp, period_nanos: i64) {
        let resumes_at = self.now_nanos + period_nanos;
        self.origin_nanos = i128::from(timestamp.as_nanos()) - i128::from(resumes_at);
    }
}

struct OpenFile {
    sink: Sink,
    rebase: Option<Rebase>,
    lead_in: VecDeque<Packet>,
}

struct Recording {
    base: PathBuf,
    part: u32,
    file: Option<OpenFile>,
    earlier_parts: i64,
    markers: u32,
}

impl Recording {
    fn part_path(&self) -> PathBuf {
        if self.part <= 1 {
            return self.base.clone();
        }
        let stem = self
            .base
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.base
            .with_file_name(format!("{stem} (part {}).mp4", self.part))
    }

    fn current_file(&self) -> PathBuf {
        self.file
            .as_ref()
            .map_or_else(|| self.part_path(), |file| file.sink.path().to_owned())
    }
}

struct Replay {
    buffer: ReplayBuffer,
    setup: ReplaySetup,
    epochs: VecDeque<EpochStreams>,
    reported_failure: bool,
}

impl Replay {
    fn remember(&mut self, streams: &Arc<[StreamInfo]>) {
        let epoch = self.buffer.current_epoch();
        self.epochs.retain(|(known, _)| *known != epoch);
        self.epochs.push_back((epoch, Arc::clone(streams)));
        while self.epochs.len() > EPOCHS_REMEMBERED {
            self.epochs.pop_front();
        }
    }
}

/// The recorder without its thread: capture events go in, encoded packets
/// go to the recording file and the replay buffer.
///
/// One thread owns it. [`Recorder`](crate::Recorder) runs it on a thread of
/// its own next to a capture session; tests feed it the events of a fake
/// session directly.
pub struct Pipeline {
    video: VideoSettings,
    audio: AudioSettings,
    encoder: ChosenEncoder,
    events: Events,
    stage: Option<Stage>,
    waits_for_new_origin: bool,
    recording: Option<Recording>,
    replay: Option<Replay>,
    saves_running: Arc<AtomicUsize>,
    files_being_completed: Vec<JoinHandle<()>>,
    frames_captured: u64,
    frames_repeated: u64,
    frames_arrive_as: Option<SurfaceKind>,
    gpu_route_failures: u32,
    wants_cpu_frames: bool,
}

impl Pipeline {
    pub fn new(
        config: &RecorderConfig,
        events: std::sync::mpsc::Sender<RecorderEvent>,
    ) -> Result<Self, RecorderError> {
        Ok(Self {
            encoder: choose_encoder(&config.video, &config.encoders)?,
            video: config.video.clone(),
            audio: config.audio,
            events: Events::new(events),
            stage: None,
            waits_for_new_origin: false,
            recording: None,
            replay: None,
            saves_running: Arc::new(AtomicUsize::new(0)),
            files_being_completed: Vec::new(),
            frames_captured: 0,
            frames_repeated: 0,
            frames_arrive_as: None,
            gpu_route_failures: 0,
            wants_cpu_frames: false,
        })
    }

    /// Whether capture should deliver DMA-BUFs on Linux, which only the
    /// chosen encoder's import can read.
    pub fn reads_dma_bufs(&self) -> bool {
        imports_dma_bufs(&self.encoder.name)
    }

    /// True once frames arrived that the encoder could not read on the GPU
    /// and that cannot be copied. Capture must be restarted without
    /// [`SessionOptions::gpu_surfaces`](ngine_capture::SessionOptions) for
    /// the picture to go on.
    pub fn wants_cpu_frames(&self) -> bool {
        self.wants_cpu_frames
    }

    fn gpu_route_allowed(&self) -> bool {
        self.gpu_route_failures < MOST_GPU_ROUTE_FAILURES
    }

    /// Changes volumes and which sources are heard while the tracks go on
    /// unbroken, so the replay buffer keeps what it holds.
    /// [`RecorderError::NeedsANewRecorder`] says the change would alter the
    /// number of tracks or their codec.
    pub fn set_audio(&mut self, audio: AudioSettings) -> Result<(), RecorderError> {
        if audio.track_count() != self.audio.track_count() || audio.codec != self.audio.codec {
            return Err(RecorderError::NeedsANewRecorder);
        }
        self.audio = audio;
        match self.stage.as_mut() {
            Some(stage) => stage.audio.retune(&audio),
            None => Ok(()),
        }
    }

    fn keyframe_interval(&self) -> i64 {
        if self.replay.is_some() {
            KEYFRAME_INTERVAL_WITH_REPLAY
        } else {
            KEYFRAME_INTERVAL_WITHOUT_REPLAY
        }
    }

    fn period(&self) -> i64 {
        frame_period(self.video.frame_rate)
    }

    /// Takes the next event of the capture session. An error means encoding
    /// failed: call [`Self::finish`] and stop.
    pub fn handle(&mut self, event: &Event) -> Result<(), RecorderError> {
        match event {
            Event::Video(frame) => self.take_frame(frame),
            Event::Audio(buffer) => self.take_sound(buffer),
            Event::Interrupted(error) => {
                self.events
                    .send(RecorderEvent::CaptureInterrupted(error.clone()));
                Ok(())
            }
            Event::Resumed => {
                self.events.send(RecorderEvent::CaptureResumed);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Tells the pipeline the time on the capture clock, so a still picture
    /// keeps producing frames while no event arrives.
    pub fn tick(&mut self, now: Timestamp) -> Result<(), RecorderError> {
        let period_nanos = self.period() * NANOS_PER_VIDEO_UNIT;
        let Some(stage) = self.stage.as_mut() else {
            return Ok(());
        };
        if self.waits_for_new_origin {
            return Ok(());
        }
        if stage.jumps_ahead(now) {
            stage.continue_at(now, period_nanos);
        }
        stage.now_nanos = stage.now_nanos.max(stage.nanos_at(now));
        self.catch_up()
    }

    /// Call it after the capture session was replaced by a new one. The
    /// timeline continues where it stood with the outage cut out.
    pub fn capture_restarted(&mut self) {
        self.waits_for_new_origin = self.stage.is_some();
    }

    fn begin_stage(&mut self, frame: &VideoFrame) -> Result<bool, RecorderError> {
        let opened = VideoStage::open(&self.video, &self.encoder, frame, self.gpu_route_allowed())?;
        if opened.gpu_route_failed {
            self.gpu_route_failures = MOST_GPU_ROUTE_FAILURES;
        }
        let video = opened.stage;
        if cannot_be_copied(video.surface()) && !video.reads_the_gpu_surface() {
            self.wants_cpu_frames = true;
            return Ok(false);
        }
        let audio = AudioStage::open(&self.audio)?;
        let streams: Arc<[StreamInfo]> = std::iter::once(video.stream_info())
            .chain(audio.stream_infos())
            .cloned()
            .collect();
        if let Some(replay) = self.replay.as_mut() {
            replay.buffer.begin_epoch();
            replay.remember(&streams);
        }
        self.stage = Some(Stage {
            video,
            audio,
            streams,
            origin_nanos: i128::from(frame.timestamp().as_nanos()),
            now_nanos: 0,
        });
        self.waits_for_new_origin = false;
        Ok(true)
    }

    fn leave_the_gpu_route(&mut self, frame: &VideoFrame) -> Result<(), RecorderError> {
        self.gpu_route_failures += 1;
        let _the_broken_encoder_may_not_flush = self.end_stage(true);
        self.take_frame(frame)
    }

    fn end_stage(&mut self, recording_continues: bool) -> Result<(), RecorderError> {
        let Some(mut stage) = self.stage.take() else {
            return Ok(());
        };
        let flushed = stage.video.finish().and_then(|mut packets| {
            packets.extend(stage.audio.finish(stage.now_frames())?);
            Ok(packets)
        });
        let now_units = stage.now_units();
        self.stage = Some(stage);
        let delivered = flushed.map(|packets| self.deliver(packets));
        self.stage = None;
        if let Some(recording) = self.recording.as_mut()
            && let Some(file) = recording.file.take()
        {
            let length = file.rebase.map_or(0, |rebase| now_units - rebase.start());
            recording.earlier_parts += length.max(0);
            recording.part += 1;
            let completing = file.sink.finish(recording_continues);
            self.complete_in_the_background(completing);
        }
        delivered
    }

    fn take_frame(&mut self, frame: &VideoFrame) -> Result<(), RecorderError> {
        let surface = surface_kind(frame);
        self.frames_arrive_as = Some(surface);
        if cannot_be_copied(surface) && !self.gpu_route_allowed() {
            self.wants_cpu_frames = true;
            return Ok(());
        }
        if self
            .stage
            .as_ref()
            .is_some_and(|stage| !stage.video.takes(frame))
        {
            self.end_stage(true)?;
        }
        if self.stage.is_none() && !self.begin_stage(frame)? {
            return Ok(());
        }
        let period = self.period();
        let interval = self.keyframe_interval();
        let Some(stage) = self.stage.as_mut() else {
            return Ok(());
        };
        if self.waits_for_new_origin || stage.jumps_ahead(frame.timestamp()) {
            stage.continue_at(frame.timestamp(), period * NANOS_PER_VIDEO_UNIT);
            self.waits_for_new_origin = false;
        }
        let nanos = stage.nanos_at(frame.timestamp()).max(0);
        let captured_at = video_units_from_nanos(nanos);
        self.repeat_until(captured_at - period / 2)?;
        let Some(stage) = self.stage.as_mut() else {
            return Ok(());
        };
        let pts = stage.video.last_pts().map_or(captured_at, |last| {
            captured_at.max(last + (period / 4).max(1))
        });
        let packets = match stage.video.encode(frame, pts, interval) {
            Ok(packets) => packets,
            Err(_) if stage.video.reads_the_gpu_surface() => {
                return self.leave_the_gpu_route(frame);
            }
            Err(error) => return Err(error),
        };
        stage.now_nanos = stage.now_nanos.max(nanos);
        self.frames_captured += 1;
        self.deliver(packets);
        self.catch_up()
    }

    fn take_sound(&mut self, buffer: &AudioBuffer) -> Result<(), RecorderError> {
        let Some(stage) = self.stage.as_mut() else {
            return Ok(());
        };
        if self.waits_for_new_origin {
            return Ok(());
        }
        if stage.jumps_ahead(buffer.timestamp()) {
            return Ok(());
        }
        let start = stage.nanos_at(buffer.timestamp());
        stage.audio.push(buffer, start)?;
        let length = i64::try_from(buffer.duration().as_nanos()).unwrap_or(0);
        stage.now_nanos = stage.now_nanos.max(start.saturating_add(length));
        self.catch_up()
    }

    fn repeat_until(&mut self, limit: i64) -> Result<(), RecorderError> {
        let period = self.period();
        let interval = self.keyframe_interval();
        loop {
            let Some(stage) = self.stage.as_mut() else {
                return Ok(());
            };
            let Some(last) = stage.video.last_pts() else {
                return Ok(());
            };
            let step = if limit - last > LONG_GAP {
                STEP_ACROSS_A_LONG_GAP
            } else {
                period
            };
            if last + step > limit {
                return Ok(());
            }
            let packets = stage.video.repeat(last + step, interval)?;
            self.frames_repeated += 1;
            self.deliver(packets);
        }
    }

    fn catch_up(&mut self) -> Result<(), RecorderError> {
        let Some(stage) = self.stage.as_ref() else {
            return Ok(());
        };
        self.repeat_until(stage.now_units() - STILL_PICTURE_LAG)?;
        let Some(stage) = self.stage.as_mut() else {
            return Ok(());
        };
        let packets = stage.audio.advance(stage.now_frames())?;
        self.deliver(packets);
        Ok(())
    }

    fn deliver(&mut self, packets: Vec<Packet>) {
        for packet in packets {
            self.store_for_replay(&packet);
            self.write_to_file(packet);
        }
    }

    fn store_for_replay(&mut self, packet: &Packet) {
        let Some(replay) = self.replay.as_mut() else {
            return;
        };
        let Some(stored) = replay_packet(packet) else {
            return;
        };
        let failure = match replay.buffer.push(stored) {
            Ok(pushed) => pushed.write_failure,
            Err(error) => Some(error),
        };
        match failure {
            Some(error) if !replay.reported_failure => {
                replay.reported_failure = true;
                self.events.send(RecorderEvent::ReplayStorageFailing {
                    error: error.into(),
                });
            }
            Some(_) => {}
            None => replay.reported_failure = false,
        }
    }

    fn open_file(&mut self) -> bool {
        let (Some(recording), Some(stage)) = (self.recording.as_mut(), self.stage.as_ref()) else {
            return false;
        };
        if recording.file.is_some() {
            return true;
        }
        let path = recording.part_path();
        match Sink::create(&path, &stage.streams, self.events.clone()) {
            Ok(sink) => {
                recording.file = Some(OpenFile {
                    sink,
                    rebase: None,
                    lead_in: VecDeque::new(),
                });
                true
            }
            Err(error) => {
                self.recording = None;
                self.events.send(RecorderEvent::RecordingFailed {
                    file: path,
                    length: Duration::ZERO,
                    error,
                });
                false
            }
        }
    }

    fn write_to_file(&mut self, packet: Packet) {
        if !self.open_file() {
            return;
        }
        let Some(file) = self
            .recording
            .as_mut()
            .and_then(|recording| recording.file.as_mut())
        else {
            return;
        };
        if file.sink.has_failed() {
            self.recording = None;
            return;
        }
        match file.rebase {
            Some(rebase) => {
                if let Some(moved) = rebase.apply(packet) {
                    file.sink.packet(moved);
                }
            }
            None if packet.stream != 0 => {
                file.lead_in.push_back(packet);
                if file.lead_in.len() > LEAD_IN_PACKETS {
                    file.lead_in.pop_front();
                }
            }
            None if packet.keyframe => {
                let Some(start) = packet.pts else {
                    return;
                };
                let rebase = Rebase::starting_at(start);
                file.rebase = Some(rebase);
                let lead_in = std::mem::take(&mut file.lead_in);
                for moved in lead_in
                    .into_iter()
                    .chain(std::iter::once(packet))
                    .filter_map(|waiting| rebase.apply(waiting))
                {
                    file.sink.packet(moved);
                }
            }
            None => {}
        }
    }

    fn begin_with_buffered(&mut self, length: Duration) {
        let (Some(replay), Some(recording)) = (self.replay.as_mut(), self.recording.as_mut())
        else {
            return;
        };
        let Some(file) = recording.file.as_mut() else {
            return;
        };
        let save = replay
            .buffer
            .save(SaveRange::Last(length), AfterSave::KeepBuffer);
        let newest = save
            .slices()
            .last()
            .filter(|slice| slice.epoch() == replay.buffer.current_epoch());
        if let Some(slice) = newest {
            let rebase = Rebase::starting_at(video_units_from_ticks(slice.video_start()));
            file.rebase = Some(rebase);
            file.sink.slice(slice.clone(), rebase);
        }
    }

    pub fn start_recording(&mut self, start: &RecordingStart) -> Result<(), RecorderError> {
        if self.recording.is_some() {
            return Err(RecorderError::AlreadyRecording);
        }
        if let Some(folder) = start
            .file
            .parent()
            .filter(|folder| folder != &Path::new(""))
        {
            fs::create_dir_all(folder)
                .map_err(|error| RecorderError::file("create the folder", folder, error))?;
        }
        self.recording = Some(Recording {
            base: start.file.clone(),
            part: 1,
            file: None,
            earlier_parts: 0,
            markers: 0,
        });
        if self.stage.is_some() {
            let Some(stage) = self.stage.as_ref() else {
                return Ok(());
            };
            let sink = Sink::create(&start.file, &stage.streams, self.events.clone());
            match (sink, self.recording.as_mut()) {
                (Ok(sink), Some(recording)) => {
                    recording.file = Some(OpenFile {
                        sink,
                        rebase: None,
                        lead_in: VecDeque::new(),
                    });
                }
                (Err(error), _) => {
                    self.recording = None;
                    return Err(error);
                }
                (Ok(_), None) => {}
            }
            if let Some(length) = start.include_buffered {
                self.begin_with_buffered(length);
            }
            if let Some(stage) = self.stage.as_mut() {
                stage.video.request_keyframe();
            }
        }
        self.events.send(RecorderEvent::RecordingStarted {
            file: start.file.clone(),
        });
        Ok(())
    }

    /// The file's end is reported as [`RecorderEvent::RecordingFinished`]
    /// once it is complete on disk.
    pub fn stop_recording(&mut self) -> Result<(), RecorderError> {
        let recording = self.recording.take().ok_or(RecorderError::NotRecording)?;
        match recording.file {
            Some(file) => {
                let completing = file.sink.finish(false);
                self.complete_in_the_background(completing);
            }
            None => self.events.send(RecorderEvent::RecordingDiscarded {
                file: recording.base,
            }),
        }
        Ok(())
    }

    /// Stores a marker at the current time and returns its position in the
    /// recording. [`RecorderError::Busy`] says the recording's first frame
    /// has not arrived yet.
    pub fn add_marker(&mut self) -> Result<Duration, RecorderError> {
        let recording = self.recording.as_mut().ok_or(RecorderError::NotRecording)?;
        let started = recording
            .file
            .as_ref()
            .and_then(|file| file.rebase.map(|rebase| (file, rebase)));
        let (Some((file, rebase)), Some(stage)) = (started, self.stage.as_ref()) else {
            return Err(RecorderError::Busy);
        };
        let in_file = (stage.now_units() - rebase.start()).max(0);
        file.sink
            .marker(u64::try_from(micros_from_video_units(in_file)).unwrap_or(0));
        recording.markers += 1;
        let at = duration_from_video_units(recording.earlier_parts + in_file);
        self.events.send(RecorderEvent::MarkerAdded { at });
        Ok(at)
    }

    /// Turns replay on with `Some`, replacing a buffer that was set up
    /// differently, and off with `None`.
    pub fn set_replay(&mut self, setup: Option<ReplaySetup>) -> Result<(), RecorderError> {
        let Some(setup) = setup else {
            if self.replay.take().is_some() {
                self.events.send(RecorderEvent::ReplayStopped);
            }
            return Ok(());
        };
        if self
            .replay
            .as_ref()
            .is_some_and(|replay| replay.setup == setup)
        {
            return Ok(());
        }
        let buffer = ReplayBuffer::new(&ReplayConfig {
            length: setup.length,
            max_bitrate_bits_per_second: bit_rate_cap(&self.video, &self.audio),
            backend: setup.backend,
            disk_folder: setup.disk_folder.clone(),
        })?;
        let was_on = self.replay.is_some();
        let mut replay = Replay {
            buffer,
            setup,
            epochs: VecDeque::new(),
            reported_failure: false,
        };
        if let Some(stage) = self.stage.as_mut() {
            replay.remember(&stage.streams);
            stage.video.request_keyframe();
        }
        self.replay = Some(replay);
        if !was_on {
            self.events.send(RecorderEvent::ReplayStarted);
        }
        Ok(())
    }

    /// Snapshots the asked range and writes it on another thread. Each clip
    /// arrives as [`RecorderEvent::ReplaySaved`] with its real length.
    pub fn save_replay(&mut self, request: ReplaySaveRequest) -> Result<(), RecorderError> {
        if self.replay.is_none() {
            return Err(RecorderError::ReplayOff);
        }
        if let Some(stage) = self.stage.as_mut() {
            let packets = stage.audio.advance_for_a_save(stage.now_frames())?;
            self.deliver(packets);
        }
        let replay = self.replay.as_mut().ok_or(RecorderError::ReplayOff)?;
        if replay.buffer.covered_duration() <= crate::replay::Ticks::ZERO {
            return Err(RecorderError::NothingHeld);
        }
        let running = self
            .saves_running
            .load(std::sync::atomic::Ordering::Acquire);
        if running >= clip::MOST_SAVES_AT_ONCE {
            return Err(RecorderError::Busy);
        }
        let after = if replay.setup.clear_after_save {
            AfterSave::ClearBuffer
        } else {
            AfterSave::KeepBuffer
        };
        let save = replay.buffer.save(request.range, after);
        if save.is_empty() {
            return Err(RecorderError::NothingHeld);
        }
        let job = ClipJob {
            save,
            epochs: replay.epochs.iter().cloned().collect(),
            folder: request.folder,
            name: request.name,
        };
        clip::spawn(job, &self.saves_running, self.events.clone())
    }

    /// Returns once the recording file holds everything encoded so far, as
    /// far as the muxer has written it out.
    pub fn wait_until_written(&self) {
        if let Some(file) = self
            .recording
            .as_ref()
            .and_then(|recording| recording.file.as_ref())
        {
            file.sink.wait_until_written();
        }
    }

    pub fn status(&self) -> Status {
        let now_units = self.stage.as_ref().map_or(0, Stage::now_units);
        Status {
            recording: self.recording.as_ref().map(|recording| {
                let in_file = recording
                    .file
                    .as_ref()
                    .and_then(|file| file.rebase)
                    .map_or(0, |rebase| (now_units - rebase.start()).max(0));
                RecordingStatus {
                    file: recording.current_file(),
                    elapsed: duration_from_video_units(recording.earlier_parts + in_file),
                    markers: recording.markers,
                }
            }),
            replay: self.replay.as_ref().map(|replay| ReplayStatus {
                held: replay
                    .buffer
                    .covered_duration()
                    .to_duration()
                    .min(replay.setup.length.get()),
                length: replay.setup.length.get(),
                on_disk: replay.buffer.backend_kind() == BackendKind::Disk,
                bytes: replay.buffer.size_bytes(),
            }),
            encoder: self
                .stage
                .as_ref()
                .map(|stage| stage.video.encoder_name().to_owned()),
            zero_copy: self
                .stage
                .as_ref()
                .is_some_and(|stage| stage.video.reads_the_gpu_surface()),
            frames_captured: self.frames_captured,
            frames_repeated: self.frames_repeated,
            frames_arrive_as: self.frames_arrive_as,
            levels: Peaks::default(),
        }
    }

    /// How long the current picture size has been encoded.
    pub fn stage_time(&self) -> Duration {
        self.stage
            .as_ref()
            .map_or(Duration::ZERO, |stage| duration_from_nanos(stage.now_nanos))
    }

    /// Empties the encoders and completes the recording file. Call it when
    /// the capture session has ended, also after an error.
    pub fn finish(&mut self) {
        let flushed = self.end_stage(false);
        if let Some(recording) = self.recording.take() {
            match recording.file {
                Some(file) => self.files_being_completed.push(file.sink.finish(false)),
                None if recording.part <= 1 => {
                    self.events.send(RecorderEvent::RecordingDiscarded {
                        file: recording.base,
                    });
                }
                None => {}
            }
        }
        if self.replay.take().is_some() {
            self.events.send(RecorderEvent::ReplayStopped);
        }
        if let Err(error) = flushed {
            self.events.send(RecorderEvent::Failed(error));
        }
        for completing in self.files_being_completed.drain(..) {
            let _writer_panicked = completing.join();
        }
    }

    fn complete_in_the_background(&mut self, completing: JoinHandle<()>) {
        self.files_being_completed
            .retain(|earlier| !earlier.is_finished());
        self.files_being_completed.push(completing);
    }
}
