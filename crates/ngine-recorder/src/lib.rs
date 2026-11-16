mod audio;
mod clip;
mod error;
mod event;
mod markers;
mod pipeline;
mod recorder;
pub mod replay;
mod settings;
mod sink;
mod time;
mod video;

pub use clip::unused_path;
pub use error::RecorderError;
pub use event::RecorderEvent;
pub use markers::{BOX_UUID, journal_path, read_markers};
pub use ngine_media::{
    AudioCodec, EncoderInfo, VideoCodec, working_video_codecs, working_video_encoders,
};
pub use pipeline::{
    Pipeline, RecorderConfig, RecordingStart, RecordingStatus, ReplaySaveRequest, ReplaySetup,
    ReplayStatus, Status,
};
pub use recorder::{CaptureSetup, Recorder};
pub use settings::{
    AUDIO_TRACK_BIT_RATE, AudioSettings, HIGHEST_FRAME_RATE, Quality, Tracks, VideoSettings,
    bit_rate_cap, video_bit_rate_cap,
};
