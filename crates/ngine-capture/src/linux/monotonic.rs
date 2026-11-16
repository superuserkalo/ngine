use rustix::time::{ClockId, clock_gettime};

use super::clock;
use crate::time::Timestamp;

pub(super) fn now() -> Option<Timestamp> {
    let reading = clock_gettime(ClockId::Monotonic);
    clock::from_timespec(reading.tv_sec, reading.tv_nsec)
}
