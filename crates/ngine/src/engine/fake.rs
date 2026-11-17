use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::{
    Audio, Capabilities, Codec, Device, Display, Engine, Event, Inventory, Levels, Problem,
    Quality, Recording, RecordingRequest, Refusal, Replay, ReplayRequest, ReplaySpan,
    ReplayStorage, Status, SystemSound, Target, TargetKind, Tracks, Video,
};

const PICKER_DELAY: Duration = Duration::from_millis(600);
const SAVE_DELAY: Duration = Duration::from_millis(300);
const KEYFRAME_INTERVAL_MS: u128 = 1000;
const AUDIO_TRACK_KBPS: u64 = 128;
const BYTES_PER_MB: u64 = 1_000_000;
const SAMPLE_FREE_BYTES: u64 = 212_400_000_000;
const SAMPLE_SYSTEM_PEAK: u32 = 3_200;
const SAMPLE_MICROPHONE_PEAK: u32 = 450;

#[derive(Clone)]
pub enum Clock {
    System(Instant),
    Manual(Rc<Cell<Duration>>),
}

impl Clock {
    pub fn system() -> Self {
        Self::System(Instant::now())
    }

    pub fn manual() -> Self {
        Self::Manual(Rc::new(Cell::new(Duration::ZERO)))
    }

    pub fn now(&self) -> Duration {
        match self {
            Self::System(start) => start.elapsed(),
            Self::Manual(now) => now.get(),
        }
    }

    pub fn advance(&self, by: Duration) {
        if let Self::Manual(now) = self {
            now.set(now.get() + by);
        }
    }
}

struct RunningRecording {
    request: RecordingRequest,
    started: Duration,
    markers: u32,
}

struct RunningReplay {
    request: ReplayRequest,
    started: Duration,
}

struct PendingSave {
    due: Duration,
    file: PathBuf,
    length: Duration,
}

struct Inner {
    clock: Clock,
    lists_targets: bool,
    recording: Option<RunningRecording>,
    replay: Option<RunningReplay>,
    pick_due: Option<Duration>,
    saves: Vec<PendingSave>,
    events: Vec<Event>,
    blocked_by: Option<Problem>,
    files_written: u32,
}

/// Stands in for the recording engine until the real one exists. It keeps
/// time and state and writes no files.
#[derive(Clone)]
pub struct FakeEngine {
    inner: Rc<RefCell<Inner>>,
}

impl FakeEngine {
    pub fn new(clock: Clock, lists_targets: bool) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                clock,
                lists_targets,
                recording: None,
                replay: None,
                pick_due: None,
                saves: Vec::new(),
                events: Vec::new(),
                blocked_by: None,
                files_written: 0,
            })),
        }
    }

    pub fn advance(&self, by: Duration) {
        self.inner.borrow().clock.advance(by);
    }

    pub fn cancel_pick(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.pick_due.take().is_some() {
            inner.events.push(Event::PickCancelled);
        }
    }

    pub fn raise(&self, problem: Problem) {
        self.inner.borrow_mut().events.push(Event::Problem(problem));
    }

    pub fn block_starts_with(&self, problem: Option<Problem>) {
        self.inner.borrow_mut().blocked_by = problem;
    }
}

impl Inner {
    fn capturing_audio(&self) -> Option<Audio> {
        self.recording
            .as_ref()
            .map(|recording| recording.request.audio)
            .or_else(|| self.replay.as_ref().map(|replay| replay.request.audio))
    }

    fn next_file(&mut self, folder: &std::path::Path, prefix: &str) -> PathBuf {
        self.files_written += 1;
        folder.join(format!("{prefix} {}.mp4", self.files_written))
    }

    fn held(&self, replay: &RunningReplay) -> Duration {
        self.clock
            .now()
            .saturating_sub(replay.started)
            .min(replay.request.length)
    }

    fn settle(&mut self) {
        let now = self.clock.now();
        if self.pick_due.is_some_and(|due| due <= now) {
            self.pick_due = None;
            self.events.push(Event::TargetPicked(Target::new(
                "picked-window",
                TargetKind::Window,
                "Counter-Strike 2",
                "",
            )));
        }
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.saves)
            .into_iter()
            .partition(|save| save.due <= now);
        self.saves = waiting;
        self.events
            .extend(done.into_iter().map(|save| Event::ReplaySaved {
                file: save.file,
                length: save.length,
            }));
    }
}

