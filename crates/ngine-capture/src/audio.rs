use std::sync::Arc;
use std::time::Duration;

use crate::time::Timestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AudioSource {
    System,
    Microphone,
}

/// Interleaved 32-bit float samples from one source.
///
/// The sample rate and channel count are what the source delivered and can
/// differ between sources and between buffers of one source.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    source: AudioSource,
    timestamp: Timestamp,
    sample_rate: u32,
    channels: u16,
    samples: Arc<[f32]>,
}

impl AudioBuffer {
    /// Returns `None` unless the rate and channel count are above zero and
    /// `samples` holds a whole number of frames.
    pub fn new(
        source: AudioSource,
        timestamp: Timestamp,
        sample_rate: u32,
        channels: u16,
        samples: Vec<f32>,
    ) -> Option<Self> {
        let whole_frames = channels != 0 && samples.len().is_multiple_of(usize::from(channels));
        (sample_rate != 0 && whole_frames).then(|| Self {
            source,
            timestamp,
            sample_rate,
            channels,
            samples: samples.into(),
        })
    }

    pub fn source(&self) -> AudioSource {
        self.source
    }

    /// The time of the first frame in the buffer.
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }

    pub fn duration(&self) -> Duration {
        let frames = u64::try_from(self.frames()).unwrap_or(u64::MAX);
        Duration::from_nanos(frames.saturating_mul(1_000_000_000) / u64::from(self.sample_rate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_counts_frames_across_channels() {
        let buffer = AudioBuffer::new(
            AudioSource::System,
            Timestamp::from_nanos(0),
            48_000,
            2,
            vec![0.0; 960],
        )
        .unwrap();

        assert_eq!(buffer.frames(), 480);
        assert_eq!(buffer.duration(), Duration::from_millis(10));
    }

    #[test]
    fn a_buffer_rejects_a_partial_frame_and_empty_formats() {
        let build = |rate, channels, samples| {
            AudioBuffer::new(
                AudioSource::Microphone,
                Timestamp::from_nanos(0),
                rate,
                channels,
                vec![0.0; samples],
            )
        };

        assert!(build(48_000, 2, 3).is_none());
        assert!(build(0, 2, 4).is_none());
        assert!(build(48_000, 0, 4).is_none());
    }
}
