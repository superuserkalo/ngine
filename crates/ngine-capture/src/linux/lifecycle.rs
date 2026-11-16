use crate::error::CaptureError;
use crate::event::{EndReason, Event};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Change {
    Streaming,
    Paused(CaptureError),
    Disconnected,
    Failed(CaptureError),
}

#[derive(Debug, Default)]
pub(super) struct Lifecycle {
    has_streamed: bool,
    interrupted: bool,
    ended: bool,
}

impl Lifecycle {
    pub(super) fn has_ended(&self) -> bool {
        self.ended
    }

    pub(super) fn on(&mut self, change: Change) -> Option<Event> {
        if self.ended {
            return None;
        }
        match change {
            Change::Streaming => {
                self.has_streamed = true;
                std::mem::take(&mut self.interrupted).then_some(Event::Resumed)
            }
            Change::Paused(why) => {
                let breaks_a_running_capture = self.has_streamed && !self.interrupted;
                self.interrupted |= breaks_a_running_capture;
                breaks_a_running_capture.then_some(Event::Interrupted(why))
            }
            Change::Disconnected => {
                self.ended = true;
                Some(Event::Ended(if self.has_streamed {
                    EndReason::StoppedByUser
                } else {
                    EndReason::Failed(CaptureError::TargetGone)
                }))
            }
            Change::Failed(why) => {
                self.ended = true;
                Some(Event::Ended(EndReason::Failed(why)))
            }
        }
    }
}

pub(super) fn stream_error(message: &str) -> CaptureError {
    let lowered = message.to_lowercase();
    if lowered.contains("no target") || lowered.contains("target not found") {
        CaptureError::TargetGone
    } else {
        CaptureError::Os(message.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paused() -> Change {
        Change::Paused(CaptureError::Os("renegotiating".to_owned()))
    }

    #[test]
    fn starting_up_reports_nothing() {
        let mut lifecycle = Lifecycle::default();

        assert!(lifecycle.on(paused()).is_none());
        assert!(lifecycle.on(Change::Streaming).is_none());
        assert!(lifecycle.on(Change::Streaming).is_none());
    }

    #[test]
    fn a_pause_while_running_interrupts_once_and_streaming_resumes() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.on(Change::Streaming);

        assert!(matches!(
            lifecycle.on(paused()),
            Some(Event::Interrupted(CaptureError::Os(_)))
        ));
        assert!(lifecycle.on(paused()).is_none());
        assert!(matches!(
            lifecycle.on(Change::Streaming),
            Some(Event::Resumed)
        ));
        assert!(lifecycle.on(Change::Streaming).is_none());
    }

    #[test]
    fn a_stream_that_disconnects_after_running_was_stopped_by_the_user() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.on(Change::Streaming);

        assert!(matches!(
            lifecycle.on(Change::Disconnected),
            Some(Event::Ended(EndReason::StoppedByUser))
        ));
        assert!(lifecycle.has_ended());
        assert!(lifecycle.on(Change::Streaming).is_none());
        assert!(lifecycle.on(Change::Disconnected).is_none());
    }

    #[test]
    fn a_stream_that_never_ran_ends_because_its_target_is_gone() {
        let mut lifecycle = Lifecycle::default();

        assert!(matches!(
            lifecycle.on(Change::Disconnected),
            Some(Event::Ended(EndReason::Failed(CaptureError::TargetGone)))
        ));
    }

    #[test]
    fn a_failure_ends_the_session_with_its_reason() {
        let mut lifecycle = Lifecycle::default();
        lifecycle.on(Change::Streaming);
        let ended = lifecycle.on(Change::Failed(stream_error("connection reset")));

        assert!(matches!(
            ended,
            Some(Event::Ended(EndReason::Failed(CaptureError::Os(text)))) if text == "connection reset"
        ));
        assert!(!Lifecycle::default().has_ended());
    }

    #[test]
    fn a_missing_target_node_is_a_target_that_is_gone() {
        assert_eq!(
            stream_error("no target node available"),
            CaptureError::TargetGone
        );
        assert_eq!(stream_error("Target not found"), CaptureError::TargetGone);
    }
}
