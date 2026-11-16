use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rune::LocalTime;

const BYTES_PER_MB: u64 = 1_000_000;
const MB_PER_GB: u64 = 1000;

fn rounded_seconds(length: Duration) -> u64 {
    (length.as_millis().saturating_add(500) / 1000)
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn length(length: Duration) -> String {
    let seconds = rounded_seconds(length);
    match (seconds / 60, seconds % 60) {
        (0, seconds) => format!("{seconds} s"),
        (minutes, 0) => format!("{minutes} min"),
        (minutes, seconds) => format!("{minutes} min {seconds} s"),
    }
}

pub fn timer(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours}:{minutes:02}:{seconds:02}")
    }
}

pub fn megabytes(megabytes: u64) -> String {
    if megabytes < MB_PER_GB {
        format!("{megabytes} MB")
    } else {
        let tenths = megabytes / (MB_PER_GB / 10);
        format!("{}.{} GB", tenths / 10, tenths % 10)
    }
}

pub fn approximate_megabytes(bytes: u64) -> u64 {
    let megabytes = bytes / BYTES_PER_MB;
    let step = if megabytes < 100 { 5 } else { 10 };
    ((megabytes + step / 2) / step * step).max(step)
}

pub fn size(bytes: u64) -> String {
    if bytes < BYTES_PER_MB {
        format!("{} KB", (bytes / 1000).max(1))
    } else {
        megabytes((bytes + BYTES_PER_MB / 2) / BYTES_PER_MB)
    }
}

pub fn position(at: Duration) -> String {
    let tenths = at.as_millis() / 100;
    let (minutes, seconds, tenth) = (tenths / 600, tenths / 10 % 60, tenths % 10);
    format!("{minutes:02}:{seconds:02}.{tenth}")
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn local(time: SystemTime) -> LocalTime {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    LocalTime::at(i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

pub fn date(time: &LocalTime) -> String {
    let month = MONTHS[(time.month.clamp(1, 12) - 1) as usize];
    format!(
        "{} {month} {}, {:02}:{:02}",
        time.day, time.year, time.hour, time.minute
    )
}

/// A name for a new recording or clip that sorts by time and is allowed in a
/// file name on every system.
pub fn stamped(kind: &str, time: &LocalTime) -> String {
    format!(
        "{kind} {:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_as_plain_words() {
        assert_eq!(length(Duration::from_millis(30_800)), "31 s");
        assert_eq!(length(Duration::from_secs(60)), "1 min");
        assert_eq!(length(Duration::from_secs(123)), "2 min 3 s");
        assert_eq!(length(Duration::from_mins(30)), "30 min");
    }

    #[test]
    fn the_timer_adds_hours_only_when_needed() {
        assert_eq!(timer(Duration::from_secs(42)), "00:42");
        assert_eq!(timer(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn sizes_are_rounded_to_numbers_people_remember() {
        assert_eq!(megabytes(approximate_megabytes(89_880_000)), "90 MB");
        assert_eq!(megabytes(approximate_megabytes(1_797_600_000)), "1.8 GB");
        assert_eq!(megabytes(approximate_megabytes(1_000_000)), "5 MB");
    }

    #[test]
    fn sizes_positions_and_dates_read_plainly() {
        assert_eq!(size(310_400_000), "310 MB");
        assert_eq!(size(2_340_000_000), "2.3 GB");
        assert_eq!(size(48_000), "48 KB");
        assert_eq!(position(Duration::from_millis(125_340)), "02:05.3");
        let time = LocalTime::at_offset(1_794_834_202_000, 3_600);
        assert_eq!(date(&time), "16 Nov 2026, 14:03");
        assert_eq!(stamped("Recording", &time), "Recording 2026-11-16 14.03.22");
    }
}
