use std::collections::VecDeque;

use ngine_capture::{AudioBuffer, AudioSource};
use ngine_media::{
    AudioEncoder, AudioEncoderSettings, AudioLayout, Mixer, Packet, Resampler, StreamInfo,
};

use crate::error::RecorderError;
use crate::settings::{AUDIO_TRACK_BIT_RATE, AudioSettings};
use crate::time::AUDIO_FRAMES_PER_SECOND;

const CHANNELS: usize = 2;
const NANOS_PER_SECOND: i128 = 1_000_000_000;
const MICROS_PER_SECOND: i128 = 1_000_000;
const SILENT_SOURCE_WAIT_FRAMES: u64 = 24_000;
const LATEST_RETURN_OF_A_SILENT_SOURCE_FRAMES: u64 = 7_200;
const SMALLEST_STEP_FRAMES: u64 = 480;
const LARGEST_STEP_FRAMES: u64 = 48_000;
const LONGEST_RESAMPLED_SILENCE_FRAMES: u64 = 96_000;

struct Line {
    layout: Option<AudioLayout>,
    resampler: Option<Resampler>,
    resampler_base: u64,
    produced: u64,
    taken: u64,
    waiting: VecDeque<f32>,
}

fn frames_from_nanos(nanos: i64) -> u64 {
    let frames = i128::from(nanos.max(0)) * i128::from(AUDIO_FRAMES_PER_SECOND) / NANOS_PER_SECOND;
    u64::try_from(frames).unwrap_or(u64::MAX)
}

impl Line {
    fn new() -> Self {
        Self {
            layout: None,
            resampler: None,
            resampler_base: 0,
            produced: 0,
            taken: 0,
            waiting: VecDeque::new(),
        }
    }

    fn accept(&mut self, samples: &[f32]) {
        let frames = (samples.len() / CHANNELS) as u64;
        let already_taken_as_silence = self.taken.saturating_sub(self.produced).min(frames);
        let skipped = usize::try_from(already_taken_as_silence).unwrap_or(usize::MAX);
        self.waiting
            .extend(samples.iter().skip(skipped.saturating_mul(CHANNELS)));
        self.produced += frames;
    }

    fn retire_resampler(&mut self) -> Result<(), RecorderError> {
        if let Some(mut resampler) = self.resampler.take() {
            let tail = resampler.finish()?;
            self.accept(&tail);
        }
        Ok(())
    }

    fn stay_silent_until(&mut self, frame: u64) {
        let not_yet_taken = frame.saturating_sub(self.produced.max(self.taken));
        let zeros = usize::try_from(not_yet_taken).unwrap_or(0) * CHANNELS;
        self.waiting.extend(std::iter::repeat_n(0.0, zeros));
        self.produced = self.produced.max(frame);
    }

    fn restart_resampler(&mut self, layout: AudioLayout) -> Result<(), RecorderError> {
        self.retire_resampler()?;
        self.resampler = Some(Resampler::synchronized(layout)?);
        self.layout = Some(layout);
        self.resampler_base = self.produced;
        Ok(())
    }

    fn push(&mut self, buffer: &AudioBuffer, start_nanos: i64) -> Result<(), RecorderError> {
        let layout = AudioLayout {
            sample_rate: buffer.sample_rate(),
            channels: buffer.channels(),
        };
        let channels = usize::from(buffer.channels());
        let early_nanos = u128::from(start_nanos.min(0).unsigned_abs());
        let early_frames = (early_nanos * u128::from(buffer.sample_rate()))
            .div_ceil(NANOS_PER_SECOND.unsigned_abs());
        let early_frames = usize::try_from(early_frames).unwrap_or(usize::MAX);
        if early_frames >= buffer.frames() {
            return Ok(());
        }
        let samples = &buffer.samples()[early_frames * channels..];
        let start_nanos = start_nanos.max(0);

        let start_frame = frames_from_nanos(start_nanos);
        let silent_for = start_frame.saturating_sub(self.produced);
        if silent_for > LONGEST_RESAMPLED_SILENCE_FRAMES {
            self.retire_resampler()?;
            self.stay_silent_until(start_frame);
        }
        if self.layout != Some(layout) || self.resampler.is_none() {
            self.restart_resampler(layout)?;
        }
        let base_micros = i128::from(self.resampler_base) * MICROS_PER_SECOND
            / i128::from(AUDIO_FRAMES_PER_SECOND);
        let start_micros = i128::from(start_nanos) / 1000 - base_micros;
        let start_micros = i64::try_from(start_micros).unwrap_or(0).max(0);
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(());
        };
        let converted = resampler.push_at(start_micros, samples)?;
        self.accept(&converted);
        Ok(())
    }

    fn take(&mut self, frames: usize) -> Vec<f32> {
        let wanted = frames * CHANNELS;
        let mut samples: Vec<f32> = self
            .waiting
            .drain(..wanted.min(self.waiting.len()))
            .collect();
        samples.resize(wanted, 0.0);
        self.taken += frames as u64;
        samples
    }
}

