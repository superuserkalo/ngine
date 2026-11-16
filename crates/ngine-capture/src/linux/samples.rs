pub(super) fn f32_from_le_bytes(
    bytes: &[u8],
    offset: usize,
    size: usize,
    channels: u16,
) -> Vec<f32> {
    let end = offset.saturating_add(size).min(bytes.len());
    let chunk = bytes.get(offset..end).unwrap_or_default();
    let samples = chunk.as_chunks::<4>().0;
    let whole_frames = match usize::from(channels) {
        0 => 0,
        channels => samples.len() - samples.len() % channels,
    };
    samples[..whole_frames]
        .iter()
        .map(|sample| f32::from_le_bytes(*sample))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_of(samples: &[f32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn the_chunk_of_a_buffer_is_read_as_float_samples() {
        let bytes = bytes_of(&[9.0, 0.25, -0.5, 0.75, 1.0, 9.0]);

        assert_eq!(f32_from_le_bytes(&bytes, 4, 16, 2), [0.25, -0.5, 0.75, 1.0]);
    }

    #[test]
    fn a_partial_frame_and_a_chunk_past_the_end_are_cut_off() {
        let bytes = bytes_of(&[0.1, 0.2, 0.3]);

        assert_eq!(f32_from_le_bytes(&bytes, 0, 12, 2), [0.1, 0.2]);
        assert_eq!(f32_from_le_bytes(&bytes, 4, 400, 1), [0.2, 0.3]);
        assert_eq!(f32_from_le_bytes(&bytes, 400, 4, 1), [] as [f32; 0]);
        assert_eq!(f32_from_le_bytes(&bytes, 0, 12, 0), [] as [f32; 0]);
    }
}
