use std::collections::VecDeque;

use ngine_media::{AudioFrame, AudioLayout, Rational, Resampler};

use crate::error::RecorderError;

pub(crate) const SAMPLE_RATE: u32 = 48_000;
pub(crate) const CHANNELS: usize = 2;
pub(crate) const SAMPLE_TIME: Rational = Rational::new(1, 48_000);
const LONGEST_SILENT_GAP: i64 = 10 * 48_000;
const LONGEST_WAIT_FOR_A_LINE: i64 = 48_000;

/// How loud each source is in a clip. `1.0` leaves it as recorded and `0.0`
/// mutes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipMix {
    pub system: f32,
    pub microphone: f32,
}

impl Default for ClipMix {
    fn default() -> Self {
        Self {
            system: 1.0,
            microphone: 1.0,
        }
    }
}

impl ClipMix {
    /// Whether the clip sounds like the recording's own mix.
    pub fn is_as_recorded(self) -> bool {
        self == Self::default()
    }

    pub(crate) fn gains(self) -> [f32; 2] {
        [self.system.clamp(0.0, 4.0), self.microphone.clamp(0.0, 4.0)]
    }
}

struct Line {
    gain: f32,
    start: i64,
    samples: VecDeque<f32>,
    started: bool,
    ended: bool,
    resampler: Option<(AudioLayout, Resampler)>,
}

impl Line {
    fn end(&self) -> i64 {
        self.start + i64::try_from(self.samples.len() / CHANNELS).unwrap_or(i64::MAX)
    }
}

pub(crate) struct TimelineMix {
    lines: Vec<Line>,
    position: Option<i64>,
    floor: i64,
}

impl TimelineMix {
    pub(crate) fn new(gains: &[f32]) -> Self {
        Self {
            lines: gains
                .iter()
                .map(|gain| Line {
                    gain: *gain,
                    start: 0,
                    samples: VecDeque::new(),
                    started: false,
                    ended: false,
                    resampler: None,
                })
                .collect(),
            position: None,
            floor: i64::MIN,
        }
    }

    pub(crate) fn set_gains(&mut self, gains: &[f32]) {
        for (line, gain) in self.lines.iter_mut().zip(gains) {
            line.gain = *gain;
        }
    }

    pub(crate) fn restart_from(&mut self, floor: i64) {
        for line in &mut self.lines {
            line.samples.clear();
            line.started = false;
            line.ended = false;
            line.resampler = None;
        }
        self.position = None;
        self.floor = floor;
    }

    pub(crate) fn push(
        &mut self,
        line: usize,
        frame: &AudioFrame,
        time_base: Rational,
    ) -> Result<(), RecorderError> {
        let floor = self.position.unwrap_or(self.floor);
        let Some(line) = self.lines.get_mut(line) else {
            return Ok(());
        };
        let layout = AudioLayout {
            sample_rate: frame.sample_rate,
            channels: frame.channels,
        };
        let converted;
        let samples = if frame.sample_rate == SAMPLE_RATE && usize::from(frame.channels) == CHANNELS
        {
            &frame.samples
        } else {
            if line
                .resampler
                .as_ref()
                .is_none_or(|(known, _)| *known != layout)
            {
                line.resampler = Some((layout, Resampler::new(layout)?));
            }
            let Some((_, resampler)) = line.resampler.as_mut() else {
                return Ok(());
            };
            converted = resampler.push(&frame.samples)?;
            &converted
        };
        let stamped = frame
            .pts
            .and_then(|pts| time_base.rescale(pts, SAMPLE_TIME));
        let mut at = match (stamped, line.started) {
            (Some(at), _) => at,
            (None, true) => line.end(),
            (None, false) => floor.max(0),
        };
        let mut samples = samples.as_slice();
        if !line.started {
            line.started = true;
            line.start = at.max(floor);
        }
        let end = line.end();
        if at < end {
            let late = usize::try_from(end - at).unwrap_or(usize::MAX);
            samples = samples.get(late.saturating_mul(CHANNELS)..).unwrap_or(&[]);
            at = end;
        }
        let gap = (at - end).min(LONGEST_SILENT_GAP);
        let silent = usize::try_from(gap).unwrap_or(0) * CHANNELS;
        line.samples.extend(std::iter::repeat_n(0.0, silent));
        line.samples.extend(samples);
        Ok(())
    }

