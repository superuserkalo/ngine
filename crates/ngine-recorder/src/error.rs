use std::fmt;
use std::io;
use std::path::PathBuf;

use ngine_capture::CaptureError;
use ngine_media::VideoCodec;

use crate::replay::ReplayError;

#[derive(Debug)]
#[non_exhaustive]
pub enum RecorderError {
    CodecUnavailable(VideoCodec),
    EncoderUnavailable(String),
    FrameRateOutOfRange(u32),
    Capture(CaptureError),
    Media(ngine_media::Error),
    Replay(ReplayError),
    File {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    AlreadyRecording,
    NotRecording,
    ReplayOff,
    NothingHeld,
    /// The new settings change what the tracks hold, which a running
    /// recorder cannot do. Start a new recorder for them.
    NeedsANewRecorder,
    /// Too many commands or saves are waiting. Nothing was started.
    Busy,
    /// The recorder's thread has ended.
    Stopped,
}

impl RecorderError {
    pub(crate) fn file(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::File {
            action,
            path: path.into(),
            source,
        }
    }

    /// Whether the cause is a disk with no room left.
    pub fn is_disk_full(&self) -> bool {
        const NO_SPACE_LEFT: i32 = -28;
        match self {
            Self::File { source, .. } | Self::Replay(ReplayError::Storage { source, .. }) => {
                source.kind() == io::ErrorKind::StorageFull
            }
            Self::Media(ngine_media::Error::Ffmpeg { code, .. }) => *code == NO_SPACE_LEFT,
            _ => false,
        }
    }
}

impl fmt::Display for RecorderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CodecUnavailable(codec) => {
                write!(formatter, "this machine has no working {codec:?} encoder")
            }
            Self::EncoderUnavailable(name) => {
                write!(
                    formatter,
                    "the encoder {name} does not work on this machine"
                )
            }
            Self::FrameRateOutOfRange(rate) => {
                write!(formatter, "a frame rate of {rate} is outside 1 to 240")
            }
            Self::Capture(error) => write!(formatter, "capture failed: {error}"),
            Self::Media(error) => write!(formatter, "{error}"),
            Self::Replay(error) => write!(formatter, "{error}"),
            Self::File {
                action,
                path,
                source,
            } => write!(
                formatter,
                "could not {action} at {}: {source}",
                path.display()
            ),
            Self::NeedsANewRecorder => formatter
                .write_str("the new settings change the tracks, which needs a new recorder"),
            Self::AlreadyRecording => formatter.write_str("a recording is already running"),
            Self::NotRecording => formatter.write_str("no recording is running"),
            Self::ReplayOff => formatter.write_str("instant replay is off"),
            Self::NothingHeld => formatter.write_str("the replay buffer is still empty"),
            Self::Busy => formatter.write_str("the recorder is busy, try again in a moment"),
            Self::Stopped => formatter.write_str("the recorder has stopped"),
        }
    }
}

impl std::error::Error for RecorderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Capture(error) => Some(error),
            Self::Media(error) => Some(error),
            Self::Replay(error) => Some(error),
            Self::File { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<ngine_media::Error> for RecorderError {
    fn from(error: ngine_media::Error) -> Self {
        Self::Media(error)
    }
}

impl From<CaptureError> for RecorderError {
    fn from(error: CaptureError) -> Self {
        Self::Capture(error)
    }
}

impl From<ReplayError> for RecorderError {
    fn from(error: ReplayError) -> Self {
        Self::Replay(error)
    }
}
