use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub mod fake;
pub mod real;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Screen,
    Window,
    App,
}

impl TargetKind {
    pub const ALL: [Self; 3] = [Self::Screen, Self::Window, Self::App];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub kind: TargetKind,
    pub name: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub const ALL: [Self; 3] = [Self::H264, Self::Hevc, Self::Av1];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Low,
    Medium,
    High,
    Highest,
}

impl Quality {
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Highest];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracks {
    Three,
    MixOnly,
}

impl Tracks {
    pub const ALL: [Self; 2] = [Self::Three, Self::MixOnly];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayStorage {
    Auto,
    Memory,
    Disk,
}

impl ReplayStorage {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Memory, Self::Disk];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Video {
    pub codec: Codec,
    pub quality: Quality,
    pub frame_rate: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioSource {
    pub enabled: bool,
    pub volume_percent: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audio {
    pub system: AudioSource,
    pub microphone: AudioSource,
    pub tracks: Tracks,
    pub app_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingRequest {
    pub target: Target,
    pub video: Video,
    pub audio: Audio,
    pub folder: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayRequest {
    pub target: Target,
    pub video: Video,
    pub audio: Audio,
    pub length: Duration,
    pub storage: ReplayStorage,
    pub memory_budget_mb: u32,
    pub buffer_folder: PathBuf,
    pub clear_after_save: bool,
    pub folder: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaySpan {
    Last(Duration),
    Everything,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub codecs: Vec<Codec>,
    pub lists_targets: bool,
    pub default_memory_budget_mb: u32,
    pub simulated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub recording: Option<Recording>,
    pub replay: Option<Replay>,
    pub levels: Option<Levels>,
}

/// How loud each source is right now, from 0 (silent) to 100 (full scale).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Levels {
    pub system: u8,
    pub microphone: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recording {
    pub target: Target,
    pub elapsed: Duration,
    pub markers: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replay {
    pub target: Target,
    pub held: Duration,
    pub length: Duration,
    pub on_disk: bool,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    ScreenRecording,
    Microphone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    PermissionMissing(Permission),
    CaptureRestarted,
    CaptureLost,
    DiskFull { folder: PathBuf },
    WriteFailed,
    EncoderFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    TargetPicked(Target),
    PickCancelled,
    RecordingStarted,
    RecordingStopped { file: PathBuf, length: Duration },
    ReplayStarted,
    ReplayStopped,
    ReplaySaved { file: PathBuf, length: Duration },
    MarkerAdded { at: Duration },
    Problem(Problem),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    AlreadyRecording,
    NotRecording,
    ReplayOff,
    NothingHeld,
    Blocked(Problem),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRecording => f.write_str("A recording is already running"),
            Self::NotRecording => f.write_str("No recording is running"),
            Self::ReplayOff => f.write_str("Instant replay is off"),
            Self::NothingHeld => f.write_str("The replay buffer is still empty"),
            Self::Blocked(problem) => f.write_str(problem.summary()),
        }
    }
}

impl Problem {
    pub fn summary(&self) -> &'static str {
        match self {
            Self::PermissionMissing(Permission::ScreenRecording) => {
                "ngine is not allowed to record the screen"
            }
            Self::PermissionMissing(Permission::Microphone) => {
                "ngine is not allowed to use the microphone"
            }
            Self::CaptureRestarted => "Capture stopped for a moment and was restarted",
            Self::CaptureLost => "Capture stopped and could not be restarted",
            Self::DiskFull { .. } => "The disk is full",
            Self::WriteFailed => "A file could not be written",
            Self::EncoderFailed => "The video encoder stopped working",
        }
    }
}

/// What the app needs from the recording engine.
///
/// Every method returns at once. Work that takes time reports its outcome
/// through [`Engine::take_events`], which the caller drains on its own thread.
pub trait Engine {
    fn capabilities(&self) -> Capabilities;

    /// Empty when [`Capabilities::lists_targets`] is false.
    fn targets(&mut self, kind: TargetKind) -> Vec<Target>;

    /// Opens the system picker. The answer arrives as [`Event::TargetPicked`]
    /// or [`Event::PickCancelled`].
    fn pick_target(&mut self);

    fn start_recording(&mut self, request: &RecordingRequest) -> Result<(), Refusal>;

    /// The file arrives as [`Event::RecordingStopped`].
    fn stop_recording(&mut self) -> Result<(), Refusal>;

    /// Calling it while replay is on applies the new request and keeps what
    /// the buffer holds when it can.
    fn start_replay(&mut self, request: &ReplayRequest) -> Result<(), Refusal>;

    fn stop_replay(&mut self) -> Result<(), Refusal>;

    /// The clip arrives as [`Event::ReplaySaved`] with its real length.
    fn save_replay(&mut self, span: ReplaySpan) -> Result<(), Refusal>;

    /// Returns the marker's position in the running recording.
    fn add_marker(&mut self) -> Result<Duration, Refusal>;

    fn status(&self) -> Status;

    /// The capped bit rate of video and audio together, in bits per second.
    fn bit_rate(&self, video: Video, audio: Audio) -> u64;

    fn take_events(&mut self) -> Vec<Event>;

    /// The bytes still free on the disk that holds `folder`, when the system
    /// can say.
    fn free_space(&self, folder: &Path) -> Option<u64>;
}
