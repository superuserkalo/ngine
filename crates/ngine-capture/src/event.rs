use crate::audio::AudioBuffer;
use crate::error::CaptureError;
use crate::frame::VideoFrame;

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EndReason {
    /// [`Session::stop`](crate::Session::stop) was called.
    Stopped,
    /// The user ended the capture from the operating system's own controls.
    StoppedByUser,
    /// Capture broke and could not be restarted.
    Failed(CaptureError),
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Event {
    Video(VideoFrame),
    Audio(AudioBuffer),
    /// Capture broke. Nothing arrives until [`Event::Resumed`] or
    /// [`Event::Ended`] follows.
    Interrupted(CaptureError),
    /// Capture runs again after an interruption. Timestamps continue on the
    /// same clock with a gap, and the frame size can differ from before.
    Resumed,
    /// The last event of a session.
    Ended(EndReason),
}

impl Event {
    /// The time of a frame or buffer. Other events carry none.
    pub fn timestamp(&self) -> Option<crate::time::Timestamp> {
        match self {
            Self::Video(frame) => Some(frame.timestamp()),
            Self::Audio(buffer) => Some(buffer.timestamp()),
            Self::Interrupted(_) | Self::Resumed | Self::Ended(_) => None,
        }
    }
}
