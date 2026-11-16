use std::time::Duration;

/// A point on the clock of one capture session.
///
/// Video frames and audio buffers of one session share that clock. The origin
/// is unspecified, so only compare timestamps that came from the same session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(u64);

impl Timestamp {
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    pub const fn checked_duration_since(self, earlier: Self) -> Option<Duration> {
        match self.0.checked_sub(earlier.0) {
            Some(nanos) => Some(Duration::from_nanos(nanos)),
            None => None,
        }
    }

    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }

    pub fn checked_add(self, duration: Duration) -> Option<Self> {
        let nanos = u64::try_from(duration.as_nanos()).ok()?;
        self.0.checked_add(nanos).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_timestamp_is_a_duration_after_an_earlier_one() {
        let earlier = Timestamp::from_nanos(1_000);
        let later = earlier.checked_add(Duration::from_micros(5)).unwrap();

        assert_eq!(later.as_nanos(), 6_000);
        assert_eq!(
            later.checked_duration_since(earlier),
            Some(Duration::from_micros(5))
        );
        assert_eq!(earlier.checked_duration_since(later), None);
        assert_eq!(earlier.saturating_duration_since(later), Duration::ZERO);
    }
}