pub(crate) struct AudioStage {
    system: Option<Line>,
    microphone: Option<Line>,
    mixer: Mixer,
    encoders: Vec<AudioEncoder>,
    position: u64,
}

impl AudioStage {
    pub(crate) fn open(settings: &AudioSettings) -> Result<Self, RecorderError> {
        let encoders = (0..settings.track_count())
            .map(|_| {
                AudioEncoder::open(AudioEncoderSettings {
                    codec: settings.codec,
                    bitrate: AUDIO_TRACK_BIT_RATE,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut mixer = Mixer::new(2);
        mixer.set_gain(0, settings.system_gain.unwrap_or(0.0))?;
        mixer.set_gain(1, settings.microphone_gain.unwrap_or(0.0))?;
        Ok(Self {
            system: settings.system_gain.map(|_| Line::new()),
            microphone: settings.microphone_gain.map(|_| Line::new()),
            mixer,
            encoders,
            position: 0,
        })
    }

    pub(crate) fn stream_infos(&self) -> impl Iterator<Item = &StreamInfo> {
        self.encoders.iter().map(AudioEncoder::stream_info)
    }

    fn line(&mut self, source: AudioSource) -> Option<&mut Line> {
        match source {
            AudioSource::System => self.system.as_mut(),
            AudioSource::Microphone => self.microphone.as_mut(),
            _ => None,
        }
    }

    pub(crate) fn push(
        &mut self,
        buffer: &AudioBuffer,
        start_nanos: i64,
    ) -> Result<(), RecorderError> {
        match self.line(buffer.source()) {
            Some(line) => line.push(buffer, start_nanos),
            None => Ok(()),
        }
    }

    fn lines(&self) -> impl Iterator<Item = &Line> {
        self.system.iter().chain(self.microphone.iter())
    }

    fn horizon(&self, now_frames: u64) -> u64 {
        self.lines()
            .map(|line| {
                let delivers_nothing = line.produced + SILENT_SOURCE_WAIT_FRAMES < now_frames;
                if delivers_nothing {
                    now_frames.saturating_sub(LATEST_RETURN_OF_A_SILENT_SOURCE_FRAMES)
                } else {
                    line.produced
                }
            })
            .min()
            .unwrap_or(self.position)
            .min(now_frames)
            .max(self.position)
    }

    fn step(&mut self, frames: usize) -> Result<Vec<Packet>, RecorderError> {
        let silence = || vec![0.0_f32; frames * CHANNELS];
        let system = self
            .system
            .as_mut()
            .map_or_else(silence, |line| line.take(frames));
        let microphone = self
            .microphone
            .as_mut()
            .map_or_else(silence, |line| line.take(frames));
        self.mixer.push(0, &system)?;
        self.mixer.push(1, &microphone)?;
        let mix = self.mixer.mix(frames);
        self.position += frames as u64;
        let mut packets = Vec::new();
        for (track, samples) in [mix, system, microphone].iter().enumerate() {
            if let Some(encoder) = self.encoders.get_mut(track) {
                packets.extend(on_track(track, encoder.encode(samples)?));
            }
        }
        Ok(packets)
    }

    pub(crate) fn advance(&mut self, now_frames: u64) -> Result<Vec<Packet>, RecorderError> {
        let mut packets = Vec::new();
        if self.encoders.is_empty() {
            return Ok(packets);
        }
        let horizon = self.horizon(now_frames);
        while horizon - self.position >= SMALLEST_STEP_FRAMES {
            let frames = (horizon - self.position).min(LARGEST_STEP_FRAMES);
            packets.extend(self.step(usize::try_from(frames).unwrap_or(0))?);
        }
        Ok(packets)
    }

    pub(crate) fn finish(&mut self, now_frames: u64) -> Result<Vec<Packet>, RecorderError> {
        let mut packets = Vec::new();
        if self.encoders.is_empty() {
            return Ok(packets);
        }
        for line in self.system.iter_mut().chain(self.microphone.iter_mut()) {
            line.retire_resampler()?;
        }
        while self.position < now_frames {
            let frames = (now_frames - self.position).min(LARGEST_STEP_FRAMES);
            packets.extend(self.step(usize::try_from(frames).unwrap_or(0))?);
        }
        for (track, encoder) in self.encoders.iter_mut().enumerate() {
            packets.extend(on_track(track, encoder.finish()?));
        }
        Ok(packets)
    }
}

fn on_track(track: usize, packets: Vec<Packet>) -> impl Iterator<Item = Packet> {
    packets.into_iter().map(move |mut packet| {
        packet.stream = track + 1;
        packet
    })
}

#[cfg(test)]
mod tests {
    use ngine_capture::Timestamp;
    use ngine_media::AudioCodec;

    use super::*;
    use crate::settings::Tracks;

    fn buffer(source: AudioSource, frames: usize, level: f32) -> AudioBuffer {
        AudioBuffer::new(
            source,
            Timestamp::from_nanos(0),
            48_000,
            2,
            vec![level; frames * 2],
        )
        .unwrap()
    }

    fn settings(tracks: Tracks) -> AudioSettings {
        AudioSettings {
            system_gain: Some(1.0),
            microphone_gain: Some(1.0),
            tracks,
            codec: AudioCodec::Aac,
        }
    }

    #[test]
    fn the_tracks_wait_for_a_source_that_is_only_a_little_behind() {
        let mut stage = AudioStage::open(&settings(Tracks::Three)).unwrap();
        stage
            .push(&buffer(AudioSource::System, 9_600, 0.1), 0)
            .unwrap();
        stage.advance(9_600).unwrap();
        assert_eq!(stage.position, 0);

        stage
            .push(&buffer(AudioSource::Microphone, 4_800, 0.1), 0)
            .unwrap();
        stage.advance(9_600).unwrap();
        assert!(
            (4_000..=4_800).contains(&stage.position),
            "{}",
            stage.position
        );
    }

    #[test]
    fn a_source_that_delivers_nothing_for_half_a_second_no_longer_holds_the_others_back() {
        let mut stage = AudioStage::open(&settings(Tracks::Three)).unwrap();
        for index in 0..100_i64 {
            stage
                .push(&buffer(AudioSource::System, 480, 0.1), index * 10_000_000)
                .unwrap();
        }
        stage.advance(20_000).unwrap();
        assert_eq!(stage.position, 0);
        stage.advance(48_000).unwrap();
        assert!(
            (40_000..=40_800).contains(&stage.position),
            "{}",
            stage.position
        );
    }

    #[test]
    fn sound_from_before_the_stage_began_is_cut_off() {
        let mut line = Line::new();
        line.push(&buffer(AudioSource::System, 480, 0.1), -20_000_000)
            .unwrap();
        assert_eq!(line.produced, 0);
        line.push(&buffer(AudioSource::System, 4_800, 0.1), -5_000_000)
            .unwrap();
        line.retire_resampler().unwrap();
        assert!(line.produced.abs_diff(4_560) <= 64, "{}", line.produced);
    }

    #[test]
    fn mix_only_encodes_one_track_and_no_source_encodes_none() {
        assert_eq!(
            AudioStage::open(&settings(Tracks::MixOnly))
                .unwrap()
                .stream_infos()
                .count(),
            1
        );
        let silent = AudioSettings {
            system_gain: None,
            microphone_gain: None,
            ..settings(Tracks::Three)
        };
        assert_eq!(AudioStage::open(&silent).unwrap().stream_infos().count(), 0);
    }
}
