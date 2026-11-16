use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, SystemTime};

use ngine_capture::{AudioApp, CaptureError, Capturer, PickedTarget, PickerOptions, RestoreToken};
use ngine_recorder::replay::{
    BackendChoice, ReplayLength, SaveRange, installed_memory_bytes, memory_budget_bytes,
};
use ngine_recorder::{
    AudioCodec, AudioSettings, CaptureSetup, EncoderInfo, Peaks, Recorder, RecorderConfig,
    RecorderError, RecorderEvent, RecordingStart, ReplaySaveRequest, ReplaySetup, VideoCodec,
    VideoSettings, bit_rate_cap, free_space_bytes, unused_path, working_video_codecs,
    working_video_encoders,
};

use crate::format;

use super::{
    Audio, AudioSource, Capabilities, Codec, Device, Engine, Event, Inputs, Levels, Permission,
    Problem, Quality, Recording, RecordingRequest, Refusal, Replay, ReplayRequest, ReplaySpan,
    ReplayStorage, Status, SystemSound, Target, TargetKind, Tracks, Video,
};

const BYTES_PER_MB: u64 = 1_000_000;
const RECORDING_NAME: &str = "Recording";
const REPLAY_NAME: &str = "Replay";
const SMALLEST_LISTED_WINDOW: u32 = 64;

type Picked = Result<Option<PickedTarget>, CaptureError>;

#[derive(Clone, PartialEq)]
struct SessionKey {
    target: String,
    video: Video,
    audio: Audio,
    inputs: Inputs,
}

struct Session {
    recorder: Recorder,
    key: SessionKey,
    target: Target,
    recording: bool,
    replay: Option<ReplayRequest>,
    ended: bool,
}

pub struct RealEngine {
    capturer: Arc<Capturer>,
    encoders: Vec<EncoderInfo>,
    known: HashMap<String, ngine_capture::Target>,
    session: Option<Session>,
    events: Vec<Event>,
    pick_sender: Sender<Picked>,
    picks: Receiver<Picked>,
    targets_picked: u32,
    earlier_parts: Duration,
    save_folder: Option<PathBuf>,
    restore_token_file: Option<PathBuf>,
}

fn read_restore_token(file: &Path) -> Option<RestoreToken> {
    let token = std::fs::read_to_string(file).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| RestoreToken::new(token))
}

fn picker_options(restore_token_file: Option<&Path>) -> PickerOptions {
    match restore_token_file.and_then(read_restore_token) {
        Some(token) => PickerOptions::new().with_restore_token(token),
        None => PickerOptions::new(),
    }
}

fn keep_restore_token(file: &Path, token: &RestoreToken) {
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(file, token.as_str()));
    if let Err(error) = written {
        eprintln!("ngine: the system picker's choice could not be kept: {error}");
    }
}

fn stamped(kind: &str) -> String {
    format::stamped(kind, &format::local(SystemTime::now()))
}

fn video_codec(codec: Codec) -> VideoCodec {
    match codec {
        Codec::H264 => VideoCodec::H264,
        Codec::Hevc => VideoCodec::Hevc,
        Codec::Av1 => VideoCodec::Av1,
    }
}

fn video_settings(video: Video) -> VideoSettings {
    VideoSettings {
        codec: video_codec(video.codec),
        quality: match video.quality {
            Quality::Low => ngine_recorder::Quality::Low,
            Quality::Medium => ngine_recorder::Quality::Medium,
            Quality::High => ngine_recorder::Quality::High,
            Quality::Highest => ngine_recorder::Quality::Highest,
        },
        frame_rate: u32::from(video.frame_rate),
        encoder: None,
    }
}

fn gain(source: AudioSource) -> Option<f32> {
    source
        .enabled
        .then(|| f32::from(source.volume_percent) / 100.0)
}

fn audio_settings(audio: Audio) -> AudioSettings {
    AudioSettings {
        system_gain: gain(audio.system),
        microphone_gain: gain(audio.microphone),
        tracks: match audio.tracks {
            Tracks::Three => ngine_recorder::Tracks::Three,
            Tracks::MixOnly => ngine_recorder::Tracks::MixOnly,
        },
        codec: AudioCodec::Aac,
    }
}

