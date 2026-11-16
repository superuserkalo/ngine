use crate::types::VideoCodec;

const H264_IDR_SLICE: u8 = 5;
const H264_LAST_SLICE_TYPE: u8 = 5;
const HEVC_IDR_W_RADL: u8 = 19;
const HEVC_IDR_N_LP: u8 = 20;
const HEVC_LAST_SLICE_TYPE: u8 = 31;
const AVCC_LENGTH_SIZE_AT: usize = 4;
const HVCC_LENGTH_SIZE_AT: usize = 21;

pub(crate) fn starts_closed_group(codec: VideoCodec, extradata: &[u8], packet: &[u8]) -> bool {
    let slice_type = match codec {
        VideoCodec::Av1 => return true,
        VideoCodec::H264 => units(packet, length_size(extradata, AVCC_LENGTH_SIZE_AT))
            .into_iter()
            .filter_map(|unit| unit.first().map(|header| header & 0x1f))
            .find(|kind| (1..=H264_LAST_SLICE_TYPE).contains(kind))
            .map(|kind| kind == H264_IDR_SLICE),
        VideoCodec::Hevc => units(packet, length_size(extradata, HVCC_LENGTH_SIZE_AT))
            .into_iter()
            .filter_map(|unit| unit.first().map(|header| (header >> 1) & 0x3f))
            .find(|kind| *kind <= HEVC_LAST_SLICE_TYPE)
            .map(|kind| kind == HEVC_IDR_W_RADL || kind == HEVC_IDR_N_LP),
    };
    slice_type.unwrap_or(true)
}

fn length_size(extradata: &[u8], at: usize) -> Option<usize> {
    match (extradata.first(), extradata.get(at)) {
        (Some(1), Some(byte)) => Some(usize::from(byte & 3) + 1),
        _ => None,
    }
}

fn units(packet: &[u8], length_size: Option<usize>) -> Vec<&[u8]> {
    match length_size {
        Some(size) => length_prefixed_units(packet, size),
        None => start_code_units(packet),
    }
}

fn length_prefixed_units(mut packet: &[u8], length_size: usize) -> Vec<&[u8]> {
    let mut units = Vec::new();
    while let Some((prefix, rest)) = packet.split_at_checked(length_size) {
        let length = prefix
            .iter()
            .fold(0_usize, |length, byte| (length << 8) | usize::from(*byte));
        let Some((unit, rest)) = rest.split_at_checked(length) else {
            break;
        };
        units.push(unit);
        packet = rest;
    }
    units
}

fn start_code_units(packet: &[u8]) -> Vec<&[u8]> {
    let starts = packet
        .windows(3)
        .enumerate()
        .filter(|(_, window)| *window == [0, 0, 1])
        .map(|(at, _)| at + 3)
        .collect::<Vec<_>>();
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = starts.get(index + 1).map_or(packet.len(), |next| next - 3);
            &packet[start..end.max(start)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVCC: [u8; 7] = [1, 0x64, 0, 0x1f, 0xff, 0xe1, 0];

    fn hvcc() -> Vec<u8> {
        let mut extradata = vec![0_u8; 23];
        extradata[0] = 1;
        extradata[HVCC_LENGTH_SIZE_AT] = 0xff;
        extradata
    }

    fn prefixed(units: &[&[u8]]) -> Vec<u8> {
        units
            .iter()
            .flat_map(|unit| {
                let length = u32::try_from(unit.len()).unwrap().to_be_bytes();
                length.into_iter().chain(unit.iter().copied())
            })
            .collect()
    }

    #[test]
    fn an_h264_idr_slice_after_parameter_sets_starts_a_closed_group() {
        let packet = prefixed(&[&[0x67, 1], &[0x68, 2], &[0x65, 3]]);
        assert!(starts_closed_group(VideoCodec::H264, &AVCC, &packet));
        let recovery_point = prefixed(&[&[0x06, 6], &[0x41, 3]]);
        assert!(!starts_closed_group(
            VideoCodec::H264,
            &AVCC,
            &recovery_point
        ));
    }

    #[test]
    fn an_hevc_cra_picture_does_not_start_a_closed_group_and_an_idr_does() {
        let idr = prefixed(&[&[32 << 1, 1], &[HEVC_IDR_W_RADL << 1, 1, 0]]);
        let cra = prefixed(&[&[21 << 1, 1, 0]]);
        assert!(starts_closed_group(VideoCodec::Hevc, &hvcc(), &idr));
        assert!(!starts_closed_group(VideoCodec::Hevc, &hvcc(), &cra));
    }

    #[test]
    fn annex_b_packets_are_split_at_start_codes() {
        let idr = [0, 0, 0, 1, 0x67, 9, 0, 0, 1, 0x65, 8];
        let slice = [0, 0, 1, 0x41, 8];
        assert!(starts_closed_group(VideoCodec::H264, &[], &idr));
        assert!(!starts_closed_group(
            VideoCodec::H264,
            &[0, 0, 0, 1, 0x67],
            &slice
        ));
    }

    #[test]
    fn a_packet_that_cannot_be_parsed_is_trusted() {
        assert!(starts_closed_group(VideoCodec::H264, &AVCC, &[0, 0]));
        assert!(starts_closed_group(VideoCodec::Av1, &[], &[1, 2, 3]));
    }
}
