use crate::error::{Error, Result};
use crate::nal::{AVCC_LENGTH_SIZE_AT, HVCC_LENGTH_SIZE_AT, length_size, units};
use crate::types::VideoCodec;

const START_CODE: [u8; 4] = [0, 0, 0, 1];
const H264_SPS: u8 = 7;
const H264_PPS: u8 = 8;
const H264_DELIMITER: u8 = 9;
const HEVC_VPS: u8 = 32;
const HEVC_PPS: u8 = 34;
const HEVC_DELIMITER: u8 = 35;
const AVCC_SETS_AT: usize = 5;
const HVCC_ARRAYS_AT: usize = 22;

fn length_size_at(codec: VideoCodec) -> Result<usize> {
    match codec {
        VideoCodec::H264 => Ok(AVCC_LENGTH_SIZE_AT),
        VideoCodec::Hevc => Ok(HVCC_LENGTH_SIZE_AT),
        VideoCodec::Av1 => Err(Error::unavailable(
            "joining re-encoded AV1 to copied AV1 is not built yet",
        )),
    }
}

fn unit_kind(codec: VideoCodec, unit: &[u8]) -> Option<u8> {
    let header = *unit.first()?;
    Some(match codec {
        VideoCodec::H264 => header & 0x1f,
        _ => (header >> 1) & 0x3f,
    })
}

fn is_parameter_set(codec: VideoCodec, unit: &[u8]) -> bool {
    unit_kind(codec, unit).is_some_and(|kind| match codec {
        VideoCodec::H264 => kind == H264_SPS || kind == H264_PPS,
        _ => (HEVC_VPS..=HEVC_PPS).contains(&kind),
    })
}

fn is_delimiter(codec: VideoCodec, unit: &[u8]) -> bool {
    unit_kind(codec, unit).is_some_and(|kind| match codec {
        VideoCodec::H264 => kind == H264_DELIMITER,
        _ => kind == HEVC_DELIMITER,
    })
}

fn split(packet: &[u8], length_size: Option<usize>) -> Vec<&[u8]> {
    let mut found = units(packet, length_size);
    if length_size.is_none() {
        for unit in &mut found {
            while let [rest @ .., 0] = *unit {
                *unit = rest;
            }
        }
    }
    found
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (first, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(*first)
    }

    fn sized_unit(&mut self) -> Option<&'a [u8]> {
        let length = usize::from(u16::from_be_bytes([self.byte()?, self.byte()?]));
        let (unit, rest) = self.0.split_at_checked(length)?;
        self.0 = rest;
        Some(unit)
    }
}

fn avcc_sets(extradata: &[u8]) -> Option<Vec<&[u8]>> {
    let mut reader = Reader(extradata.get(AVCC_SETS_AT..)?);
    let mut sets = Vec::new();
    for mask in [0x1f, 0xff] {
        for _ in 0..reader.byte()? & mask {
            sets.push(reader.sized_unit()?);
        }
    }
    Some(sets)
}

fn hvcc_sets(extradata: &[u8]) -> Option<Vec<&[u8]>> {
    let mut reader = Reader(extradata.get(HVCC_ARRAYS_AT..)?);
    let mut sets = Vec::new();
    for _ in 0..reader.byte()? {
        reader.byte()?;
        let count = u16::from_be_bytes([reader.byte()?, reader.byte()?]);
        for _ in 0..count {
            sets.push(reader.sized_unit()?);
        }
    }
    Some(sets)
}

fn parameter_sets(codec: VideoCodec, extradata: &[u8]) -> Result<Vec<&[u8]>> {
    let boxed = length_size(extradata, length_size_at(codec)?).is_some();
    let sets = match (boxed, codec) {
        (false, _) => Some(split(extradata, None)),
        (true, VideoCodec::H264) => avcc_sets(extradata),
        (true, _) => hvcc_sets(extradata),
    };
    sets.map(|sets| {
        sets.into_iter()
            .filter(|unit| is_parameter_set(codec, unit))
            .collect()
    })
    .ok_or_else(|| Error::invalid("the video track's parameter sets cannot be read"))
}

fn pack(units: &[&[u8]], length_size: Option<usize>) -> Result<Vec<u8>> {
    let mut packed = Vec::with_capacity(units.iter().map(|unit| unit.len() + 4).sum());
    for unit in units {
        match length_size {
            None => packed.extend_from_slice(&START_CODE),
            Some(size) => {
                let length = u32::try_from(unit.len())
                    .ok()
                    .filter(|length| size >= 4 || u64::from(*length) < 1 << (8 * size))
                    .ok_or_else(|| Error::invalid("a video unit is too long for its track"))?;
                packed.extend_from_slice(&length.to_be_bytes()[4 - size.min(4)..]);
            }
        }
        packed.extend_from_slice(unit);
    }
    Ok(packed)
}

/// Whether two encoders' global headers describe the same H.264 or HEVC
/// stream, so packets of one decode in a track described by the other.
pub fn same_parameter_sets(codec: VideoCodec, one: &[u8], other: &[u8]) -> bool {
    match (parameter_sets(codec, one), parameter_sets(codec, other)) {
        (Ok(one), Ok(other)) => !one.is_empty() && one == other,
        _ => false,
    }
}