fn replay_setup(request: &ReplayRequest) -> ReplaySetup {
    let length = request
        .length
        .clamp(ReplayLength::SHORTEST.get(), ReplayLength::LONGEST.get());
    ReplaySetup {
        length: ReplayLength::new(length).unwrap_or_default(),
        backend: match request.storage {
            ReplayStorage::Memory => BackendChoice::Memory,
            ReplayStorage::Disk => BackendChoice::Disk,
            ReplayStorage::Auto => BackendChoice::Auto {
                memory_budget_bytes: u64::from(request.memory_budget_mb) * BYTES_PER_MB,
            },
        },
        disk_folder: Some(request.buffer_folder.clone()),
        clear_after_save: request.clear_after_save,
    }
}

fn problem_of(error: &CaptureError) -> Problem {
    match error {
        CaptureError::PermissionDenied(ngine_capture::Permission::Microphone) => {
            Problem::PermissionMissing(Permission::Microphone)
        }
        CaptureError::PermissionDenied(_) => {
            Problem::PermissionMissing(Permission::ScreenRecording)
        }
        _ => Problem::CaptureLost,
    }
}

fn write_problem(error: &RecorderError, folder: Option<PathBuf>) -> Problem {
    match folder {
        Some(folder) if error.is_disk_full() => Problem::DiskFull { folder },
        _ => Problem::WriteFailed,
    }
}

fn level(peak: u16, source: AudioSource) -> u16 {
    if !source.enabled {
        return 0;
    }
    let scaled = u32::from(peak) * u32::from(source.volume_percent) / 100;
    u16::try_from(scaled)
        .unwrap_or(Levels::FULL_SCALE)
        .min(Levels::FULL_SCALE)
}

fn levels(peaks: Peaks, audio: Audio) -> Levels {
    Levels {
        system: level(peaks.system, audio.system),
        microphone: level(peaks.microphone, audio.microphone),
    }
}

fn sound_of(inputs: &Inputs) -> Option<AudioApp> {
    match &inputs.system {
        SystemSound::App { id, name } => Some(AudioApp::new(*id, name.clone())),
        SystemSound::Everything | SystemSound::Output(_) => None,
    }
}

fn app_sounds(apps: impl IntoIterator<Item = (u64, String)>) -> Vec<SystemSound> {
    let mut sounds: Vec<SystemSound> = Vec::new();
    for (id, name) in apps {
        let known = sounds
            .iter()
            .any(|sound| matches!(sound, SystemSound::App { id: other, .. } if *other == id));
        if !known && !name.is_empty() {
            sounds.push(SystemSound::App { id, name });
        }
    }
    sounds
}

fn size(size: ngine_capture::Size) -> String {
    format!("{} × {}", size.width, size.height)
}

impl RealEngine {
    pub fn new(capturer: Capturer) -> Self {
        let (pick_sender, picks) = mpsc::channel();
        Self {
            capturer: Arc::new(capturer),
            encoders: working_video_encoders(),
            known: HashMap::new(),
            session: None,
            events: Vec::new(),
            pick_sender,
            picks,
            targets_picked: 0,
            earlier_parts: Duration::ZERO,
            save_folder: None,
            restore_token_file: None,
        }
    }

    /// Keeps what the person chose in the system picker in `file`, so the
    /// picker can skip its dialogue next time, also after a restart, where
    /// the system offers that (the XDG desktop portal on Linux).
    #[must_use]
    pub fn keeping_picks_in(mut self, file: PathBuf) -> Self {
        self.restore_token_file = Some(file);
        self
    }

    fn remember(&mut self, target: Target, capture: ngine_capture::Target) -> Target {
        self.known.insert(target.id.clone(), capture);
        target
    }

