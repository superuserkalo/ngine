use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use super::packet::StreamIndex;
use super::time::Ticks;

#[derive(Debug)]
pub enum ReplayError {
    LengthOutOfRange {
        asked: Duration,
    },
    NegativeDuration {
        stream: StreamIndex,
        duration: Ticks,
    },
    KeyframeNotAfterPrevious {
        previous: Ticks,
        pts: Ticks,
    },
    NoDiskFolder,
    Storage {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl ReplayError {
    pub(crate) fn storage(
        action: &'static str,
        path: impl Into<PathBuf>,
        source: io::Error,
    ) -> Self {
        Self::Storage {
            action,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LengthOutOfRange { asked } => write!(
                formatter,
                "a replay length of {asked:?} is outside 15 seconds to 30 minutes"
            ),
            Self::NegativeDuration { stream, duration } => write!(
                formatter,
                "a packet of stream {} has the negative duration {}",
                stream.get(),
                duration.0
            ),
            Self::KeyframeNotAfterPrevious { previous, pts } => write!(
                formatter,
                "a video keyframe at {} does not come after the previous one at {}; begin a new epoch when timestamps restart",
                pts.0, previous.0
            ),
            Self::NoDiskFolder => write!(
                formatter,
                "a replay buffer on disk needs a folder and none was given"
            ),
            Self::Storage {
                action,
                path,
                source,
            } => write!(
                formatter,
                "could not {action} at {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ReplayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage { source, .. } => Some(source),
            Self::LengthOutOfRange { .. }
            | Self::NoDiskFolder
            | Self::NegativeDuration { .. }
            | Self::KeyframeNotAfterPrevious { .. } => None,
        }
    }
}
