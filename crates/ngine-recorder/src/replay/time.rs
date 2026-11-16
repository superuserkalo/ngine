use std::ops::{Add, Sub};
use std::time::Duration;

/// A time or a length of time in the one time base every replay packet uses:
/// `1 / Ticks::PER_SECOND` of a second.
///
/// Arithmetic saturates instead of overflowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ticks(pub i64);

const NANOS_PER_SECOND: u128 = 1_000_000_000;

impl Ticks {
    pub const PER_SECOND: i64 = 705_600_000;
    pub const ZERO: Self = Self(0);

    pub const fn from_secs(secs: i64) -> Self {
        Self(secs.saturating_mul(Self::PER_SECOND))
    }

    pub const fn from_millis(millis: i64) -> Self {
        Self(millis.saturating_mul(Self::PER_SECOND / 1000))
    }

    /// Never shorter than `duration`: a fraction of a tick rounds up.
    pub fn from_duration_rounded_up(duration: Duration) -> Self {
        let ticks = duration
            .as_nanos()
            .saturating_mul(u128::from(Self::PER_SECOND.unsigned_abs()))
            .div_ceil(NANOS_PER_SECOND);
        Self(i64::try_from(ticks).unwrap_or(i64::MAX))
    }

    /// Never longer than the ticks: a fraction of a nanosecond rounds down,
    /// and a negative value gives zero.
    pub fn to_duration(self) -> Duration {
        let ticks = u128::from(self.0.max(0).unsigned_abs());
        let nanos = ticks * NANOS_PER_SECOND / u128::from(Self::PER_SECOND.unsigned_abs());
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }
}

impl Add for Ticks {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl Sub for Ticks {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}