    fn list(&mut self, kind: TargetKind) -> Vec<Target> {
        let listed = self
            .capturer
            .lister()
            .and_then(|lister| lister.targets().ok());
        let Some(listed) = listed else {
            return Vec::new();
        };
        match kind {
            TargetKind::Screen => listed
                .screens
                .into_iter()
                .map(|screen| {
                    let target = Target::new(
                        format!("screen-{}", screen.id.0),
                        kind,
                        screen.name.clone(),
                        size(screen.size),
                    );
                    self.remember(target, screen.into())
                })
                .collect(),
            TargetKind::Window => listed
                .windows
                .into_iter()
                .filter(|window| {
                    !window.title.is_empty()
                        && (!window.on_screen
                            || (window.size.width >= SMALLEST_LISTED_WINDOW
                                && window.size.height >= SMALLEST_LISTED_WINDOW))
                })
                .map(|window| {
                    let mut target = Target::new(
                        format!("window-{}", window.id.0),
                        kind,
                        window.title.clone(),
                        window.app_name.clone(),
                    )
                    .elsewhere(!window.on_screen);
                    if let Some(process_id) = window.process_id {
                        target = target.owned_by(format!("app-{process_id}"));
                    }
                    self.remember(target, window.into())
                })
                .collect(),
            TargetKind::App => listed
                .apps
                .into_iter()
                .map(|app| {
                    let target = Target::new(
                        format!("app-{}", app.process_id),
                        kind,
                        app.name.clone(),
                        "",
                    );
                    self.remember(target, app.into())
                })
                .collect(),
        }
    }

    fn take_picks(&mut self) {
        while let Ok(picked) = self.picks.try_recv() {
            let event = match picked {
                Ok(Some(picked)) => {
                    if let (Some(file), Some(token)) =
                        (&self.restore_token_file, &picked.restore_token)
                    {
                        keep_restore_token(file, token);
                    }
                    self.targets_picked += 1;
                    let capture = picked.target;
                    let target = Target::new(
                        format!("picked-{}", self.targets_picked),
                        match capture.kind() {
                            ngine_capture::TargetKind::Screen => TargetKind::Screen,
                            ngine_capture::TargetKind::App => TargetKind::App,
                            _ => TargetKind::Window,
                        },
                        capture.label(),
                        "",
                    );
                    Event::TargetPicked(self.remember(target, capture))
                }
                Ok(None) => Event::PickCancelled,
                Err(error) => {
                    self.events.push(Event::PickCancelled);
                    Event::Problem(problem_of(&error))
                }
            };
            self.events.push(event);
        }
    }

    fn microphone_in_use(&self, wanted: Option<&String>) -> Option<String> {
        let wanted = wanted?;
        let present = self
            .capturer
            .microphones()
            .is_ok_and(|devices| devices.iter().any(|device| &device.id == wanted));
        present.then(|| wanted.clone())
    }

    fn capture_setup(
        &self,
        capture: ngine_capture::Target,
        audio: Audio,
        inputs: &Inputs,
    ) -> CaptureSetup {
        CaptureSetup {
            target: capture,
            show_cursor: true,
            app_sound_only: audio.app_only,
            sound_of: sound_of(inputs),
            microphone: self.microphone_in_use(inputs.microphone.as_ref()),
        }
    }

    fn session_for(
        &mut self,
        target: &Target,
        video: Video,
        audio: Audio,
        inputs: &Inputs,
    ) -> Result<&mut Session, Refusal> {
        let key = SessionKey {
            target: target.id.clone(),
            video,
            audio,
            inputs: inputs.clone(),
        };
        let carried_replay = match &self.session {
            Some(session) if session.key == key && !session.ended => None,
            Some(session) if session.recording => return Err(Refusal::AlreadyRecording),
            Some(session) => session.replay.clone(),
            None => None,
        };
        let reusable = self
            .session
            .as_ref()
            .is_some_and(|session| session.key == key && !session.ended);
        let changed_in_place = !reusable && self.change_capture_in_place(target, &key);
        if !reusable && !changed_in_place {
            self.stop_session();
            let capture = self
                .known
                .get(&target.id)
                .cloned()
                .ok_or(Refusal::Blocked(Problem::CaptureLost))?;
            let config = RecorderConfig {
                video: video_settings(video),
                audio: audio_settings(audio),
                encoders: self.encoders.clone(),
            };
            let setup = self.capture_setup(capture, audio, inputs);
            let recorder = Recorder::start(Arc::clone(&self.capturer), setup, &config)
                .map_err(|_| Refusal::Blocked(Problem::EncoderFailed))?;
            if let Some(replay) = &carried_replay {
                let _queue_is_empty = recorder.set_replay(Some(replay_setup(replay)));
            }
            self.session = Some(Session {
                recorder,
                key,
                target: target.clone(),
                recording: false,
                replay: carried_replay,
                ended: false,
            });
        }
        self.session
            .as_mut()
            .ok_or(Refusal::Blocked(Problem::CaptureLost))
    }