fn clip_length(running: Duration, held: Duration, span: ReplaySpan) -> Duration {
    match span {
        ReplaySpan::Everything => held,
        ReplaySpan::Last(asked) if asked >= held => held,
        ReplaySpan::Last(asked) => {
            let wanted_start_ms = running.saturating_sub(asked).as_millis();
            let keyframe_ms = wanted_start_ms - wanted_start_ms % KEYFRAME_INTERVAL_MS;
            let keyframe = Duration::from_millis(u64::try_from(keyframe_ms).unwrap_or(u64::MAX));
            running.saturating_sub(keyframe).min(held)
        }
    }
}

fn video_kbps(video: Video) -> u64 {
    let base = match video.quality {
        Quality::Low => 5_600,
        Quality::Medium => 8_600,
        Quality::High => 11_600,
        Quality::Highest => 19_600,
    };
    let codec_percent = match video.codec {
        Codec::H264 => 100,
        Codec::Hevc => 70,
        Codec::Av1 => 60,
    };
    let rate_percent = 20 + u64::from(video.frame_rate) * 80 / 60;
    base * codec_percent / 100 * rate_percent / 100
}

fn audio_kbps(audio: Audio) -> u64 {
    match audio.tracks {
        Tracks::Three => 3 * AUDIO_TRACK_KBPS,
        Tracks::MixOnly => AUDIO_TRACK_KBPS,
    }
}

fn screens() -> Vec<Target> {
    vec![
        target(
            TargetKind::Screen,
            "screen-1",
            "Built-in display",
            "3024 × 1964",
        )
        .on_display(Display {
            id: 1,
            width: 3024,
            height: 1964,
        }),
        target(
            TargetKind::Screen,
            "screen-2",
            "LG UltraGear",
            "2560 × 1440",
        )
        .on_display(Display {
            id: 2,
            width: 2560,
            height: 1440,
        }),
    ]
}

fn window(id: &str, title: &str, app: &str, owner: &str) -> Target {
    Target::new(id, TargetKind::Window, title, app).owned_by(owner)
}

fn windows() -> Vec<Target> {
    vec![
        window("window-1", "Counter-Strike 2", "Counter-Strike 2", "app-1"),
        window("window-2", "#general · Squad", "Discord", "app-2"),
        window("window-3", "Voice connected", "Discord", "app-2").elsewhere(true),
        window(
            "window-4",
            "Patch notes – Counter-Strike 2",
            "Firefox",
            "app-3",
        ),
        window("window-5", "Ranked map pool explained", "Firefox", "app-3"),
        window("window-6", "Inbox", "Firefox", "app-3").elsewhere(true),
    ]
}

fn apps() -> Vec<Target> {
    vec![
        Target::new("app-1", TargetKind::App, "Counter-Strike 2", ""),
        Target::new("app-2", TargetKind::App, "Discord", ""),
        Target::new("app-3", TargetKind::App, "Firefox", ""),
    ]
}

fn microphones() -> Vec<Device> {
    let device = |id: &str, name: &str, is_default: bool| Device {
        id: id.into(),
        name: name.into(),
        is_default,
    };
    vec![
        device("built-in", "MacBook Pro Microphone", true),
        device("usb-mv7", "Shure MV7", false),
    ]
}

fn system_sounds() -> Vec<SystemSound> {
    let app = |id: u64, name: &str| SystemSound::App {
        id,
        name: name.into(),
    };
    vec![app(2, "Discord"), app(3, "Firefox")]
}

fn target(kind: TargetKind, id: &str, name: &str, detail: &str) -> Target {
    Target::new(id, kind, name, detail)
}

