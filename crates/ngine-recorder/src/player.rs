use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ngine_media::{
    AudioDecoder, Demuxer, PixelFormat, Rational, VideoDecoder, VideoDecoderOptions, VideoFrame,
};

use crate::error::RecorderError;
use crate::library::{MediaInfo, info_of, tracks_of};
use crate::sound::{CHANNELS, ClipMix, SAMPLE_RATE, SAMPLE_TIME, TimelineMix};
use crate::thumbnail::Picture;

const FRAME_TOLERANCE_MICROS: i64 = 500;
const PICTURES_AHEAD: usize = 4;
const SOUND_AHEAD_SAMPLES: usize = SAMPLE_RATE as usize / 4 * CHANNELS;
const IDLE_WAIT: Duration = Duration::from_millis(50);
const MICROS_PER_SECOND: i64 = 1_000_000;
const FALLBACK_FRAME_MICROS: i64 = 16_667;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerOptions {
    /// Pictures are shrunk to fit inside this many pixels.
    pub picture_bounds: (u32, u32),
    /// Decode on the GPU when the machine can.
    pub hardware: bool,
}

/// What a player shows right now.
#[derive(Clone, Debug)]
pub struct Playhead {
    pub position: Duration,
    pub playing: bool,
    /// How long one frame of the video is shown.
    pub frame_time: Duration,
    /// The picture to show, when it changed since the last call.
    pub picture: Option<Arc<Picture>>,
    pub failure: Option<String>,
}

#[derive(Clone, Copy)]
struct Work {
    seeking: bool,
    drained: bool,
    sound_over: bool,
}

#[derive(Clone, Copy)]
struct Media {
    has_sound: bool,
    length: i64,
    frame_micros: i64,
}

struct State {
    closing: bool,
    playing: bool,
    seek: Option<i64>,
    work: Work,
    media: Media,
    mix: ClipMix,
    pictures: VecDeque<(i64, Arc<Picture>)>,
    shown: Option<Arc<Picture>>,
    fresh: bool,
    sound: VecDeque<f32>,
    sound_at: i64,
    base: i64,
    played: i64,
    wall: Option<Instant>,
    feeds: usize,
    failure: Option<String>,
}

impl State {
    fn position(&self) -> i64 {
        let ahead = if self.feeds > 0 {
            self.played * MICROS_PER_SECOND / i64::from(SAMPLE_RATE)
        } else {
            self.wall.map_or(0, |since| {
                i64::try_from(since.elapsed().as_micros()).unwrap_or(i64::MAX)
            })
        };
        self.base.saturating_add(ahead).clamp(0, self.media.length)
    }

    fn rest_at(&mut self, micros: i64) {
        self.base = micros;
        self.played = 0;
        self.wall = (self.playing && self.feeds == 0).then(Instant::now);
    }

    fn wants_pictures(&self) -> bool {
        !self.work.drained && self.pictures.len() < PICTURES_AHEAD
    }