/// Rewrites a packet made by an encoder with the global headers `encoder`
/// into the framing of the track described by `track`. When the two describe
/// different streams, a keyframe gets the encoder's parameter sets in front,
/// so a decoder switches to them at that keyframe.
pub fn conform_to_track(
    codec: VideoCodec,
    track: &[u8],
    encoder: &[u8],
    packet: &[u8],
    keyframe: bool,
) -> Result<Vec<u8>> {
    let at = length_size_at(codec)?;
    let same = same_parameter_sets(codec, track, encoder);
    let mut wanted = Vec::new();
    if keyframe && !same {
        wanted = parameter_sets(codec, encoder)?;
    }
    let own = split(packet, length_size(encoder, at));
    wanted.extend(own.into_iter().filter(|unit| {
        !unit.is_empty()
            && !is_delimiter(codec, unit)
            && !(is_parameter_set(codec, unit) && (same || keyframe))
    }));
    pack(&wanted, length_size(track, at))
}

/// Puts the track's own parameter sets in front of one of its keyframes. The
/// first copied keyframe after re-encoded video needs this when the encoder's
/// parameter sets differ from the track's.
pub fn with_parameter_sets(codec: VideoCodec, track: &[u8], packet: &[u8]) -> Result<Vec<u8>> {
    let size = length_size(track, length_size_at(codec)?);
    let mut wanted = parameter_sets(codec, track)?;
    wanted.extend(
        split(packet, size)
            .into_iter()
            .filter(|unit| !is_parameter_set(codec, unit)),
    );
    pack(&wanted, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: [u8; 3] = [0x67, 1, 2];
    const PPS: [u8; 2] = [0x68, 3];
    const OTHER_SPS: [u8; 3] = [0x67, 9, 9];

    fn avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
        let mut extradata = vec![1, 0x64, 0, 0x1f, 0xff, 0xe1, 0, 3];
        extradata.extend_from_slice(sps);
        extradata.extend_from_slice(&[1, 0, 2]);
        extradata.extend_from_slice(pps);
        extradata
    }

    fn annex_b(units: &[&[u8]]) -> Vec<u8> {
        pack(units, None).unwrap()
    }

    #[test]
    fn the_same_sets_match_across_framings_and_different_ones_do_not() {
        let boxed = avcc(&SPS, &PPS);
        assert!(same_parameter_sets(
            VideoCodec::H264,
            &boxed,
            &annex_b(&[&SPS, &PPS])
        ));
        assert!(!same_parameter_sets(
            VideoCodec::H264,
            &boxed,
            &annex_b(&[&OTHER_SPS, &PPS])
        ));
        assert!(!same_parameter_sets(VideoCodec::H264, &[], &[]));
    }

    #[test]
    fn a_matching_encoder_packet_is_reframed_without_its_parameter_sets() {
        let track = avcc(&SPS, &PPS);
        let encoder = annex_b(&[&SPS, &PPS]);
        let packet = annex_b(&[&[9, 0xf0], &SPS, &PPS, &[0x65, 7, 7]]);
        let conformed =
            conform_to_track(VideoCodec::H264, &track, &encoder, &packet, true).unwrap();
        assert_eq!(conformed, [0, 0, 0, 3, 0x65, 7, 7]);
    }

    #[test]
    fn a_foreign_keyframe_carries_its_encoders_sets_and_other_frames_do_not() {
        let track = avcc(&SPS, &PPS);
        let encoder = annex_b(&[&OTHER_SPS, &PPS]);
        let keyframe = annex_b(&[&[0x65, 7]]);
        let conformed =
            conform_to_track(VideoCodec::H264, &track, &encoder, &keyframe, true).unwrap();
        assert_eq!(
            conformed,
            [
                0, 0, 0, 3, 0x67, 9, 9, 0, 0, 0, 2, 0x68, 3, 0, 0, 0, 2, 0x65, 7
            ]
        );
        let later = annex_b(&[&[0x41, 5]]);
        let conformed =
            conform_to_track(VideoCodec::H264, &track, &encoder, &later, false).unwrap();
        assert_eq!(conformed, [0, 0, 0, 2, 0x41, 5]);
    }

    #[test]
    fn a_copied_keyframe_gets_its_tracks_sets_exactly_once() {
        let track = avcc(&SPS, &PPS);
        let packet = pack(&[&SPS, &[0x65, 4]], Some(4)).unwrap();
        let prefixed = with_parameter_sets(VideoCodec::H264, &track, &packet).unwrap();
        assert_eq!(prefixed, pack(&[&SPS, &PPS, &[0x65, 4]], Some(4)).unwrap());
    }

    #[test]
    fn hevc_sets_are_read_from_an_hvcc_box() {
        let vps: [u8; 3] = [32 << 1, 1, 5];
        let sps: [u8; 3] = [33 << 1, 1, 6];
        let mut boxed = vec![0_u8; 23];
        boxed[0] = 1;
        boxed[HVCC_LENGTH_SIZE_AT] = 0xff;
        boxed[HVCC_ARRAYS_AT] = 2;
        for set in [&vps, &sps] {
            boxed.extend_from_slice(&[0x80 | (set[0] >> 1), 0, 1, 0, 3]);
            boxed.extend_from_slice(set);
        }
        assert!(same_parameter_sets(
            VideoCodec::Hevc,
            &boxed,
            &annex_b(&[&vps, &sps])
        ));
    }

    #[test]
    fn av1_is_refused() {
        assert!(conform_to_track(VideoCodec::Av1, &[], &[], &[1], true).is_err());
    }
}
