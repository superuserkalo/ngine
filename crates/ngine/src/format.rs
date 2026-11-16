use std::time::Duration;

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
}
