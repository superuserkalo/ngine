use std::time::{Duration, Instant};

use ngine_capture::{AudioBuffer, AudioSource};

/// The loudest sample of each source since the previous report, in
/// ten-thousandths of full scale: 0 is silence and [`Peaks::FULL_SCALE`] the
/// loudest a sample can be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Peaks {
    pub system: u16,
    pub microphone: u16,
}

impl Peaks {
    pub const FULL_SCALE: u16 = 10_000;
}

/// How often the peaks are reported at most: twenty times a second.
pub const REPORT_EVERY: Duration = Duration::from_millis(50);

fn loudest(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0_f32, |loudest, sample| loudest.max(sample.abs()))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the peak is clamped to 0..=1 first, so the product fits a u16"
)]
fn scaled(peak: f32) -> u16 {
    if peak.is_nan() {
        return 0;
    }
    (peak.clamp(0.0, 1.0) * f32::from(Peaks::FULL_SCALE)).round() as u16
}

#[derive(Debug, Default)]
pub(crate) struct PeakMeter {
    system: f32,
    microphone: f32,
    shown: Peaks,
    reported_at: Option<Instant>,
}

impl PeakMeter {
    pub(crate) fn take(&mut self, buffer: &AudioBuffer) {
        let peak = loudest(buffer.samples());
        let held = match buffer.source() {
            AudioSource::Microphone => &mut self.microphone,
            _ => &mut self.system,
        };
        *held = held.max(peak);
    }

    pub(crate) fn report(&mut self, now: Instant) -> Peaks {
        let due = self
            .reported_at
            .is_none_or(|at| now.saturating_duration_since(at) >= REPORT_EVERY);
        if due {
            self.shown = Peaks {
                system: scaled(self.system),
                microphone: scaled(self.microphone),
            };
            self.system = 0.0;
            self.microphone = 0.0;
            self.reported_at = Some(now);
        }
        self.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ngine_capture::Timestamp;

    fn buffer(source: AudioSource, samples: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(source, Timestamp::from_nanos(0), 48_000, 2, samples)
            .expect("whole stereo frames")
    }

    #[test]
    fn the_peak_is_the_loudest_sample_either_way_from_zero() {
        assert!((loudest(&[0.1, -0.7, 0.3, 0.2]) - 0.7).abs() < f32::EPSILON);
        assert!(loudest(&[]).abs() < f32::EPSILON);
        assert_eq!(scaled(0.5), 5_000);
        assert_eq!(scaled(1.8), Peaks::FULL_SCALE);
        assert_eq!(scaled(f32::NAN), 0);
    }

    #[test]
    fn each_source_keeps_its_own_peak_until_the_next_report() {
        let start = Instant::now();
        let mut meter = PeakMeter::default();
        meter.take(&buffer(AudioSource::System, vec![0.25, -0.5]));
        meter.take(&buffer(AudioSource::System, vec![0.1, 0.1]));
        meter.take(&buffer(AudioSource::Microphone, vec![0.05, 0.0]));
        assert_eq!(
            meter.report(start),
            Peaks {
                system: 5_000,
                microphone: 500
            }
        );
    }

    #[test]
    fn reports_change_at_most_twenty_times_a_second_and_fall_to_silence() {
        let start = Instant::now();
        let mut meter = PeakMeter::default();
        meter.take(&buffer(AudioSource::System, vec![0.5, 0.5]));
        let first = meter.report(start);
        meter.take(&buffer(AudioSource::System, vec![0.9, 0.9]));
        assert_eq!(meter.report(start + Duration::from_millis(20)), first);
        assert_eq!(
            meter.report(start + REPORT_EVERY).system,
            9_000,
            "the louder sound shows once the interval has passed"
        );
        assert_eq!(
            meter.report(start + 2 * REPORT_EVERY),
            Peaks::default(),
            "no sound since the last report reads as silence"
        );
    }
}
