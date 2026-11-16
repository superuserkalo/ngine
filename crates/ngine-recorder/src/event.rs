use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use ngine_capture::CaptureError;

use crate::error::RecorderError;

/// What the recorder reports while it runs. Every state change the user
/// should hear about arrives here, so nothing stops silently.
#[derive(Debug)]
#[non_exhaustive]
pub enum RecorderEvent {
    RecordingStarted {
        file: PathBuf,
    },
    /// The file is complete. `continues` says the recording goes on in a new
    /// part, as after the captured picture changed its size.
    RecordingFinished {
        file: PathBuf,
        length: Duration,
        continues: bool,
    },
    /// Writing failed and the recording has ended. The file plays up to
    /// `length`.
    RecordingFailed {
        file: PathBuf,
        length: Duration,
        error: RecorderError,
    },
    /// The recording was stopped before its first frame arrived, so there is
    /// no file.
    RecordingDiscarded {
        file: PathBuf,
    },
    MarkerAdded {
        at: Duration,
    },
    ReplayStarted,
    ReplayStopped,
    /// One clip was written. A save that spans a change of picture size
    /// writes one clip per size.
    ReplaySaved {
        file: PathBuf,
        length: Duration,
    },
    ReplaySaveFailed {
        error: RecorderError,
    },
    /// The replay buffer could not store what it was given. It keeps running
    /// with what it holds.
    ReplayStorageFailing {
        error: RecorderError,
    },
    /// Capture broke and is being restarted. The recording and the replay
    /// buffer keep what they have.
    CaptureInterrupted(CaptureError),
    CaptureResumed,
    /// Capture ended and could not be restarted, or the user ended it from
    /// the operating system's controls. The recorder has stopped.
    CaptureLost(Option<CaptureError>),
    /// Encoding failed and the recorder has stopped. Files already written
    /// were finished first.
    Failed(RecorderError),
    /// The last event: the recorder's thread has ended.
    Stopped,
}

#[derive(Clone)]
pub(crate) struct Events(Sender<RecorderEvent>);

impl Events {
    pub(crate) fn new(sender: Sender<RecorderEvent>) -> Self {
        Self(sender)
    }

    pub(crate) fn send(&self, event: RecorderEvent) {
        let _nobody_listens = self.0.send(event);
    }
}
