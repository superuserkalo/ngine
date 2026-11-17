use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use ngine_capture::fake::FakeCapture;
use ngine_capture::{
    AudioBuffer, AudioSource, CpuFrame, EndReason, Event, Screen, ScreenId, Session,
    SessionOptions, Size, Target, Timestamp, VideoFrame,
};
use ngine_media::{
    AudioCodec, AudioDecoder, AudioFrame, Demuxer, EncoderInfo, PixelFormat, Rational,
    SOFTWARE_H264, StreamInfo, VideoCodec, VideoDecoder, VideoDecoderOptions,
    VideoFrame as Decoded, has_encoder, working_video_encoders,
};
use ngine_recorder::{
    AudioSettings, Pipeline, Quality, RecorderConfig, RecorderEvent, Tracks, VideoSettings,
};

pub const SIZE: Size = Size::new(320, 240);
pub const FRAME_RATE: u32 = 30;
pub const CLOCK_START_NANOS: u64 = 1_000_000_000_000;
pub const SAMPLE_RATE: usize = 48_000;
pub const SYSTEM_TONE: f64 = 440.0;
pub const MICROPHONE_TONE: f64 = 1_000.0;
pub const LEVEL: f64 = 0.4;
const EVENT_WAIT: Duration = Duration::from_secs(20);

pub fn float(count: usize) -> f64 {
    f64::from(u32::try_from(count).unwrap())
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a test sample lies between -2 and 2, which an f32 holds, so only precision beyond what the API takes is lost"
)]
pub fn narrow(value: f64) -> f32 {
    value as f32
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test times are small positive numbers of seconds"
)]
pub fn nanos(seconds: f64) -> u64 {
    (seconds * 1e9).round() as u64
}

pub fn at(seconds: f64) -> Timestamp {
    Timestamp::from_nanos(CLOCK_START_NANOS + nanos(seconds))
}

pub struct TempFolder(PathBuf);

impl TempFolder {
    pub fn new(name: &str) -> Self {
        let folder =
            std::env::temp_dir().join(format!("ngine-recorder-{}-{name}", std::process::id()));
        let _missing_is_fine = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        Self(folder)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _missing_is_fine = std::fs::remove_dir_all(&self.0);
    }
}

fn encoder() -> EncoderInfo {
    if has_encoder(SOFTWARE_H264) {
        return EncoderInfo {
            name: SOFTWARE_H264.to_owned(),
            codec: VideoCodec::H264,
            hardware: false,
        };
    }
    working_video_encoders()
        .into_iter()
        .find(|encoder| encoder.codec == VideoCodec::H264)
        .expect("the tests need libopenh264 or a working hardware H.264 encoder")
}

pub fn config(frame_rate: u32, tracks: Tracks) -> RecorderConfig {
    let encoder = encoder();
    RecorderConfig {
        video: VideoSettings {
            codec: VideoCodec::H264,
            quality: Quality::Low,
            frame_rate,
            encoder: Some(encoder.name.clone()),
        },
        audio: AudioSettings {
            system_gain: Some(1.0),
            microphone_gain: Some(1.0),
            tracks,
            codec: AudioCodec::Aac,
        },
        encoders: vec![encoder],
    }
}

pub fn gray(size: Size, level: u8) -> Arc<CpuFrame> {
    Arc::new(CpuFrame::solid_bgra(size, [level, level, level, 255]).unwrap())
}

pub fn frames(
    frame_rate: u32,
    from: f64,
    until: f64,
    picture: impl Fn(f64) -> Arc<CpuFrame> + Send + 'static,
) -> impl Iterator<Item = Event> + Send {
    let period = 1.0 / f64::from(frame_rate);
    (0_u32..)
        .map(move |index| from + f64::from(index) * period)
        .take_while(move |seconds| *seconds < until - 1e-9)
        .map(move |seconds| Event::Video(VideoFrame::from_cpu(at(seconds), picture(seconds))))
}

#[derive(Clone, Copy)]
pub struct Sound {
    pub source: AudioSource,
    pub nominal_rate: u32,
    pub real_rate: f64,
    pub channels: u16,
}

impl Sound {
    pub fn system() -> Self {
        Self {
            source: AudioSource::System,
            nominal_rate: 48_000,
            real_rate: 48_000.0,
            channels: 2,
        }
    }