impl Engine for FakeEngine {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            codecs: vec![Codec::H264, Codec::Hevc],
            lists_targets: self.inner.borrow().lists_targets,
            default_memory_budget_mb: 512,
            simulated: true,
        }
    }

    fn read_inventory(&mut self, _: &Path) {
        let mut inner = self.inner.borrow_mut();
        let sources = if inner.lists_targets {
            [screens(), windows(), apps()].concat()
        } else {
            Vec::new()
        };
        inner.events.push(Event::Inventory(Inventory {
            sources,
            microphones: microphones(),
            system_sounds: system_sounds(),
            free_space: Some(SAMPLE_FREE_BYTES),
        }));
    }

    fn pick_target(&mut self) {
        let mut inner = self.inner.borrow_mut();
        inner.pick_due = Some(inner.clock.now() + PICKER_DELAY);
    }

    fn start_recording(&mut self, request: &RecordingRequest) -> Result<(), Refusal> {
        let mut inner = self.inner.borrow_mut();
        if inner.recording.is_some() {
            return Err(Refusal::AlreadyRecording);
        }
        if let Some(problem) = inner.blocked_by.clone() {
            return Err(Refusal::Blocked(problem));
        }
        inner.recording = Some(RunningRecording {
            request: request.clone(),
            started: inner.clock.now(),
            markers: 0,
        });
        inner.events.push(Event::RecordingStarted);
        Ok(())
    }

    fn stop_recording(&mut self) -> Result<(), Refusal> {
        let mut inner = self.inner.borrow_mut();
        let recording = inner.recording.take().ok_or(Refusal::NotRecording)?;
        let length = inner.clock.now().saturating_sub(recording.started);
        let file = inner.next_file(&recording.request.folder, "Recording");
        inner.events.push(Event::RecordingStopped { file, length });
        Ok(())
    }

    fn start_replay(&mut self, request: &ReplayRequest) -> Result<(), Refusal> {
        let mut inner = self.inner.borrow_mut();
        if let Some(problem) = inner.blocked_by.clone() {
            return Err(Refusal::Blocked(problem));
        }
        let now = inner.clock.now();
        let already_on = inner.replay.is_some();
        let started = inner.replay.as_ref().map_or(now, |replay| replay.started);
        inner.replay = Some(RunningReplay {
            request: request.clone(),
            started,
        });
        if !already_on {
            inner.events.push(Event::ReplayStarted);
        }
        Ok(())
    }

    fn stop_replay(&mut self) -> Result<(), Refusal> {
        let mut inner = self.inner.borrow_mut();
        inner.replay.take().ok_or(Refusal::ReplayOff)?;
        inner.events.push(Event::ReplayStopped);
        Ok(())
    }

    fn save_replay(&mut self, span: ReplaySpan) -> Result<(), Refusal> {
        let mut inner = self.inner.borrow_mut();
        let now = inner.clock.now();
        let (running, held, folder, clear) = {
            let replay = inner.replay.as_ref().ok_or(Refusal::ReplayOff)?;
            (
                now.saturating_sub(replay.started),
                inner.held(replay),
                replay.request.folder.clone(),
                replay.request.clear_after_save,
            )
        };
        if held.is_zero() {
            return Err(Refusal::NothingHeld);
        }
        let file = inner.next_file(&folder, "Replay");
        inner.saves.push(PendingSave {
            due: now + SAVE_DELAY,
            file,
            length: clip_length(running, held, span),
        });
        if clear && let Some(replay) = inner.replay.as_mut() {
            replay.started = now;
        }
        Ok(())
    }

    fn add_marker(&mut self) -> Result<Duration, Refusal> {
        let mut inner = self.inner.borrow_mut();
        let now = inner.clock.now();
        let recording = inner.recording.as_mut().ok_or(Refusal::NotRecording)?;
        recording.markers += 1;
        let at = now.saturating_sub(recording.started);
        inner.events.push(Event::MarkerAdded { at });
        Ok(at)
    }

    fn status(&self) -> Status {
        let inner = self.inner.borrow();
        let now = inner.clock.now();
        Status {
            recording: inner.recording.as_ref().map(|recording| Recording {
                target: recording.request.target.clone(),
                elapsed: now.saturating_sub(recording.started),
                markers: recording.markers,
            }),
            replay: inner.replay.as_ref().map(|replay| {
                let held = inner.held(replay);
                let bytes_per_second =
                    self.bit_rate(replay.request.video, replay.request.audio) / 8;
                Replay {
                    target: replay.request.target.clone(),
                    held,
                    length: replay.request.length,
                    on_disk: holds_on_disk(&replay.request, self),
                    bytes: bytes_per_second * held.as_secs(),
                }
            }),
            levels: inner.capturing_audio().map(sample_levels),
        }
    }

    fn bit_rate(&self, video: Video, audio: Audio) -> u64 {
        (video_kbps(video) + audio_kbps(audio)) * 1000
    }

    fn take_events(&mut self) -> Vec<Event> {
        let mut inner = self.inner.borrow_mut();
        inner.settle();
        std::mem::take(&mut inner.events)
    }

    fn free_space(&self, _: &Path) -> Option<u64> {
        Some(SAMPLE_FREE_BYTES)
    }
}

fn sample_levels(audio: Audio) -> Levels {
    let level = |source: super::AudioSource, loudest: u32| {
        let scaled = loudest * u32::from(source.volume_percent) / 100;
        if source.enabled {
            u16::try_from(scaled).unwrap_or(Levels::FULL_SCALE)
        } else {
            0
        }
    };
    Levels {
        system: level(audio.system, SAMPLE_SYSTEM_PEAK),
        microphone: level(audio.microphone, SAMPLE_MICROPHONE_PEAK),
    }
}