    fn change_capture_in_place(&mut self, target: &Target, key: &SessionKey) -> bool {
        let Some(capture) = self.known.get(&target.id).cloned() else {
            return false;
        };
        let setup = self.capture_setup(capture, key.audio, &key.inputs);
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let audio = audio_settings(key.audio);
        let keeps_its_streams = !session.ended
            && !session.recording
            && session.key.video == key.video
            && audio_settings(session.key.audio).track_count() == audio.track_count();
        if !keeps_its_streams {
            return false;
        }
        if session.recorder.change_capture(setup, audio).is_err() {
            return false;
        }
        session.key = key.clone();
        session.target = target.clone();
        true
    }

    fn stop_session(&mut self) {
        if let Some(session) = self.session.take() {
            for event in session.recorder.stop() {
                self.absorb(event);
            }
        }
    }

    fn stop_session_when_idle(&mut self) {
        let idle = self
            .session
            .as_ref()
            .is_some_and(|session| !session.recording && session.replay.is_none());
        if idle {
            self.stop_session();
        }
    }

    fn absorb(&mut self, event: RecorderEvent) {
        match event {
            RecorderEvent::RecordingStarted { .. } => self.events.push(Event::RecordingStarted),
            RecorderEvent::RecordingFinished {
                length,
                continues: true,
                ..
            } => self.earlier_parts += length,
            RecorderEvent::RecordingFinished { file, length, .. } => {
                self.events.push(Event::RecordingStopped {
                    file,
                    length: self.earlier_parts + length,
                });
                self.earlier_parts = Duration::ZERO;
                self.recording_ended();
            }
            RecorderEvent::RecordingFailed {
                file,
                length,
                error,
            } => {
                let folder = file.parent().map(PathBuf::from);
                self.events
                    .push(Event::Problem(write_problem(&error, folder)));
                if !length.is_zero() {
                    self.events.push(Event::RecordingStopped {
                        file,
                        length: self.earlier_parts + length,
                    });
                }
                self.earlier_parts = Duration::ZERO;
                self.recording_ended();
            }
            RecorderEvent::RecordingDiscarded { .. } => self.recording_ended(),
            RecorderEvent::MarkerAdded { at } => self.events.push(Event::MarkerAdded { at }),
            RecorderEvent::ReplayStarted => self.events.push(Event::ReplayStarted),
            RecorderEvent::ReplayStopped => {
                if let Some(session) = self.session.as_mut() {
                    session.replay = None;
                }
                self.events.push(Event::ReplayStopped);
            }
            RecorderEvent::ReplaySaved { file, length } => {
                self.events.push(Event::ReplaySaved { file, length });
            }
            RecorderEvent::ReplaySaveFailed { error }
            | RecorderEvent::ReplayStorageFailing { error } => {
                self.events.push(Event::Problem(write_problem(
                    &error,
                    self.save_folder.clone(),
                )));
            }
            RecorderEvent::CaptureResumed => {
                self.events.push(Event::Problem(Problem::CaptureRestarted));
            }
            RecorderEvent::CaptureLost(error) => {
                let problem = error.as_ref().map_or(Problem::CaptureLost, problem_of);
                self.events.push(Event::Problem(problem));
            }
            RecorderEvent::Failed(_) => {
                self.events.push(Event::Problem(Problem::EncoderFailed));
            }
            RecorderEvent::Stopped => {
                if let Some(session) = self.session.as_mut() {
                    session.ended = true;
                }
            }
            _ => {}
        }
    }

    fn recording_ended(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.recording = false;
        }
    }

    fn take_recorder_events(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let waiting: Vec<RecorderEvent> =
            std::iter::from_fn(|| session.recorder.try_event()).collect();
        for event in waiting {
            self.absorb(event);
        }
        if self.session.as_ref().is_some_and(|session| session.ended) {
            self.stop_session();
        } else {
            self.stop_session_when_idle();
        }
    }
}

impl Engine for RealEngine {
    fn capabilities(&self) -> Capabilities {
        let budget = memory_budget_bytes(installed_memory_bytes()) / BYTES_PER_MB;
        Capabilities {
            codecs: working_video_codecs(&self.encoders)
                .into_iter()
                .map(|codec| match codec {
                    VideoCodec::H264 => Codec::H264,
                    VideoCodec::Hevc => Codec::Hevc,
                    VideoCodec::Av1 => Codec::Av1,
                })
                .collect(),
            lists_targets: self.capturer.lister().is_some(),
            default_memory_budget_mb: u32::try_from(budget).unwrap_or(u32::MAX),
            simulated: false,
        }
    }

