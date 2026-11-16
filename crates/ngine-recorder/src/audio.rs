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
const LARGEST_STEP: usize = 48_000;
const LONGEST_RESAMPLED_SILENCE_FRAMES: u64 = 96_000;
const MOST_FRAMES_ONE_CALL_ENCODES: u64 = 60 * LARGEST_STEP_FRAMES;
const PATIENCE_WITH_A_SOURCE_AT_A_SAVE_FRAMES: u64 = 4_800;

enum Stretch {
    Sound(VecDeque<f32>),
    Silence(u64),
}

struct Line {
    layout: Option<AudioLayout>,
    resampler: Option<Resampler>,
    resampler_base: u64,
    produced: u64,
    taken: u64,
    waiting: VecDeque<Stretch>,
}

#[derive(Clone, Copy)]
struct Patience {
    silent_after: u64,
    latest_return: u64,
}

const WHILE_RUNNING: Patience = Patience {
    silent_after: SILENT_SOURCE_WAIT_FRAMES,
    latest_return: LATEST_RETURN_OF_A_SILENT_SOURCE_FRAMES,
};
const AT_A_SAVE: Patience = Patience {
    silent_after: PATIENCE_WITH_A_SOURCE_AT_A_SAVE_FRAMES,
    latest_return: 0,
};

fn frames_from_nanos(nanos: i64) -> u64 {
    let frames = i128::from(nanos.max(0)) * i128::from(AUDIO_FRAMES_PER_SECOND) / NANOS_PER_SECOND;
    u64::try_from(frames).unwrap_or(u64::MAX)
}

impl Line {
    fn new() -> Self {
        Self::starting_at(0)
    }

    fn starting_at(frame: u64) -> Self {
        Self {
            layout: None,
            resampler: None,
            resampler_base: frame,
            produced: frame,
            taken: frame,
            waiting: VecDeque::new(),
        }
    }