    pub fn microphone() -> Self {
        Self {
            source: AudioSource::Microphone,
            nominal_rate: 44_100,
            real_rate: 44_100.0,
            channels: 1,
        }
    }
}

pub fn sound(
    shape: Sound,
    from: f64,
    until: f64,
    signal: impl Fn(f64) -> f64 + Send + 'static,
) -> impl Iterator<Item = Event> + Send {
    let frames_per_buffer = shape.nominal_rate / 100;
    (0_u64..)
        .map(move |buffer| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "frame counts in a test stay far below 2^52"
            )]
            let first_frame = (buffer * u64::from(frames_per_buffer)) as f64;
            (first_frame, from + first_frame / shape.real_rate)
        })
        .take_while(move |(_, starts)| *starts < until - 1e-9)
        .map(move |(first_frame, starts)| {
            let samples = (0..frames_per_buffer)
                .flat_map(|frame| {
                    let time = from + (first_frame + f64::from(frame)) / shape.real_rate;
                    std::iter::repeat_n(narrow(signal(time)), usize::from(shape.channels))
                })
                .collect();
            let buffer = AudioBuffer::new(
                shape.source,
                at(starts),
                shape.nominal_rate,
                shape.channels,
                samples,
            )
            .unwrap();
            Event::Audio(buffer)
        })
}

pub fn tone(frequency: f64) -> impl Fn(f64) -> f64 + Send + 'static {
    move |time| LEVEL * (std::f64::consts::TAU * frequency * time).sin()
}

pub fn merged(scripts: Vec<Box<dyn Iterator<Item = Event> + Send>>) -> Vec<Event> {
    let mut events: Vec<Event> = scripts.into_iter().flatten().collect();
    events.sort_by_key(Event::timestamp);
    events
}

pub struct Rig {
    pub pipeline: Pipeline,
    pub events: Receiver<RecorderEvent>,
    session: Session,
    held_back: Option<Event>,
    pub seen: Vec<RecorderEvent>,
}

impl Rig {
    pub fn new<S>(config: &RecorderConfig, script: S) -> Self
    where
        S: IntoIterator<Item = Event>,
        S::IntoIter: Send + 'static,
    {
        let screen = Screen::new(ScreenId(1), "Fake screen", SIZE);
        Self::capturing(&Target::from(screen), config, script)
    }

    pub fn capturing<S>(target: &Target, config: &RecorderConfig, script: S) -> Self
    where
        S: IntoIterator<Item = Event>,
        S::IntoIter: Send + 'static,
    {
        let (sender, events) = mpsc::channel();
        let session = FakeCapture::new()
            .with_script(script)
            .into_capturer()
            .start(target, &SessionOptions::new())
            .unwrap();
        Self {
            pipeline: Pipeline::new(config, sender).unwrap(),
            events,
            session,
            held_back: None,
            seen: Vec::new(),
        }
    }

    pub fn run_until(&mut self, seconds: f64) -> bool {
        let limit = at(seconds);
        loop {
            let Some(event) = self.held_back.take().or_else(|| self.session.recv()) else {
                return false;
            };
            if event.timestamp().is_some_and(|stamp| stamp >= limit) {
                self.held_back = Some(event);
                return true;
            }
            if matches!(event, Event::Ended(EndReason::Stopped)) {
                return false;
            }
            self.pipeline.handle(&event).unwrap();
        }
    }

    pub fn run_to_the_end(&mut self) {
        while self.run_until(f64::from(u32::MAX)) {}
        self.pipeline.finish();
    }

    pub fn wait_for<T>(&mut self, mut wanted: impl FnMut(&RecorderEvent) -> Option<T>) -> T {
        loop {
            let event = self
                .events
                .recv_timeout(EVENT_WAIT)
                .expect("the awaited recorder event arrives");
            let found = wanted(&event);
            self.seen.push(event);
            if let Some(found) = found {
                return found;
            }
        }
    }

    pub fn wait_for_finished_file(&mut self) -> (PathBuf, Duration) {
        self.wait_for(|event| match event {
            RecorderEvent::RecordingFinished { file, length, .. } => Some((file.clone(), *length)),
            RecorderEvent::RecordingFailed { error, .. } => panic!("the recording failed: {error}"),
            _ => None,
        })
    }

