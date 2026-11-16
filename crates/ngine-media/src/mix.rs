use std::collections::VecDeque;

use crate::error::{Error, Result};

const CHANNELS: usize = 2;

struct Source {
    gain: f32,
    queue: VecDeque<f32>,
    queue_start: u64,
}

/// Adds 48 kHz stereo streams that share one clock into one.
///
/// Every source is an unbroken stream that starts at frame zero, which is what
/// a synchronized [`Resampler`](crate::Resampler) produces. The mixer never
/// waits: where a source has delivered nothing by the time its frames are
/// mixed, the mix holds silence for it, and those frames are dropped if they
/// arrive later.
pub struct Mixer {
    sources: Vec<Source>,
    position: u64,
}

impl Mixer {
    pub fn new(source_count: usize) -> Self {
        let sources = (0..source_count)
            .map(|_| Source {
                gain: 1.0,
                queue: VecDeque::new(),
                queue_start: 0,
            })
            .collect();
        Self {
            sources,
            position: 0,
        }
    }

    fn source(&mut self, index: usize) -> Result<&mut Source> {
        self.sources
            .get_mut(index)
            .ok_or_else(|| Error::invalid(format!("the mixer has no source {index}")))
    }

    pub fn set_gain(&mut self, source: usize, gain: f32) -> Result<()> {
        self.source(source)?.gain = gain;
        Ok(())
    }

    /// Appends interleaved stereo samples to one source's stream.
    pub fn push(&mut self, source: usize, samples: &[f32]) -> Result<()> {
        if !samples.len().is_multiple_of(CHANNELS) {
            return Err(Error::invalid(
                "stereo sound needs an even number of samples",
            ));
        }
        self.source(source)?.queue.extend(samples);
        Ok(())
    }

    /// Frames of one source that are waiting at or after the mix position.
    pub fn waiting_frames(&self, source: usize) -> usize {
        self.sources.get(source).map_or(0, |source| {
            let queued = (source.queue.len() / CHANNELS) as u64;
            let late = self.position.saturating_sub(source.queue_start);
            usize::try_from(queued.saturating_sub(late)).unwrap_or(usize::MAX)
        })
    }

    /// Frames mixed so far.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Returns the next `frames` frames of the mix, clamped to full scale.
    pub fn mix(&mut self, frames: usize) -> Vec<f32> {
        let mut output = vec![0.0_f32; frames * CHANNELS];
        for source in &mut self.sources {
            let queued = source.queue.len() / CHANNELS;
            let late = usize::try_from(self.position.saturating_sub(source.queue_start))
                .unwrap_or(usize::MAX)
                .min(queued);
            source.queue.drain(..late * CHANNELS);
            source.queue_start += late as u64;
            if source.queue_start != self.position {
                continue;
            }
            let taken = frames.min(queued - late);
            for (mixed, sample) in output
                .iter_mut()
                .zip(source.queue.drain(..taken * CHANNELS))
            {
                *mixed += sample * source.gain;
            }
            source.queue_start += taken as u64;
        }
        for sample in &mut output {
            *sample = sample.clamp(-1.0, 1.0);
        }
        self.position += frames as u64;
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_sources_add_with_their_gains() {
        let mut mixer = Mixer::new(2);
        mixer.set_gain(1, 0.5).unwrap();
        mixer.push(0, &[0.25, 0.25, 0.25, 0.25]).unwrap();
        mixer.push(1, &[0.5, -0.5, 0.5, -0.5]).unwrap();
        assert_eq!(mixer.mix(2), [0.5, 0.0, 0.5, 0.0]);
    }

    #[test]
    fn a_silent_source_does_not_hold_the_mix_back_and_its_late_frames_are_dropped() {
        let mut mixer = Mixer::new(2);
        mixer.push(0, &[0.1; 8]).unwrap();
        assert_eq!(mixer.mix(4), [0.1; 8]);

        mixer.push(1, &[0.2; 12]).unwrap();
        mixer.push(0, &[0.1; 4]).unwrap();
        assert_eq!(mixer.waiting_frames(1), 2);
        let output = mixer.mix(2);
        assert!(output.iter().all(|sample| (sample - 0.3).abs() < 1e-6));
        assert_eq!(mixer.position(), 6);
    }

    #[test]
    fn the_sum_is_clamped_to_full_scale() {
        let mut mixer = Mixer::new(2);
        mixer.push(0, &[0.9, -0.9]).unwrap();
        mixer.push(1, &[0.9, -0.9]).unwrap();
        assert_eq!(mixer.mix(1), [1.0, -1.0]);
    }

    #[test]
    fn an_odd_sample_count_and_an_unknown_source_are_errors() {
        let mut mixer = Mixer::new(1);
        assert!(mixer.push(0, &[0.0; 3]).is_err());
        assert!(mixer.push(1, &[0.0; 2]).is_err());
    }
}