    fn wants_sound(&self) -> bool {
        self.feeds > 0
            && self.media.has_sound
            && !self.work.sound_over
            && self.sound.len() < SOUND_AHEAD_SAMPLES
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Plays one recording or clip: decoded pictures to show and mixed sound to
/// send to an output device. While a [`SoundFeed`] is attached, the position
/// follows the sound that was really played; otherwise it follows the clock.
pub struct Player {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    info: MediaInfo,
}

impl Player {
    pub fn open(video: &Path, options: PlayerOptions) -> Result<Self, RecorderError> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                closing: false,
                playing: false,
                seek: Some(0),
                work: Work {
                    seeking: true,
                    drained: false,
                    sound_over: false,
                },
                media: Media {
                    has_sound: false,
                    length: 0,
                    frame_micros: FALLBACK_FRAME_MICROS,
                },
                mix: ClipMix::default(),
                pictures: VecDeque::new(),
                shown: None,
                fresh: false,
                sound: VecDeque::new(),
                sound_at: 0,
                base: 0,
                played: 0,
                wall: None,
                feeds: 0,
                failure: None,
            }),
            wake: Condvar::new(),
        });
        let (opened, answer) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let path: PathBuf = video.to_owned();
        let worker = thread::Builder::new()
            .name("ngine-player".to_owned())
            .spawn(move || match Worker::open(&path, options, worker_shared) {
                Ok((mut worker, info)) => {
                    if opened.send(Ok(info)).is_ok() {
                        worker.run();
                    }
                }
                Err(error) => {
                    let _player_is_gone = opened.send(Err(error));
                }
            })
            .map_err(|error| RecorderError::file("start a thread to play", video, error))?;
        let info = answer.recv().map_err(|_| RecorderError::Stopped)??;
        Ok(Self {
            shared,
            worker: Some(worker),
            info,
        })
    }

    pub fn info(&self) -> &MediaInfo {
        &self.info
    }

    pub fn play(&self) {
        let mut state = self.shared.lock();
        if !state.playing {
            state.playing = true;
            let at = state.position();
            state.rest_at(at);
        }
        self.shared.wake.notify_all();
    }

    pub fn pause(&self) {
        let mut state = self.shared.lock();
        let at = state.position();
        state.playing = false;
        state.rest_at(at);
    }

    /// Shows the frame at `to` and continues from there.
    pub fn seek(&self, to: Duration) {
        let mut state = self.shared.lock();
        let micros = i64::try_from(to.as_micros())
            .unwrap_or(i64::MAX)
            .clamp(0, state.media.length);
        state.seek = Some(micros);
        state.work.seeking = true;
        state.pictures.clear();
        state.sound.clear();
        state.work.drained = false;
        state.rest_at(micros);
        self.shared.wake.notify_all();
    }

    pub fn set_mix(&self, mix: ClipMix) {
        self.shared.lock().mix = mix;
    }

    /// Moves the playhead and says what to show. Call it for every frame the
    /// window draws.
    pub fn advance(&self) -> Playhead {
        let mut state = self.shared.lock();
        let position = state.position();
        if !state.work.seeking {
            while state
                .pictures
                .front()
                .is_some_and(|(micros, _)| *micros <= position + FRAME_TOLERANCE_MICROS)
            {
                if let Some((_, picture)) = state.pictures.pop_front() {
                    state.shown = Some(picture);
                    state.fresh = true;
                }
            }
            let over = state.work.drained && state.pictures.is_empty();
            if state.playing && (over || position >= state.media.length) {
                state.playing = false;
                state.rest_at(position);
            }
        }
        let picture = if std::mem::take(&mut state.fresh) {
            state.shown.clone()
        } else {
            None
        };
        if state.wants_pictures() || state.wants_sound() {
            self.shared.wake.notify_all();
        }
        Playhead {
            position: Duration::from_micros(position.unsigned_abs()),
            playing: state.playing,
            frame_time: Duration::from_micros(state.media.frame_micros.unsigned_abs()),
            picture,
            failure: state.failure.clone(),
        }
    }

    /// The player's sound, for one output device.
    pub fn sound(&self) -> SoundFeed {
        let mut state = self.shared.lock();
        let at = state.position();
        state.feeds += 1;
        state.sound.clear();
        state.rest_at(at);
        SoundFeed {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.lock().closing = true;
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            eprintln!("ngine: the player's thread ended badly");
        }
    }
}

/// The sound of a [`Player`] as 48 kHz stereo. What it hands out moves the
/// player's position.
pub struct SoundFeed {
    shared: Arc<Shared>,
}

impl SoundFeed {
    /// Fills `out` with interleaved stereo samples, silence while the player
    /// is paused or has nothing ready.
    pub fn fill(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let mut state = self.shared.lock();
        if !state.playing || state.work.seeking {
            return;
        }
        let wanted = out.len() / CHANNELS;
        let position = state.base * i64::from(SAMPLE_RATE) / MICROS_PER_SECOND + state.played;
        if state.sound.is_empty() {
            if !state.media.has_sound || state.work.sound_over {
                state.played += i64::try_from(wanted).unwrap_or(0);
            }
            return;
        }
        let late = usize::try_from(position - state.sound_at).unwrap_or(0);
        let missed = (late * CHANNELS).min(state.sound.len());
        state.sound.drain(..missed);
        state.sound_at += i64::try_from(missed / CHANNELS).unwrap_or(0);
        let lead = usize::try_from(state.sound_at - position)
            .unwrap_or(0)
            .min(wanted);
        let taken = ((wanted - lead) * CHANNELS).min(state.sound.len());
        for (sample, played) in out[lead * CHANNELS..]
            .iter_mut()
            .zip(state.sound.drain(..taken))
        {
            *sample = played;
        }
        let frames = taken / CHANNELS;
        state.sound_at += i64::try_from(frames).unwrap_or(0);
        state.played += i64::try_from(lead + frames).unwrap_or(0);
        if state.wants_pictures() || state.wants_sound() {
            self.shared.wake.notify_all();
        }
    }
}

