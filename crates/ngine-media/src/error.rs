use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Ffmpeg {
        action: &'static str,
        code: i32,
        message: String,
    },
    Unavailable(String),
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub(crate) fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ffmpeg {
                action,
                code,
                message,
            } => write!(formatter, "FFmpeg could not {action}: {message} ({code})"),
            Self::Unavailable(message) => {
                write!(formatter, "not available in this FFmpeg build: {message}")
            }
            Self::Invalid(message) => write!(formatter, "invalid input: {message}"),
        }
    }
}

impl std::error::Error for Error {}
