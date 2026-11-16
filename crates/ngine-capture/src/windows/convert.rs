use std::time::Duration;

use crate::error::{CaptureError, Permission};
use crate::frame::Size;
use crate::time::Timestamp;

const NANOS_PER_COUNTER_UNIT: u64 = 100;
const BYTES_PER_SAMPLE: usize = size_of::<f32>();
const ACCESS_DENIED: i32 = 0x8007_0005_u32.cast_signed();
const ELEMENT_NOT_FOUND: i32 = 0x8007_0490_u32.cast_signed();
const AUDIO_DEVICE_INVALIDATED: i32 = 0x8889_0004_u32.cast_signed();
const AUDIO_SERVICE_NOT_RUNNING: i32 = 0x8889_0010_u32.cast_signed();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Input {
    Picture,
    SystemSound,
    Microphone,
}

pub(crate) fn timestamp(hundreds_of_nanos: i64) -> Option<Timestamp> {
    u64::try_from(hundreds_of_nanos)
        .ok()?
        .checked_mul(NANOS_PER_COUNTER_UNIT)
        .map(Timestamp::from_nanos)
}

pub(crate) fn device_timestamp(hundreds_of_nanos: u64, device_vouches: bool) -> Option<Timestamp> {
    let stamped = device_vouches && hundreds_of_nanos != 0;
    stamped
        .then(|| i64::try_from(hundreds_of_nanos).ok().and_then(timestamp))
        .flatten()
}

pub(crate) fn samples(little_endian: &[u8]) -> Vec<f32> {
    little_endian
        .as_chunks::<BYTES_PER_SAMPLE>()
        .0
        .iter()
        .map(|sample| f32::from_le_bytes(*sample))
        .collect()
}

pub(crate) fn length(frames: usize, sample_rate: u32) -> Duration {
    let frames = u64::try_from(frames).unwrap_or(u64::MAX);
    Duration::from_nanos(frames.saturating_mul(1_000_000_000) / u64::from(sample_rate.max(1)))
}

pub(crate) fn time_between_frames(frame_rate: u32) -> Option<Duration> {
    (frame_rate != 0).then(|| Duration::from_secs(1) / frame_rate)
}

pub(crate) fn size(width: i32, height: i32) -> Option<Size> {
    let width = u32::try_from(width).ok().filter(|width| *width > 0)?;
    let height = u32::try_from(height).ok().filter(|height| *height > 0)?;
    Some(Size::new(width, height))
}

pub(crate) fn error_from_code(code: i32, message: &str, input: Input) -> CaptureError {
    match (code, input) {
        (ACCESS_DENIED, Input::Microphone) => {
            CaptureError::PermissionDenied(Permission::Microphone)
        }
        (ACCESS_DENIED, Input::Picture) => {
            CaptureError::PermissionDenied(Permission::ScreenRecording)
        }
        (ELEMENT_NOT_FOUND | AUDIO_DEVICE_INVALIDATED | AUDIO_SERVICE_NOT_RUNNING, _) => {
            CaptureError::Unavailable(
                match input {
                    Input::Microphone => "the microphone that was asked for is not connected",
                    Input::SystemSound => "no sound output device is connected",
                    Input::Picture => "the capture source is not connected",
                }
                .to_owned(),
            )
        }
        _ => CaptureError::Os(format!("{message} (0x{code:08X})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_time_becomes_nanoseconds() {
        assert_eq!(timestamp(15), Some(Timestamp::from_nanos(1_500)));
        assert_eq!(timestamp(-1), None);
        assert_eq!(timestamp(i64::MAX), None);
    }

    #[test]
    fn only_a_stamp_the_device_vouches_for_is_used() {
        assert_eq!(
            device_timestamp(15, true),
            Some(Timestamp::from_nanos(1_500))
        );
        assert_eq!(device_timestamp(15, false), None);
        assert_eq!(device_timestamp(0, true), None);
    }

    #[test]
    fn bytes_become_samples_and_a_cut_off_sample_is_left_out() {
        let mut bytes = Vec::new();
        bytes.extend(0.5_f32.to_le_bytes());
        bytes.extend((-1.0_f32).to_le_bytes());
        bytes.extend([1, 2, 3]);

        assert_eq!(samples(&bytes), [0.5, -1.0]);
    }

    #[test]
    fn frames_last_as_long_as_the_sample_rate_says() {
        assert_eq!(length(480, 48_000), Duration::from_millis(10));
        assert_eq!(length(480, 0), Duration::from_secs(480));
    }

    #[test]
    fn a_frame_rate_sets_the_time_between_frames() {
        assert_eq!(time_between_frames(50), Some(Duration::from_millis(20)));
        assert_eq!(time_between_frames(0), None);
    }

    #[test]
    fn a_size_needs_both_sides_above_zero() {
        assert_eq!(size(800, 600), Some(Size::new(800, 600)));
        assert_eq!(size(0, 600), None);
        assert_eq!(size(800, -1), None);
    }

    #[test]
    fn access_denied_names_the_input_that_was_refused() {
        assert_eq!(
            error_from_code(ACCESS_DENIED, "denied", Input::Microphone),
            CaptureError::PermissionDenied(Permission::Microphone)
        );
        assert_eq!(
            error_from_code(ACCESS_DENIED, "denied", Input::Picture),
            CaptureError::PermissionDenied(Permission::ScreenRecording)
        );
        assert!(matches!(
            error_from_code(ACCESS_DENIED, "denied", Input::SystemSound),
            CaptureError::Os(_)
        ));
    }

    #[test]
    fn a_missing_or_unplugged_device_is_unavailable() {
        for code in [
            ELEMENT_NOT_FOUND,
            AUDIO_DEVICE_INVALIDATED,
            AUDIO_SERVICE_NOT_RUNNING,
        ] {
            for input in [Input::Microphone, Input::SystemSound, Input::Picture] {
                assert!(matches!(
                    error_from_code(code, "gone", input),
                    CaptureError::Unavailable(_)
                ));
            }
        }
    }

    #[test]
    fn any_other_failure_keeps_its_text_and_code() {
        let unspecified = 0x8000_4005_u32.cast_signed();

        assert_eq!(
            error_from_code(unspecified, "Unspecified error", Input::Picture),
            CaptureError::Os("Unspecified error (0x80004005)".to_owned())
        );
    }
}
