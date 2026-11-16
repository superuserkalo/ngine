use crate::engine::Levels;

pub const QUIETEST_DB: f32 = -60.0;
const FALL_PER_SECOND: f32 = 0.9;
const LONGEST_STEP_SECONDS: f32 = 0.25;
const STILL: f32 = 0.001;

/// Where a peak sits on a meter, from 0 (at or below [`QUIETEST_DB`]) to 1
/// (full scale), even in decibels so quiet speech still shows.
pub fn position(peak: u16) -> f32 {
    let amplitude = f32::from(peak.min(Levels::FULL_SCALE)) / f32::from(Levels::FULL_SCALE);
    if amplitude <= 0.0 {
        return 0.0;
    }
    let decibels = 20.0 * amplitude.log10();
    (1.0 - decibels / QUIETEST_DB).clamp(0.0, 1.0)
}

/// What the level meters show: a new peak at once, a quieter one only as
/// fast as the bar falls.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Meters {
    pub system: f32,
    pub microphone: f32,
    updated: Option<f64>,
}

fn settle(shown: f32, target: f32, seconds: f32) -> f32 {
    if target >= shown {
        target
    } else {
        (shown - FALL_PER_SECOND * seconds).max(target)
    }
}

impl Meters {
    /// Moves the bars towards `levels` at `clock` seconds. Returns whether
    /// they moved.
    pub fn follow(&mut self, levels: Option<Levels>, clock: f64) -> bool {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a step between two frames is a fraction of a second"
        )]
        let seconds = self
            .updated
            .map_or(0.0, |before| (clock - before).max(0.0) as f32)
            .min(LONGEST_STEP_SECONDS);
        self.updated = Some(clock);
        let levels = levels.unwrap_or_default();
        let before = *self;
        self.system = settle(self.system, position(levels.system), seconds);
        self.microphone = settle(self.microphone, position(levels.microphone), seconds);
        (self.system - before.system).abs() > STILL
            || (self.microphone - before.microphone).abs() > STILL
    }

    /// Whether a bar is still falling.
    pub fn moving(&self) -> bool {
        self.system > STILL || self.microphone > STILL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_empty_full_scale_is_full_and_minus_twenty_decibels_is_two_thirds() {
        assert!(position(0).abs() < f32::EPSILON);
        assert!((position(Levels::FULL_SCALE) - 1.0).abs() < 1e-6);
        assert!((position(1_000) - 2.0 / 3.0).abs() < 1e-4);
        assert!(position(5).abs() < f32::EPSILON);
        assert!((position(u16::MAX) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_peak_shows_at_once_and_falls_slowly() {
        let mut meters = Meters::default();
        let loud = Levels {
            system: Levels::FULL_SCALE,
            microphone: 1_000,
        };
        assert!(meters.follow(Some(loud), 0.0));
        assert!((meters.system - 1.0).abs() < 1e-6);
        assert!(meters.follow(Some(Levels::default()), 0.1));
        assert!((meters.system - 0.91).abs() < 1e-4);
        meters.follow(Some(Levels::default()), 0.2);
        assert!((meters.system - 0.82).abs() < 1e-4);
        assert!(meters.moving());
        for step in 3..40 {
            meters.follow(None, f64::from(step) * 0.1);
        }
        assert!(!meters.moving());
        assert!(!meters.follow(None, 5.0));
    }

    #[test]
    fn a_long_pause_between_frames_falls_no_further_than_a_quarter_second() {
        let mut meters = Meters::default();
        meters.follow(
            Some(Levels {
                system: Levels::FULL_SCALE,
                microphone: 0,
            }),
            0.0,
        );
        meters.follow(None, 10.0);
        assert!((meters.system - (1.0 - FALL_PER_SECOND * LONGEST_STEP_SECONDS)).abs() < 1e-4);
    }
}