    fn accept(&mut self, samples: &[f32]) {
        let frames = (samples.len() / CHANNELS) as u64;
        let already_taken_as_silence = self.taken.saturating_sub(self.produced).min(frames);
        let skipped = usize::try_from(already_taken_as_silence).unwrap_or(usize::MAX);
        let kept = samples.iter().skip(skipped.saturating_mul(CHANNELS));
        if let Some(Stretch::Sound(queue)) = self.waiting.back_mut() {
            queue.extend(kept);
        } else {
            self.waiting
                .push_back(Stretch::Sound(kept.copied().collect()));
        }
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
        if not_yet_taken > 0 {
            self.waiting.push_back(Stretch::Silence(not_yet_taken));
        }
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
        let frames = frames.min(LARGEST_STEP);
        let wanted = frames * CHANNELS;
        let mut samples: Vec<f32> = Vec::with_capacity(wanted);
        while samples.len() < wanted {
            let missing = wanted - samples.len();
            let emptied = match self.waiting.front_mut() {
                None => break,
                Some(Stretch::Sound(queue)) => {
                    samples.extend(queue.drain(..missing.min(queue.len())));
                    queue.is_empty()
                }
                Some(Stretch::Silence(left)) => {
                    let silent = usize::try_from(*left)
                        .unwrap_or(usize::MAX)
                        .min(missing / CHANNELS);
                    samples.resize(samples.len() + silent * CHANNELS, 0.0);
                    *left -= silent as u64;
                    *left == 0 || silent == 0
                }
            };
            if emptied {
                self.waiting.pop_front();
            }
        }
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

    fn horizon(&self, now_frames: u64, patience: Patience) -> u64 {
        self.lines()
            .map(|line| {
                let delivers_nothing = line.produced + patience.silent_after < now_frames;
                if delivers_nothing {
                    now_frames.saturating_sub(patience.latest_return)
                } else {
                    line.produced
                }
            })
            .min()
            .unwrap_or(self.position)
            .min(now_frames)
            .max(self.position)
            .min(self.position + MOST_FRAMES_ONE_CALL_ENCODES)
    }

    pub(crate) fn retune(&mut self, settings: &AudioSettings) -> Result<(), RecorderError> {
        self.mixer
            .set_gain(0, settings.system_gain.unwrap_or(0.0))?;
        self.mixer
            .set_gain(1, settings.microphone_gain.unwrap_or(0.0))?;
        let position = self.position;
        for (line, wanted) in [
            (&mut self.system, settings.system_gain),
            (&mut self.microphone, settings.microphone_gain),
        ] {
            match (line.is_some(), wanted) {
                (false, Some(_)) => *line = Some(Line::starting_at(position)),
                (true, None) => *line = None,
                _ => {}
            }
        }
        Ok(())
    }

    fn tracks(&mut self, frames: usize) -> Result<[Vec<f32>; 3], RecorderError> {
        let frames = frames.min(LARGEST_STEP);
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
        Ok([mix, system, microphone])
    }

    fn step(&mut self, frames: usize) -> Result<Vec<Packet>, RecorderError> {
        let tracks = self.tracks(frames)?;
        let mut packets = Vec::new();
        for (track, samples) in tracks.iter().enumerate() {
            if let Some(encoder) = self.encoders.get_mut(track) {
                packets.extend(on_track(track, encoder.encode(samples)?));
            }
        }
        Ok(packets)
    }

    pub(crate) fn advance(&mut self, now_frames: u64) -> Result<Vec<Packet>, RecorderError> {
        self.advance_with(now_frames, WHILE_RUNNING)
    }

    pub(crate) fn advance_for_a_save(
        &mut self,
        now_frames: u64,
    ) -> Result<Vec<Packet>, RecorderError> {
        self.advance_with(now_frames, AT_A_SAVE)
    }

    fn advance_with(
        &mut self,
        now_frames: u64,
        patience: Patience,
    ) -> Result<Vec<Packet>, RecorderError> {
        let mut packets = Vec::new();
        if self.encoders.is_empty() {
            return Ok(packets);
        }
        let horizon = self.horizon(now_frames, patience);
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
        let end = now_frames.min(self.position + MOST_FRAMES_ONE_CALL_ENCODES);
        while self.position < end {
            let frames = (end - self.position).min(LARGEST_STEP_FRAMES);
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
    fn a_timestamp_years_ahead_is_a_gap_that_costs_no_memory() {
        let mut line = Line::new();
        line.push(&buffer(AudioSource::System, 480, 0.25), 0)
            .unwrap();
        let ten_years_in_nanos = 10 * 365 * 24 * 3_600 * 1_000_000_000_i64;

        line.push(&buffer(AudioSource::System, 480, 0.5), ten_years_in_nanos)
            .unwrap();

        let gap_in_frames = frames_from_nanos(ten_years_in_nanos);
        assert!(line.produced >= gap_in_frames);
        assert!(line.waiting.len() <= 3);
        let held: usize = line
            .waiting
            .iter()
            .map(|stretch| match stretch {
                Stretch::Sound(queue) => queue.len(),
                Stretch::Silence(_) => 0,
            })
            .sum();
        assert!(held <= 4 * 480 * CHANNELS, "{held} samples held");
        let first = line.take(usize::MAX);
        assert_eq!(first.len(), LARGEST_STEP * CHANNELS);
        assert!(
            first[200..400]
                .iter()
                .all(|sample| (sample - 0.25).abs() < 0.01)
        );
        assert!(first[4_000..].iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn one_call_encodes_a_minute_at_most_however_far_the_clock_jumped() {
        let mut stage = AudioStage::open(&settings(Tracks::MixOnly)).unwrap();
        let a_day = 24 * 3_600 * 48_000_u64;

        stage.advance(a_day).unwrap();
        assert_eq!(stage.position, MOST_FRAMES_ONE_CALL_ENCODES);
        stage.finish(a_day).unwrap();
        assert_eq!(stage.position, 2 * MOST_FRAMES_ONE_CALL_ENCODES);
    }

    #[test]
    fn a_save_does_not_wait_for_a_source_that_went_quiet() {
        let mut stage = AudioStage::open(&settings(Tracks::Three)).unwrap();
        for index in 0..200_i64 {
            stage
                .push(&buffer(AudioSource::System, 480, 0.1), index * 10_000_000)
                .unwrap();
        }
        stage
            .push(&buffer(AudioSource::Microphone, 80_000, 0.1), 0)
            .unwrap();
        stage.advance(96_000).unwrap();
        let while_running = stage.position;
        assert!(while_running <= 80_640, "{while_running}");

        stage.advance_for_a_save(96_000).unwrap();

        assert!(96_000 - stage.position <= 2_048, "{}", stage.position);
    }

    #[test]
    fn the_volumes_shape_the_mix_and_leave_the_separate_tracks_as_captured() {
        let quiet_microphone = AudioSettings {
            system_gain: Some(0.5),
            microphone_gain: Some(0.25),
            ..settings(Tracks::Three)
        };
        let mut stage = AudioStage::open(&quiet_microphone).unwrap();
        stage.system.as_mut().unwrap().accept(&[0.2; 960]);
        stage.microphone.as_mut().unwrap().accept(&[0.8; 960]);

        let [mix, system, microphone] = stage.tracks(480).unwrap();

        assert!(mix.iter().all(|sample| (sample - 0.3).abs() < 1e-6));
        assert!(system.iter().all(|sample| (sample - 0.2).abs() < 1e-6));
        assert!(microphone.iter().all(|sample| (sample - 0.8).abs() < 1e-6));
    }

    #[test]
    fn new_volumes_and_sources_take_effect_without_new_encoders() {
        let mut stage = AudioStage::open(&settings(Tracks::Three)).unwrap();
        stage.system.as_mut().unwrap().accept(&[0.4; 1_920]);
        stage.tracks(480).unwrap();
        let without_microphone = AudioSettings {
            system_gain: Some(0.5),
            microphone_gain: None,
            ..settings(Tracks::Three)
        };

        stage.retune(&without_microphone).unwrap();
        let [mix, ..] = stage.tracks(480).unwrap();

        assert!(stage.microphone.is_none());
        assert!(mix.iter().all(|sample| (sample - 0.2).abs() < 1e-6));
        stage.retune(&settings(Tracks::Three)).unwrap();
        assert_eq!(stage.microphone.as_ref().unwrap().produced, 960);
        assert_eq!(stage.stream_infos().count(), 3);
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