    pub(crate) fn end_line(&mut self, line: usize) {
        if let Some(line) = self.lines.get_mut(line) {
            line.ended = true;
        }
    }

    fn ready_until(&self) -> Option<i64> {
        let furthest = self
            .lines
            .iter()
            .filter(|line| line.started)
            .map(Line::end)
            .max()?;
        let waited_for = self
            .lines
            .iter()
            .filter(|line| !line.ended)
            .map(|line| {
                if line.started {
                    line.end()
                } else {
                    furthest - LONGEST_WAIT_FOR_A_LINE
                }
            })
            .min();
        Some(waited_for.unwrap_or(furthest).min(furthest))
    }

    pub(crate) fn take(&mut self) -> Option<(i64, Vec<f32>)> {
        let until = self.ready_until()?;
        let from = match self.position {
            Some(position) => position,
            None => self
                .lines
                .iter()
                .filter(|line| line.started)
                .map(|line| line.start)
                .min()?,
        };
        if until <= from {
            return None;
        }
        let frames = usize::try_from(until - from).unwrap_or(0);
        let mut mixed = vec![0.0_f32; frames * CHANNELS];
        for line in self.lines.iter_mut().filter(|line| line.started) {
            let lead = usize::try_from(line.start - from).unwrap_or(0).min(frames);
            let taken = ((frames - lead) * CHANNELS).min(line.samples.len());
            for (mixed, sample) in mixed[lead * CHANNELS..]
                .iter_mut()
                .zip(line.samples.drain(..taken))
            {
                *mixed += sample * line.gain;
            }
            line.start = line.start.max(until);
        }
        for sample in &mut mixed {
            *sample = sample.clamp(-1.0, 1.0);
        }
        self.position = Some(until);
        Some((from, mixed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pts: i64, samples: &[f32]) -> AudioFrame {
        AudioFrame {
            pts: Some(pts),
            sample_rate: SAMPLE_RATE,
            channels: 2,
            samples: samples.to_vec(),
        }
    }

    #[test]
    fn two_lines_are_added_where_they_overlap_with_their_gains() {
        let mut mix = TimelineMix::new(&[1.0, 0.5]);
        mix.push(0, &frame(100, &[0.2; 8]), SAMPLE_TIME).unwrap();
        mix.push(1, &frame(102, &[0.4; 8]), SAMPLE_TIME).unwrap();
        let (start, samples) = mix.take().unwrap();
        assert_eq!(start, 100);
        assert_eq!(samples.len(), 8);
        assert!((samples[0] - 0.2).abs() < 1e-6);
        assert!((samples[4] - 0.4).abs() < 1e-6);
        mix.end_line(0);
        let (start, samples) = mix.take().unwrap();
        assert_eq!(start, 104);
        assert!(samples.iter().all(|sample| (sample - 0.2).abs() < 1e-6));
    }

    #[test]
    fn sound_before_the_floor_is_dropped_and_a_gap_becomes_silence() {
        let mut mix = TimelineMix::new(&[1.0]);
        mix.restart_from(4);
        mix.push(0, &frame(0, &[0.5; 12]), SAMPLE_TIME).unwrap();
        mix.push(0, &frame(8, &[0.25; 4]), SAMPLE_TIME).unwrap();
        mix.end_line(0);
        let (start, samples) = mix.take().unwrap();
        assert_eq!(start, 4);
        assert_eq!(
            samples,
            [
                0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.25, 0.25, 0.25, 0.25
            ]
        );
    }
}