fn holds_on_disk(request: &ReplayRequest, engine: &FakeEngine) -> bool {
    match request.storage {
        ReplayStorage::Memory => false,
        ReplayStorage::Disk => true,
        ReplayStorage::Auto => {
            let bytes =
                engine.bit_rate(request.video, request.audio) / 8 * request.length.as_secs();
            bytes > u64::from(request.memory_budget_mb) * BYTES_PER_MB
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AudioSource;

    fn video() -> Video {
        Video {
            codec: Codec::H264,
            quality: Quality::High,
            frame_rate: 60,
        }
    }

    fn audio() -> Audio {
        let source = AudioSource {
            enabled: true,
            volume_percent: 100,
        };
        Audio {
            system: source,
            microphone: source,
            tracks: Tracks::Three,
            app_only: true,
        }
    }

    fn replay_request(length: Duration) -> ReplayRequest {
        ReplayRequest {
            target: screens().remove(0),
            video: video(),
            audio: audio(),
            inputs: super::super::Inputs::default(),
            length,
            storage: ReplayStorage::Auto,
            memory_budget_mb: 512,
            buffer_folder: PathBuf::from("buffer"),
            clear_after_save: false,
            folder: PathBuf::from("clips"),
        }
    }

    fn saved_length(engine: &mut FakeEngine) -> Duration {
        engine.advance(SAVE_DELAY);
        engine
            .take_events()
            .into_iter()
            .find_map(|event| match event {
                Event::ReplaySaved { length, .. } => Some(length),
                _ => None,
            })
            .expect("a saved replay")
    }

    #[test]
    fn a_saved_clip_is_never_shorter_than_asked_and_at_most_a_second_longer() {
        let mut engine = FakeEngine::new(Clock::manual(), true);
        engine
            .start_replay(&replay_request(Duration::from_secs(120)))
            .expect("replay starts");
        engine.advance(Duration::from_millis(100_800));
        engine
            .save_replay(ReplaySpan::Last(Duration::from_secs(30)))
            .expect("save accepted");
        let length = saved_length(&mut engine);
        assert!(length >= Duration::from_secs(30));
        assert!(length <= Duration::from_secs(31));
    }

    #[test]
    fn asking_for_more_than_the_buffer_holds_saves_what_it_holds() {
        let mut engine = FakeEngine::new(Clock::manual(), true);
        engine
            .start_replay(&replay_request(Duration::from_secs(60)))
            .expect("replay starts");
        engine.advance(Duration::from_secs(12));
        engine
            .save_replay(ReplaySpan::Last(Duration::from_secs(30)))
            .expect("save accepted");
        assert_eq!(saved_length(&mut engine), Duration::from_secs(12));
    }

    #[test]
    fn the_buffer_never_holds_more_than_its_length() {
        let mut engine = FakeEngine::new(Clock::manual(), true);
        engine
            .start_replay(&replay_request(Duration::from_secs(60)))
            .expect("replay starts");
        engine.advance(Duration::from_secs(600));
        let held = engine.status().replay.expect("replay is on").held;
        assert_eq!(held, Duration::from_secs(60));
    }

    #[test]
    fn saving_with_replay_off_is_refused() {
        let mut engine = FakeEngine::new(Clock::manual(), true);
        assert_eq!(
            engine.save_replay(ReplaySpan::Everything),
            Err(Refusal::ReplayOff)
        );
    }

    #[test]
    fn a_minute_at_the_default_quality_costs_about_ninety_megabytes() {
        let engine = FakeEngine::new(Clock::manual(), true);
        let megabytes = engine.bit_rate(video(), audio()) / 8 * 60 / BYTES_PER_MB;
        assert!((85..=95).contains(&megabytes), "{megabytes} MB");
    }

    #[test]
    fn a_long_buffer_goes_to_disk_when_storage_is_automatic() {
        let mut engine = FakeEngine::new(Clock::manual(), true);
        engine
            .start_replay(&replay_request(Duration::from_mins(20)))
            .expect("replay starts");
        assert!(engine.status().replay.expect("replay is on").on_disk);
    }

    #[test]
    fn the_picker_answers_with_a_target() {
        let mut engine = FakeEngine::new(Clock::manual(), false);
        engine.read_inventory(Path::new("clips"));
        assert!(matches!(
            engine.take_events().as_slice(),
            [Event::Inventory(inventory)] if inventory.sources.is_empty()
        ));
        engine.pick_target();
        assert_eq!(engine.take_events(), Vec::new());
        engine.advance(PICKER_DELAY);
        assert!(matches!(
            engine.take_events().as_slice(),
            [Event::TargetPicked(_)]
        ));
    }
}