    fn targets(&mut self, kind: TargetKind) -> Vec<Target> {
        self.list(kind)
    }

    fn pick_target(&mut self) {
        let sender = self.pick_sender.clone();
        match self.capturer.picker() {
            Some(picker) => picker.pick(
                picker_options(self.restore_token_file.as_deref()),
                move |picked| {
                    let _engine_is_gone = sender.send(picked);
                },
            ),
            None => self.events.push(Event::PickCancelled),
        }
    }

    fn start_recording(&mut self, request: &RecordingRequest) -> Result<(), Refusal> {
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.recording)
        {
            return Err(Refusal::AlreadyRecording);
        }
        let file = unused_path(&request.folder, &stamped(RECORDING_NAME));
        let session = self.session_for(
            &request.target,
            request.video,
            request.audio,
            &request.inputs,
        )?;
        session
            .recorder
            .start_recording(RecordingStart {
                file,
                include_buffered: None,
            })
            .map_err(|_| Refusal::Blocked(Problem::CaptureLost))?;
        session.recording = true;
        self.earlier_parts = Duration::ZERO;
        Ok(())
    }

    fn stop_recording(&mut self) -> Result<(), Refusal> {
        let session = self
            .session
            .as_mut()
            .filter(|session| session.recording)
            .ok_or(Refusal::NotRecording)?;
        session
            .recorder
            .stop_recording()
            .map_err(|_| Refusal::NotRecording)
    }

    fn start_replay(&mut self, request: &ReplayRequest) -> Result<(), Refusal> {
        let session = self.session_for(
            &request.target,
            request.video,
            request.audio,
            &request.inputs,
        )?;
        session
            .recorder
            .set_replay(Some(replay_setup(request)))
            .map_err(|_| Refusal::Blocked(Problem::CaptureLost))?;
        session.replay = Some(request.clone());
        self.save_folder = Some(request.folder.clone());
        Ok(())
    }

    fn stop_replay(&mut self) -> Result<(), Refusal> {
        let session = self
            .session
            .as_mut()
            .filter(|session| session.replay.is_some())
            .ok_or(Refusal::ReplayOff)?;
        session
            .recorder
            .set_replay(None)
            .map_err(|_| Refusal::ReplayOff)?;
        session.replay = None;
        self.stop_session_when_idle();
        Ok(())
    }

    fn save_replay(&mut self, span: ReplaySpan) -> Result<(), Refusal> {
        let session = self.session.as_ref().ok_or(Refusal::ReplayOff)?;
        let replay = session.replay.as_ref().ok_or(Refusal::ReplayOff)?;
        let held = session.recorder.status().replay.map(|replay| replay.held);
        if held.is_none_or(|held| held.is_zero()) {
            return Err(Refusal::NothingHeld);
        }
        session
            .recorder
            .save_replay(ReplaySaveRequest {
                range: match span {
                    ReplaySpan::Last(length) => SaveRange::Last(length),
                    ReplaySpan::Everything => SaveRange::Everything,
                },
                folder: replay.folder.clone(),
                name: stamped(REPLAY_NAME),
            })
            .map_err(|_| Refusal::NothingHeld)
    }

    fn add_marker(&mut self) -> Result<Duration, Refusal> {
        let session = self
            .session
            .as_ref()
            .filter(|session| session.recording)
            .ok_or(Refusal::NotRecording)?;
        let elapsed = session
            .recorder
            .status()
            .recording
            .map_or(Duration::ZERO, |recording| recording.elapsed);
        session
            .recorder
            .add_marker()
            .map_err(|_| Refusal::NotRecording)?;
        Ok(elapsed)
    }

    fn status(&self) -> Status {
        let Some(session) = &self.session else {
            return Status::default();
        };
        let status = session.recorder.status();
        Status {
            recording: session.recording.then(|| {
                let running = status.recording.as_ref();
                Recording {
                    target: session.target.clone(),
                    elapsed: running.map_or(Duration::ZERO, |recording| recording.elapsed),
                    markers: running.map_or(0, |recording| recording.markers),
                }
            }),
            replay: session.replay.as_ref().map(|request| Replay {
                target: session.target.clone(),
                held: status.replay.map_or(Duration::ZERO, |replay| replay.held),
                length: request.length,
                on_disk: status.replay.is_some_and(|replay| replay.on_disk),
                bytes: status.replay.map_or(0, |replay| replay.bytes),
            }),
            levels: (!session.ended).then(|| levels(status.levels, session.key.audio)),
        }
    }

    fn microphones(&mut self) -> Vec<Device> {
        self.capturer
            .microphones()
            .unwrap_or_default()
            .into_iter()
            .map(|device| Device {
                id: device.id,
                name: device.name,
                is_default: device.is_default,
            })
            .collect()
    }

    fn system_sounds(&mut self) -> Vec<SystemSound> {
        if let Some(lister) = self.capturer.audio_apps() {
            return app_sounds(
                lister
                    .playing()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|app| (app.id, app.name)),
            );
        }
        let Some(listed) = self
            .capturer
            .lister()
            .and_then(|lister| lister.targets().ok())
        else {
            return Vec::new();
        };
        if listed.apps.is_empty() {
            app_sounds(listed.windows.into_iter().filter_map(|window| {
                window
                    .process_id
                    .map(|process_id| (u64::from(process_id), window.app_name))
            }))
        } else {
            app_sounds(
                listed
                    .apps
                    .into_iter()
                    .map(|app| (u64::from(app.process_id), app.name)),
            )
        }
    }

    fn bit_rate(&self, video: Video, audio: Audio) -> u64 {
        bit_rate_cap(&video_settings(video), &audio_settings(audio))
    }

    fn take_events(&mut self) -> Vec<Event> {
        self.take_picks();
        self.take_recorder_events();
        std::mem::take(&mut self.events)
    }

    fn free_space(&self, folder: &Path) -> Option<u64> {
        free_space_bytes(folder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(enabled: bool, volume_percent: u8) -> AudioSource {
        AudioSource {
            enabled,
            volume_percent,
        }
    }

    #[test]
    fn levels_follow_each_sources_volume_and_are_silent_when_it_is_off() {
        let audio = Audio {
            system: source(true, 50),
            microphone: source(false, 100),
            tracks: Tracks::Three,
            app_only: false,
        };
        let peaks = Peaks {
            system: 8_000,
            microphone: 9_000,
        };
        assert_eq!(
            levels(peaks, audio),
            Levels {
                system: 4_000,
                microphone: 0
            }
        );
        let loud = Audio {
            system: source(true, 200),
            ..audio
        };
        assert_eq!(levels(peaks, loud).system, Levels::FULL_SCALE);
    }

    #[test]
    fn each_app_is_offered_once_for_its_own_sound() {
        let sounds = app_sounds([
            (7, "Firefox".to_owned()),
            (7, "Firefox".to_owned()),
            (9, String::new()),
            (3, "Discord".to_owned()),
        ]);
        assert_eq!(
            sounds,
            [
                SystemSound::App {
                    id: 7,
                    name: "Firefox".into()
                },
                SystemSound::App {
                    id: 3,
                    name: "Discord".into()
                },
            ]
        );
        assert_eq!(
            sound_of(&Inputs {
                microphone: None,
                system: sounds[1].clone()
            }),
            Some(AudioApp::new(3, "Discord"))
        );
        assert_eq!(sound_of(&Inputs::default()), None);
    }

    #[test]
    fn the_pickers_choice_is_kept_across_starts_in_a_file() {
        let file = std::env::temp_dir()
            .join(format!("ngine-restore-token-{}", std::process::id()))
            .join("picker-restore-token");
        assert_eq!(picker_options(Some(&file)), PickerOptions::new());
        assert_eq!(picker_options(None), PickerOptions::new());
        let token = RestoreToken::new("4f3c-token");
        keep_restore_token(&file, &token);
        assert_eq!(
            picker_options(Some(&file)),
            PickerOptions::new().with_restore_token(token)
        );
        std::fs::write(&file, "\n").expect("the file is emptied");
        assert_eq!(picker_options(Some(&file)), PickerOptions::new());
        if let Some(folder) = file.parent() {
            std::fs::remove_dir_all(folder).expect("the temp folder is removed");
        }
    }
}