impl Drop for SoundFeed {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        let at = state.position();
        state.feeds = state.feeds.saturating_sub(1);
        state.rest_at(at);
    }
}

struct SoundLine {
    source: usize,
    decoder: AudioDecoder,
    time_base: Rational,
}

struct Sound {
    demuxer: Demuxer,
    lines: Vec<SoundLine>,
    separate: bool,
    mix: TimelineMix,
    over: bool,
}

impl Sound {
    fn seek(&mut self, target: i64) -> Result<(), RecorderError> {
        self.demuxer.seek(target)?;
        for line in &mut self.lines {
            line.decoder.reset();
        }
        self.mix
            .restart_from(Rational::MICROS.rescale(target, SAMPLE_TIME).unwrap_or(0));
        self.over = false;
        Ok(())
    }

    fn step(&mut self) -> Result<(), RecorderError> {
        let Some(packet) = self.demuxer.read()? else {
            for (index, line) in self.lines.iter_mut().enumerate() {
                for frame in line.decoder.finish()? {
                    self.mix.push(index, &frame, line.time_base)?;
                }
                self.mix.end_line(index);
            }
            self.over = true;
            return Ok(());
        };
        let heard = self
            .lines
            .iter_mut()
            .enumerate()
            .find(|(_, line)| line.source == packet.stream);
        if let Some((index, line)) = heard {
            for frame in line.decoder.decode(&packet)? {
                self.mix.push(index, &frame, line.time_base)?;
            }
        }
        Ok(())
    }

    fn publish(&mut self, state: &mut State) {
        while let Some((at, samples)) = self.mix.take() {
            if state.feeds == 0 {
                continue;
            }
            if state.sound.is_empty() {
                state.sound_at = at;
            }
            state.sound.extend(samples);
        }
        state.work.sound_over = self.over;
    }
}

struct Worker {
    shared: Arc<Shared>,
    demuxer: Demuxer,
    video: usize,
    video_time_base: Rational,
    decoder: VideoDecoder,
    sound: Option<Sound>,
    bounds: (u32, u32),
    last_micros: Option<i64>,
}

impl Worker {
    fn open(
        path: &Path,
        options: PlayerOptions,
        shared: Arc<Shared>,
    ) -> Result<(Self, MediaInfo), RecorderError> {
        let demuxer = Demuxer::open(path)?;
        let info = info_of(&demuxer)?;
        let tracks = tracks_of(demuxer.streams())?;
        let stream = &demuxer.streams()[tracks.video];
        let decoder = VideoDecoder::open(
            stream,
            VideoDecoderOptions {
                output: PixelFormat::Nv12,
                hardware: options.hardware,
            },
        )?;
        let separate = info.has_separate_sound();
        let heard: &[usize] = if separate {
            &tracks.audio[1..]
        } else {
            &tracks.audio[..tracks.audio.len().min(1)]
        };
        let mut lines = Vec::new();
        for &source in heard {
            let stream = &demuxer.streams()[source];
            lines.push(SoundLine {
                source,
                decoder: AudioDecoder::open(stream)?,
                time_base: stream.time_base,
            });
        }
        let sound = if lines.is_empty() {
            None
        } else {
            Some(Sound {
                demuxer: Demuxer::open(path)?,
                mix: TimelineMix::new(&vec![1.0; lines.len()]),
                lines,
                separate,
                over: false,
            })
        };
        {
            let mut state = shared.lock();
            state.media.has_sound = sound.is_some();
            state.media.length = i64::try_from(info.length.as_micros()).unwrap_or(i64::MAX);
        }
        let worker = Self {
            video: tracks.video,
            video_time_base: stream.time_base,
            shared,
            demuxer,
            decoder,
            sound,
            bounds: options.picture_bounds,
            last_micros: None,
        };
        Ok((worker, info))
    }

