use screencapturekit::cm::CMTime;
use screencapturekit::error::{SCError, SCStreamErrorCode};

use crate::error::{CaptureError, Permission};
use crate::time::Timestamp;

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const STOPPED_BY_THE_SYSTEM: &str =
    "macOS stopped the capture. It does this when the startup disk is nearly full.";

pub(super) fn timestamp(time: CMTime) -> Option<Timestamp> {
    if !time.is_numeric() || time.timescale <= 0 {
        return None;
    }
    let nanos = i128::from(time.value) * NANOS_PER_SECOND / i128::from(time.timescale);
    u64::try_from(nanos).ok().map(Timestamp::from_nanos)
}

pub(super) fn whole_number(value: f64) -> Option<u32> {
    let rounded = value.round();
    let fits = rounded.is_finite() && rounded >= 0.0 && rounded <= f64::from(u32::MAX);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "false positive: clippy does not see that the line above keeps the value a whole number inside the range of u32"
    )]
    fits.then_some(rounded as u32)
}

pub(super) fn error_from_text(text: &str) -> CaptureError {
    let lowered = text.to_lowercase();
    let about_microphone = lowered.contains("microphone");
    let refused = lowered.contains("tcc") || lowered.contains("declined");
    if about_microphone {
        CaptureError::PermissionDenied(Permission::Microphone)
    } else if refused {
        CaptureError::PermissionDenied(Permission::ScreenRecording)
    } else {
        CaptureError::Os(text.to_owned())
    }
}

pub(super) fn error_from(error: &SCError) -> CaptureError {
    match error.stream_error_code() {
        Some(SCStreamErrorCode::UserDeclined) => {
            CaptureError::PermissionDenied(Permission::ScreenRecording)
        }
        Some(SCStreamErrorCode::FailedToStartMicrophoneCapture) => {
            CaptureError::PermissionDenied(Permission::Microphone)
        }
        Some(SCStreamErrorCode::SystemStoppedStream) => {
            CaptureError::Os(STOPPED_BY_THE_SYSTEM.to_owned())
        }
        _ => error_from_text(&error.to_string()),
    }
}

pub(super) fn was_stopped_by_the_user(error: &SCError) -> bool {
    error.stream_error_code() == Some(SCStreamErrorCode::UserStopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_media_time_becomes_nanoseconds() {
        assert_eq!(
            timestamp(CMTime::new(3, 2)),
            Some(Timestamp::from_nanos(1_500_000_000))
        );
        assert_eq!(
            timestamp(CMTime::new(123_456_789_012, 1_000_000_000)),
            Some(Timestamp::from_nanos(123_456_789_012))
        );
    }

    #[test]
    fn an_invalid_or_negative_media_time_is_no_timestamp() {
        assert_eq!(timestamp(CMTime::INVALID), None);
        assert_eq!(timestamp(CMTime::new(-1, 600)), None);
    }

    #[test]
    fn the_refusal_text_of_screencapturekit_is_a_missing_screen_recording_permission() {
        let refusal = "The user declined TCCs for application, window, display capture";

        assert_eq!(
            error_from_text(refusal),
            CaptureError::PermissionDenied(Permission::ScreenRecording)
        );
        assert_eq!(
            error_from(&SCError::NoShareableContent(refusal.to_owned())),
            CaptureError::PermissionDenied(Permission::ScreenRecording)
        );
        assert_eq!(
            error_from(&SCError::from_stream_error_code(
                SCStreamErrorCode::UserDeclined
            )),
            CaptureError::PermissionDenied(Permission::ScreenRecording)
        );
    }

    #[test]
    fn a_microphone_that_did_not_start_is_a_missing_microphone_permission() {
        assert_eq!(
            error_from(&SCError::from_stream_error_code(
                SCStreamErrorCode::FailedToStartMicrophoneCapture
            )),
            CaptureError::PermissionDenied(Permission::Microphone)
        );
        assert_eq!(
            error_from(&SCError::CaptureStartFailed(
                "Failed to start microphone capture".to_owned()
            )),
            CaptureError::PermissionDenied(Permission::Microphone)
        );
    }

    #[test]
    fn a_stream_that_the_system_stopped_names_the_full_disk() {
        let stopped = SCError::from_stream_error_code(SCStreamErrorCode::SystemStoppedStream);

        assert_eq!(
            error_from(&stopped),
            CaptureError::Os(STOPPED_BY_THE_SYSTEM.to_owned())
        );
        assert!(!was_stopped_by_the_user(&stopped));
    }

    #[test]
    fn any_other_failure_keeps_its_text() {
        assert_eq!(
            error_from_text("the display went to sleep"),
            CaptureError::Os("the display went to sleep".to_owned())
        );
    }

    #[test]
    fn only_a_whole_number_inside_the_range_converts() {
        assert_eq!(whole_number(47_999.6), Some(48_000));
        assert_eq!(whole_number(-1.0), None);
        assert_eq!(whole_number(f64::NAN), None);
        assert_eq!(whole_number(1e12), None);
    }
}
