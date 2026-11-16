use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    ScreenRecording,
    Microphone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CaptureError {
    /// The user has not allowed this process to use the named input.
    PermissionDenied(Permission),
    /// The screen, window or app no longer exists.
    TargetGone,
    /// This operating system cannot do what was asked.
    Unsupported(String),
    /// Something capture depends on is missing, such as a display server, a
    /// system service or an input device.
    Unavailable(String),
    /// The operating system reported a failure.
    Os(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermissionDenied(Permission::ScreenRecording) => {
                formatter.write_str("screen recording is not allowed for this app")
            }
            Self::PermissionDenied(Permission::Microphone) => {
                formatter.write_str("the microphone is not allowed for this app")
            }
            Self::TargetGone => formatter.write_str("the capture target no longer exists"),
            Self::Unsupported(what) => write!(formatter, "not supported here: {what}"),
            Self::Unavailable(what) => write!(formatter, "not available: {what}"),
            Self::Os(message) => write!(formatter, "the operating system failed: {message}"),
        }
    }
}

impl std::error::Error for CaptureError {}
