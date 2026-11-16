use crate::time::Timestamp;

const NANOS_PER_SECOND: i128 = 1_000_000_000;

pub(super) fn from_timespec(seconds: i64, nanos: i64) -> Option<Timestamp> {
    let total = i128::from(seconds) * NANOS_PER_SECOND + i128::from(nanos);
    u64::try_from(total).ok().map(Timestamp::from_nanos)
}

pub(super) fn video_timestamp(presentation_nanos: Option<i64>, now: Timestamp) -> Timestamp {
    presentation_nanos
        .filter(|nanos| *nanos > 0)
        .and_then(|nanos| u64::try_from(nanos).ok())
        .map_or(now, Timestamp::from_nanos)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GraphTime {
    pub(super) now_nanos: i64,
    pub(super) delay_ticks: i64,
    pub(super) tick_numerator: u32,
    pub(super) tick_denominator: u32,
}

pub(super) fn audio_timestamp(graph: Option<GraphTime>, now: Timestamp) -> Timestamp {
    let Some(graph) = graph.filter(|graph| graph.now_nanos > 0 && graph.tick_denominator != 0)
    else {
        return now;
    };
    let delay_nanos =
        i128::from(graph.delay_ticks) * i128::from(graph.tick_numerator) * NANOS_PER_SECOND
            / i128::from(graph.tick_denominator);
    let captured = i128::from(graph.now_nanos) - delay_nanos.max(0);
    u64::try_from(captured).map_or(now, Timestamp::from_nanos)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FrameLimiter {
    shortest_gap_nanos: u64,
    last: Option<Timestamp>,
}

impl FrameLimiter {
    pub(super) fn new(frames_per_second: u32) -> Self {
        let whole_gap = 1_000_000_000 / u64::from(frames_per_second.max(1));
        Self {
            shortest_gap_nanos: whole_gap - whole_gap / 10,
            last: None,
        }
    }

    pub(super) fn accepts(&mut self, timestamp: Timestamp) -> bool {
        let too_soon = self.last.is_some_and(|last| {
            timestamp >= last && timestamp.as_nanos() - last.as_nanos() < self.shortest_gap_nanos
        });
        if !too_soon {
            self.last = Some(timestamp);
        }
        !too_soon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monotonic_reading_becomes_nanoseconds() {
        assert_eq!(
            from_timespec(12, 345),
            Some(Timestamp::from_nanos(12_000_000_345))
        );
        assert_eq!(from_timespec(-1, 0), None);
    }

    #[test]
    fn a_frame_keeps_the_time_the_compositor_stamped_and_falls_back_to_now() {
        let now = Timestamp::from_nanos(9_000);

        assert_eq!(
            video_timestamp(Some(7_000), now),
            Timestamp::from_nanos(7_000)
        );
        assert_eq!(video_timestamp(Some(0), now), now);
        assert_eq!(video_timestamp(Some(-1), now), now);
        assert_eq!(video_timestamp(None, now), now);
    }

    #[test]
    fn an_audio_buffer_is_stamped_at_the_graph_time_less_the_capture_delay() {
        let now = Timestamp::from_nanos(5);
        let graph = GraphTime {
            now_nanos: 2_000_000_000,
            delay_ticks: 480,
            tick_numerator: 1,
            tick_denominator: 48_000,
        };

        assert_eq!(
            audio_timestamp(Some(graph), now),
            Timestamp::from_nanos(1_990_000_000)
        );
    }

    #[test]
    fn audio_without_a_usable_graph_time_is_stamped_now() {
        let now = Timestamp::from_nanos(5);
        let unset = GraphTime {
            now_nanos: 0,
            delay_ticks: 0,
            tick_numerator: 1,
            tick_denominator: 48_000,
        };
        let no_rate = GraphTime {
            now_nanos: 100,
            tick_denominator: 0,
            ..unset
        };
        let delay_before_the_epoch = GraphTime {
            now_nanos: 100,
            delay_ticks: 48_000,
            ..unset
        };

        assert_eq!(audio_timestamp(None, now), now);
        assert_eq!(audio_timestamp(Some(unset), now), now);
        assert_eq!(audio_timestamp(Some(no_rate), now), now);
        assert_eq!(audio_timestamp(Some(delay_before_the_epoch), now), now);
    }

    #[test]
    fn frames_faster_than_the_asked_rate_are_refused() {
        let mut limiter = FrameLimiter::new(50);
        let at = |millis: u64| Timestamp::from_nanos(millis * 1_000_000);

        assert!(limiter.accepts(at(0)));
        assert!(!limiter.accepts(at(5)));
        assert!(limiter.accepts(at(19)));
        assert!(limiter.accepts(at(39)));
        assert!(limiter.accepts(at(10)));
    }
}