    fn run(&mut self) {
        loop {
            let (seek, wants) = {
                let mut state = self.shared.lock();
                loop {
                    if state.closing {
                        return;
                    }
                    if state.seek.is_some() || state.wants_pictures() || state.wants_sound() {
                        break;
                    }
                    state = self
                        .shared
                        .wake
                        .wait_timeout(state, IDLE_WAIT)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                if let Some(sound) = self.sound.as_mut().filter(|sound| sound.separate) {
                    sound.mix.set_gains(&state.mix.gains());
                }
                (state.seek.take(), state.wants_sound())
            };
            let outcome = match (seek, wants) {
                (Some(target), _) => self.seek(target),
                (None, true) => self.hear(),
                (None, false) => self.step(),
            };
            if let Err(error) = outcome {
                let mut state = self.shared.lock();
                state.failure = Some(error.to_string());
                state.work = Work {
                    seeking: false,
                    drained: true,
                    sound_over: true,
                };
            }
        }
    }

    fn micros(&self, frame: &VideoFrame) -> Option<i64> {
        frame
            .pts
            .and_then(|pts| self.video_time_base.rescale(pts, Rational::MICROS))
    }

    fn picture(&self, frame: &VideoFrame) -> Result<Arc<Picture>, RecorderError> {
        Picture::from_nv12(frame, self.bounds).map(Arc::new)
    }

    fn hear(&mut self) -> Result<(), RecorderError> {
        let Some(sound) = &mut self.sound else {
            return Ok(());
        };
        sound.step()?;
        let mut state = self.shared.lock();
        if state.seek.is_none() {
            sound.publish(&mut state);
        }
        Ok(())
    }

    fn next_frames(&mut self) -> Result<Option<Vec<VideoFrame>>, RecorderError> {
        loop {
            match self.demuxer.read()? {
                Some(packet) if packet.stream == self.video => {
                    return Ok(Some(self.decoder.decode(&packet)?));
                }
                Some(_) => {}
                None => return Ok(None),
            }
        }
    }

    fn seek(&mut self, target: i64) -> Result<(), RecorderError> {
        self.demuxer.seek(target)?;
        self.decoder.reset();
        self.last_micros = None;
        if let Some(sound) = &mut self.sound {
            sound.seek(target)?;
        }
        let mut before = None;
        let mut found: Vec<(i64, VideoFrame)> = Vec::new();
        let mut drained = false;
        while found.is_empty() && !drained {
            if self.shared.lock().seek.is_some() {
                return Ok(());
            }
            let frames = if let Some(frames) = self.next_frames()? {
                frames
            } else {
                drained = true;
                self.decoder.finish()?
            };
            for frame in frames {
                let micros = self.micros(&frame).unwrap_or(target);
                if micros >= target - FRAME_TOLERANCE_MICROS {
                    found.push((micros, frame));
                } else {
                    before = Some(frame);
                }
            }
        }
        let mut pictures = VecDeque::new();
        for (micros, frame) in &found {
            pictures.push_back((*micros, self.picture(frame)?));
        }
        let shown = match (pictures.pop_front(), before) {
            (Some((_, first)), _) => Some(first),
            (None, Some(last)) => Some(self.picture(&last)?),
            (None, None) => None,
        };
        let mut state = self.shared.lock();
        if state.seek.is_some() {
            return Ok(());
        }
        state.sound.clear();
        state.pictures = pictures;
        if shown.is_some() {
            state.shown = shown;
            state.fresh = true;
        }
        state.work = Work {
            seeking: false,
            drained,
            sound_over: false,
        };
        state.rest_at(target);
        Ok(())
    }

    fn step(&mut self) -> Result<(), RecorderError> {
        let (frames, drained) = match self.next_frames()? {
            Some(frames) => (frames, false),
            None => (self.decoder.finish()?, true),
        };
        let mut pictures = Vec::with_capacity(frames.len());
        let mut frame_micros = None;
        for frame in &frames {
            if let Some(micros) = self.micros(frame) {
                let since_last = self.last_micros.map(|last| micros - last);
                frame_micros = since_last
                    .filter(|step| (1..MICROS_PER_SECOND).contains(step))
                    .or(frame_micros);
                self.last_micros = Some(micros);
                pictures.push((micros, self.picture(frame)?));
            }
        }
        let mut state = self.shared.lock();
        if state.seek.is_some() {
            return Ok(());
        }
        if let Some(frame_micros) = frame_micros {
            state.media.frame_micros = frame_micros;
        }
        state.pictures.extend(pictures);
        state.work.drained = drained;
        Ok(())
    }
}
