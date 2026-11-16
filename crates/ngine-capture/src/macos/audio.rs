use screencapturekit::cm::CMSampleBuffer;

use super::convert;
use crate::audio::{AudioBuffer, AudioSource};

const LOWER_HALF_VALUES: f32 = 65_536.0;
const LARGEST_INT32_SAMPLE: f32 = 2_147_483_648.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Float32,
    Int16,
    Int32,
}

impl Encoding {
    fn of(bits_per_channel: u32, is_float: bool) -> Option<Self> {
        match (bits_per_channel, is_float) {
            (32, true) => Some(Self::Float32),
            (16, false) => Some(Self::Int16),
            (32, false) => Some(Self::Int32),
            _ => None,
        }
    }

    fn decode(self, bytes: &[u8]) -> Vec<f32> {
        match self {
            Self::Float32 => bytes
                .as_chunks()
                .0
                .iter()
                .map(|sample| f32::from_ne_bytes(*sample))
                .collect(),
            Self::Int16 => bytes
                .as_chunks()
                .0
                .iter()
                .map(|sample| f32::from(i16::from_ne_bytes(*sample)) / -f32::from(i16::MIN))
                .collect(),
            Self::Int32 => bytes
                .as_chunks()
                .0
                .iter()
                .map(|sample| {
                    let [high_a, high_b, low_a, low_b] = i32::from_ne_bytes(*sample).to_be_bytes();
                    let upper_half = f32::from(i16::from_be_bytes([high_a, high_b]));
                    let lower_half = f32::from(u16::from_be_bytes([low_a, low_b]));
                    (upper_half * LOWER_HALF_VALUES + lower_half) / LARGEST_INT32_SAMPLE
                })
                .collect(),
        }
    }
}

fn interleave(channels: &[Vec<f32>]) -> Vec<f32> {
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    (0..frames)
        .flat_map(|frame| channels.iter().map(move |channel| channel[frame]))
        .collect()
}

pub(super) fn buffer_from(sample: &CMSampleBuffer, source: AudioSource) -> Option<AudioBuffer> {
    let timestamp = convert::timestamp(sample.presentation_timestamp())?;
    let format = sample.format_description()?;
    if format.audio_is_big_endian() != cfg!(target_endian = "big") {
        return None;
    }
    let sample_rate = convert::whole_number(format.audio_sample_rate()?)?;
    let encoding = Encoding::of(format.audio_bits_per_channel()?, format.audio_is_float())?;
    let list = sample.audio_buffer_list().ok()?;
    let mut blocks: Vec<Vec<f32>> = list
        .iter()
        .map(|block| encoding.decode(block.data()))
        .collect();
    let (channels, samples) = if blocks.len() == 1 {
        (format.audio_channel_count()?, blocks.pop()?)
    } else {
        (u32::try_from(blocks.len()).ok()?, interleave(&blocks))
    };
    AudioBuffer::new(
        source,
        timestamp,
        sample_rate,
        u16::try_from(channels).ok()?,
        samples,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separate_channels_become_frames() {
        let left = vec![1.0, 2.0, 3.0];
        let right = vec![-1.0, -2.0, -3.0];

        assert_eq!(
            interleave(&[left, right]),
            [1.0, -1.0, 2.0, -2.0, 3.0, -3.0]
        );
    }

    #[test]
    fn channels_of_unequal_length_keep_only_whole_frames() {
        assert_eq!(interleave(&[vec![1.0, 2.0], vec![5.0]]), [1.0, 5.0]);
        assert_eq!(interleave(&[]), [] as [f32; 0]);
    }

    #[test]
    fn float_samples_are_read_as_they_are() {
        let bytes: Vec<u8> = [0.25_f32, -0.5]
            .iter()
            .flat_map(|sample| sample.to_ne_bytes())
            .collect();

        assert_eq!(Encoding::Float32.decode(&bytes), [0.25, -0.5]);
    }

    #[test]
    fn integer_samples_are_scaled_to_between_minus_one_and_one() {
        let sixteen: Vec<u8> = [i16::MIN, 0, 16_384]
            .iter()
            .flat_map(|sample| sample.to_ne_bytes())
            .collect();
        let thirty_two: Vec<u8> = [i32::MIN, 1 << 30]
            .iter()
            .flat_map(|sample| sample.to_ne_bytes())
            .collect();

        assert_eq!(Encoding::Int16.decode(&sixteen), [-1.0, 0.0, 0.5]);
        assert_eq!(Encoding::Int32.decode(&thirty_two), [-1.0, 0.5]);
    }

    #[test]
    fn only_known_sample_layouts_are_decoded() {
        assert_eq!(Encoding::of(32, true), Some(Encoding::Float32));
        assert_eq!(Encoding::of(16, false), Some(Encoding::Int16));
        assert_eq!(Encoding::of(24, false), None);
        assert_eq!(Encoding::of(64, true), None);
    }
}
