use std::time::Duration;

use ngine_media::{Packet, Rational};

use crate::replay::{self, StreamIndex, Ticks};

pub(crate) const VIDEO_TIME_BASE: Rational = Rational::new(1, 90_000);
pub(crate) const VIDEO_UNITS_PER_SECOND: i64 = 90_000;
pub(crate) const AUDIO_FRAMES_PER_SECOND: i64 = 48_000;
const NANOS_PER_SECOND: i64 = 1_000_000_000;
const TICKS_PER_VIDEO_UNIT: i64 = Ticks::PER_SECOND / VIDEO_UNITS_PER_SECOND;
const TICKS_PER_AUDIO_FRAME: i64 = Ticks::PER_SECOND / AUDIO_FRAMES_PER_SECOND;

fn scale(value: i64, numerator: i64, denominator: i64) -> i64 {
    let scaled = i128::from(value) * i128::from(numerator) / i128::from(denominator);
    i64::try_from(scaled).unwrap_or(if scaled < 0 { i64::MIN } else { i64::MAX })
}

pub(crate) fn video_units_from_nanos(nanos: i64) -> i64 {
    scale(nanos, VIDEO_UNITS_PER_SECOND, NANOS_PER_SECOND)
}

pub(crate) fn audio_frames_from_nanos(nanos: i64) -> i64 {
    scale(nanos, AUDIO_FRAMES_PER_SECOND, NANOS_PER_SECOND)
}

pub(crate) fn audio_frames_from_video_units(units: i64) -> i64 {
    scale(units, AUDIO_FRAMES_PER_SECOND, VIDEO_UNITS_PER_SECOND)
}

pub(crate) fn micros_from_video_units(units: i64) -> i64 {
    scale(units, 1_000_000, VIDEO_UNITS_PER_SECOND)
}

pub(crate) fn duration_from_nanos(nanos: i64) -> Duration {
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(0))
}

pub(crate) fn duration_from_video_units(units: i64) -> Duration {
    Duration::from_micros(u64::try_from(micros_from_video_units(units)).unwrap_or(0))
}

pub(crate) fn video_units_from_ticks(ticks: Ticks) -> i64 {
    ticks.0.div_euclid(TICKS_PER_VIDEO_UNIT)
}

pub(crate) fn micros_from_ticks(ticks: Ticks) -> i64 {
    scale(ticks.0, 1_000_000, Ticks::PER_SECOND)
}

fn ticks_per_unit(stream: usize) -> i64 {
    if stream == 0 {
        TICKS_PER_VIDEO_UNIT
    } else {
        TICKS_PER_AUDIO_FRAME
    }
}

pub(crate) fn replay_packet(packet: &Packet) -> Option<replay::Packet> {
    let stream = match u8::try_from(packet.stream).ok()? {
        0 => StreamIndex::VIDEO,
        track => StreamIndex::audio(track - 1)?,
    };
    let unit = ticks_per_unit(packet.stream);
    let pts = packet.pts?;
    Some(replay::Packet {
        stream,
        pts: Ticks(pts.saturating_mul(unit)),
        dts: Ticks(packet.dts.unwrap_or(pts).saturating_mul(unit)),
        duration: Ticks(packet.duration.saturating_mul(unit)),
        keyframe: packet.keyframe,
        data: packet.data.as_slice().into(),
    })
}

pub(crate) fn media_packet(packet: &replay::Packet) -> Packet {
    let stream = usize::from(packet.stream.get());
    let unit = ticks_per_unit(stream);
    Packet {
        stream,
        pts: Some(packet.pts.0.div_euclid(unit)),
        dts: Some(packet.dts.0.div_euclid(unit)),
        duration: packet.duration.0.div_euclid(unit),
        keyframe: packet.keyframe,
        data: packet.data.to_vec(),
        ..Packet::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rebase {
    video_offset: i64,
    audio_offset: i64,
}

impl Rebase {
    const AUDIO_PACKETS_KEPT_BEFORE_THE_START: i64 = 2;

    pub(crate) fn starting_at(video_pts: i64) -> Self {
        Self {
            video_offset: video_pts,
            audio_offset: audio_frames_from_video_units(video_pts),
        }
    }

    pub(crate) fn start(self) -> i64 {
        self.video_offset
    }

    pub(crate) fn apply(self, mut packet: Packet) -> Option<Packet> {
        let offset = if packet.stream == 0 {
            self.video_offset
        } else {
            self.audio_offset
        };
        let pts = packet.pts? - offset;
        let earliest = if packet.stream == 0 {
            0
        } else {
            -Self::AUDIO_PACKETS_KEPT_BEFORE_THE_START * packet.duration
        };
        if pts < earliest {
            return None;
        }
        packet.pts = Some(pts);
        packet.dts = packet.dts.map(|dts| dts - offset);
        Some(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(stream: usize, pts: i64, duration: i64) -> Packet {
        Packet {
            stream,
            pts: Some(pts),
            dts: Some(pts),
            duration,
            keyframe: true,
            data: vec![1, 2, 3],
            ..Packet::default()
        }
    }

    #[test]
    fn a_packet_survives_the_trip_through_the_replay_time_base() {
        for original in [packet(0, 90_001, 1_500), packet(2, -1_024, 1_024)] {
            let stored = replay_packet(&original).unwrap();
            assert_eq!(media_packet(&stored), original);
        }
        assert!(replay_packet(&packet(4, 0, 1)).is_none());
    }

    #[test]
    fn a_rebased_file_starts_at_zero_and_keeps_two_audio_packets_of_lead_in() {
        let rebase = Rebase::starting_at(90_000);
        assert_eq!(rebase.apply(packet(0, 90_000, 1_500)).unwrap().pts, Some(0));
        assert!(rebase.apply(packet(0, 88_500, 1_500)).is_none());
        assert_eq!(
            rebase.apply(packet(1, 46_000, 1_024)).unwrap().pts,
            Some(-2_000)
        );
        assert!(rebase.apply(packet(1, 45_000, 1_024)).is_none());
    }

    #[test]
    fn nanoseconds_become_video_units_and_audio_frames() {
        assert_eq!(video_units_from_nanos(1_000_000_000), 90_000);
        assert_eq!(audio_frames_from_nanos(10_000_000), 480);
        assert_eq!(micros_from_video_units(45_000), 500_000);
        assert_eq!(video_units_from_ticks(Ticks::from_secs(2)), 180_000);
    }
}
