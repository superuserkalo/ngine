mod audio;
mod clip;
mod disk;
mod error;
mod event;
mod export;
mod levels;
mod library;
mod markers;
mod pipeline;
mod player;
mod recorder;
mod recover;
pub mod replay;
mod settings;
mod sink;
mod sound;
mod thumbnail;
mod time;
mod video;

pub use clip::unused_path;
pub use disk::free_space_bytes;
pub use error::RecorderError;
pub use event::RecorderEvent;
pub use export::{Cut, ExportControl, ExportRequest, ExportSummary, export_clip};
pub use levels::{Peaks, REPORT_EVERY as LEVELS_REPORT_EVERY};
pub use library::{LibraryItem, MediaInfo, RenameError, probe, rename, scan};
pub use markers::{BOX_UUID, journal_path, read_markers};
pub use ngine_media::{
    AudioCodec, EncoderInfo, Error as MediaError, ProbedEncoder, SurfaceKind, Trial, VideoCodec,
    probe_video_encoders, working_video_codecs, working_video_encoders,
};
pub use pipeline::{
    Pipeline, RecorderConfig, RecordingStart, RecordingStatus, ReplaySaveRequest, ReplaySetup,
    ReplayStatus, Status,
};
pub use player::{Player, PlayerOptions, Playhead, SoundFeed};
pub use recorder::{CaptureSetup, Recorder};
pub use recover::{Recovery, finish_recording, finish_unfinished_recordings, is_unfinished};
pub use settings::{
    AUDIO_TRACK_BIT_RATE, AudioSettings, HIGHEST_FRAME_RATE, Quality, Tracks, VideoSettings,
    bit_rate_cap, video_bit_rate_cap,
};
pub use sound::ClipMix;
pub use thumbnail::{Picture, ThumbnailCache, picture_at};