    pub fn wait_for_saved_clip(&mut self) -> (PathBuf, Duration) {
        self.wait_for(|event| match event {
            RecorderEvent::ReplaySaved { file, length } => Some((file.clone(), *length)),
            RecorderEvent::ReplaySaveFailed { error } => panic!("the save failed: {error}"),
            _ => None,
        })
    }
}

pub struct File {
    pub streams: Vec<StreamInfo>,
    pub video: Vec<Decoded>,
    pub audio: Vec<Vec<AudioFrame>>,
    pub duration_micros: Option<i64>,
}

impl File {
    pub fn decode(path: &Path) -> Self {
        let mut demuxer = Demuxer::open(path).unwrap();
        let streams = demuxer.streams().to_vec();
        let options = VideoDecoderOptions {
            output: PixelFormat::Nv12,
            hardware: false,
        };
        let mut video_decoder = VideoDecoder::open(&streams[0], options).unwrap();
        let mut audio_decoders: Vec<AudioDecoder> = streams[1..]
            .iter()
            .map(|stream| AudioDecoder::open(stream).unwrap())
            .collect();
        let mut video = Vec::new();
        let mut audio = vec![Vec::new(); audio_decoders.len()];
        while let Some(packet) = demuxer.read().unwrap() {
            if packet.stream == 0 {
                video.extend(video_decoder.decode(&packet).unwrap());
            } else {
                let track = packet.stream - 1;
                audio[track].extend(audio_decoders[track].decode(&packet).unwrap());
            }
        }
        video.extend(video_decoder.finish().unwrap());
        for (track, decoder) in audio_decoders.iter_mut().enumerate() {
            audio[track].extend(decoder.finish().unwrap());
        }
        Self {
            duration_micros: demuxer.duration_micros(),
            streams,
            video,
            audio,
        }
    }

    pub fn video_micros(&self) -> Vec<i64> {
        let time_base = self.streams[0].time_base;
        self.video
            .iter()
            .map(|frame| {
                time_base
                    .rescale(frame.pts.unwrap(), Rational::MICROS)
                    .unwrap()
            })
            .collect()
    }

    pub fn audio_start_micros(&self, track: usize) -> i64 {
        let time_base = self.streams[track + 1].time_base;
        let first = self.audio[track].first().unwrap().pts.unwrap();
        time_base.rescale(first, Rational::MICROS).unwrap()
    }

    pub fn left_channel(&self, track: usize) -> Vec<f32> {
        self.audio[track]
            .iter()
            .flat_map(|frame| frame.samples.iter().step_by(usize::from(frame.channels)))
            .copied()
            .collect()
    }

    pub fn sound_onset(&self, track: usize, from_seconds: f64, level: f32) -> Option<f64> {
        let start = self.audio_start_micros(track);
        #[expect(
            clippy::cast_precision_loss,
            reason = "timestamps in a test stay far below 2^52"
        )]
        let start_seconds = start as f64 / 1e6;
        self.left_channel(track)
            .iter()
            .enumerate()
            .map(|(index, sample)| (start_seconds + float(index) / float(SAMPLE_RATE), sample))
            .find(|(time, sample)| *time >= from_seconds && sample.abs() > level)
            .map(|(time, _)| time)
    }

    pub fn bright_onset(&self, from_seconds: f64, level: f64) -> Option<f64> {
        self.video_micros()
            .iter()
            .zip(&self.video)
            .map(|(micros, frame)| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "timestamps in a test stay far below 2^52"
                )]
                let seconds = *micros as f64 / 1e6;
                (seconds, mean_luma(frame))
            })
            .find(|(seconds, luma)| *seconds >= from_seconds && *luma > level)
            .map(|(seconds, _)| seconds)
    }
}

pub fn mean_luma(frame: &Decoded) -> f64 {
    let plane = &frame.planes[0];
    let sum: f64 = plane.data.iter().map(|value| f64::from(*value)).sum();
    sum / float(plane.data.len())
}

pub fn tone_level(samples: &[f32], frequency: f64) -> f64 {
    let (mut sine, mut cosine) = (0.0_f64, 0.0_f64);
    for (index, sample) in samples.iter().enumerate() {
        let angle = std::f64::consts::TAU * frequency * float(index) / float(SAMPLE_RATE);
        sine += f64::from(*sample) * angle.sin();
        cosine += f64::from(*sample) * angle.cos();
    }
    2.0 * sine.hypot(cosine) / float(samples.len())
}
