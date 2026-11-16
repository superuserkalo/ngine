use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::time::Timestamp;

const NOT_LINKED_YET: u64 = 0;

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

pub(crate) struct PerformanceCounterReadThroughInstant {
    began: Instant,
    counter_when_began_by_the_freshest_stamp_seen: AtomicU64,
}

impl PerformanceCounterReadThroughInstant {
    pub(crate) fn new() -> Self {
        Self {
            began: Instant::now(),
            counter_when_began_by_the_freshest_stamp_seen: AtomicU64::new(NOT_LINKED_YET),
        }
    }

    pub(crate) fn observe(&self, stamp: Timestamp) {
        self.observe_after(stamp, self.began.elapsed());
    }

    pub(crate) fn now(&self) -> Option<Timestamp> {
        self.after(self.began.elapsed())
    }

    fn observe_after(&self, stamp: Timestamp, since_beginning: Duration) {
        if let Some(when_began) = stamp.as_nanos().checked_sub(nanos(since_beginning)) {
            self.counter_when_began_by_the_freshest_stamp_seen
                .fetch_max(when_began, Ordering::AcqRel);
        }
    }

    fn after(&self, since_beginning: Duration) -> Option<Timestamp> {
        let when_began = self
            .counter_when_began_by_the_freshest_stamp_seen
            .load(Ordering::Acquire);
        (when_began != NOT_LINKED_YET)
            .then(|| when_began.checked_add(nanos(since_beginning)))
            .flatten()
            .map(Timestamp::from_nanos)
    }
}

#[derive(Default)]
pub(crate) struct BufferTimeline {
    end_of_last: Option<Timestamp>,
}

impl BufferTimeline {
    pub(crate) fn stamp(
        &mut self,
        from_device: Option<Timestamp>,
        length: Duration,
        now: Option<Timestamp>,
    ) -> Option<Timestamp> {
        let ending_now = || {
            now.and_then(|now| now.as_nanos().checked_sub(nanos(length)))
                .map(Timestamp::from_nanos)
        };
        let start = from_device.or(self.end_of_last).or_else(ending_now)?;
        self.end_of_last = start.checked_add(length);
        Some(start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn a_clock_that_saw_no_stamp_cannot_tell_the_time() {
        let clock = PerformanceCounterReadThroughInstant::new();

        assert_eq!(clock.after(SECOND), None);
        assert_eq!(clock.now(), None);
    }

    #[test]
    fn the_time_is_the_seen_stamp_plus_what_passed_since() {
        let clock = PerformanceCounterReadThroughInstant::new();

        clock.observe_after(Timestamp::from_nanos(50_000_000_000), SECOND);

        assert_eq!(
            clock.after(SECOND * 3),
            Some(Timestamp::from_nanos(52_000_000_000))
        );
    }

    #[test]
    fn a_stamp_that_arrived_late_does_not_turn_the_clock_back() {
        let clock = PerformanceCounterReadThroughInstant::new();
        let fresh = Timestamp::from_nanos(50_000_000_000);
        let seen_a_tenth_of_a_second_late = Timestamp::from_nanos(50_900_000_000);

        clock.observe_after(fresh, SECOND);
        clock.observe_after(seen_a_tenth_of_a_second_late, SECOND * 2);

        assert_eq!(
            clock.after(SECOND * 2),
            Some(Timestamp::from_nanos(51_000_000_000))
        );
    }

    #[test]
    fn a_stamp_older_than_the_clock_could_explain_is_ignored() {
        let clock = PerformanceCounterReadThroughInstant::new();

        clock.observe_after(Timestamp::from_nanos(5), SECOND);

        assert_eq!(clock.after(SECOND), None);
    }

    #[test]
    fn observing_a_stamp_lets_the_clock_tell_the_time() {
        let clock = PerformanceCounterReadThroughInstant::new();
        let stamp = Timestamp::from_nanos(90_000_000_000);

        clock.observe(stamp);

        assert!(clock.now().is_some_and(|now| now >= stamp));
    }

    #[test]
    fn a_buffer_keeps_the_stamp_of_its_device() {
        let mut timeline = BufferTimeline::default();
        let stamped = Timestamp::from_nanos(7_000);

        assert_eq!(
            timeline.stamp(Some(stamped), SECOND, Some(Timestamp::from_nanos(1))),
            Some(stamped)
        );
    }

    #[test]
    fn an_unstamped_buffer_continues_where_the_last_one_ended() {
        let mut timeline = BufferTimeline::default();
        let first = Timestamp::from_nanos(4_000_000_000);

        timeline.stamp(Some(first), SECOND, None);

        assert_eq!(
            timeline.stamp(None, SECOND, Some(Timestamp::from_nanos(1))),
            Some(Timestamp::from_nanos(5_000_000_000))
        );
        assert_eq!(
            timeline.stamp(None, SECOND, None),
            Some(Timestamp::from_nanos(6_000_000_000))
        );
    }

    #[test]
    fn the_first_unstamped_buffer_ends_at_the_current_time() {
        let mut timeline = BufferTimeline::default();
        let now = Timestamp::from_nanos(9_000_000_000);

        assert_eq!(
            timeline.stamp(None, SECOND, Some(now)),
            Some(Timestamp::from_nanos(8_000_000_000))
        );
        assert_eq!(BufferTimeline::default().stamp(None, SECOND, None), None);
    }
}
