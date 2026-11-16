use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use ngine_recorder::{
    ClipMix, Cut, ExportControl, ExportRequest, ExportSummary, MediaInfo, Player, PlayerOptions,
    VideoCodec, export_clip, read_markers, unused_path, working_video_encoders,
};
use rune::Pixels;

use crate::library::sample_picture;
use crate::speaker::Speaker;
use crate::work::{Job, Progress};

pub const PREVIEW_PIXELS: (u32, u32) = (1280, 720);
pub const LOUDEST_PERCENT: u16 = 200;
pub const PERCENT_STEP: u16 = 10;
pub const FULL_VOLUME: u8 = 100;
const FALLBACK_FRAME: Duration = Duration::from_micros(16_667);
pub const CONTROLS_LINGER_SECONDS: f64 = 2.5;

/// How the video fills the window while it is watched full screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Watching {
    /// When the pointer last moved, on the app's clock.
    pub moved_at: f64,
    /// Whether the pointer rests on the controls, which keeps them shown.
    pub on_controls: bool,
    /// Whether ngine also took the window full screen, so it gives it back.
    pub took_the_screen: bool,
}

/// One sound source of the recording, as the clip will hold it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub on: bool,
    pub percent: u16,
}

impl Level {
    const AS_RECORDED: Self = Self {
        on: true,
        percent: 100,
    };

    fn gain(self) -> f32 {
        if self.on {
            f32::from(self.percent.min(LOUDEST_PERCENT)) / 100.0
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    System,
    Microphone,
}

type LiveExport = (Arc<ExportControl>, Job<Result<ExportSummary, String>>);

pub enum Exporting {
    Idle,
    Running { done: f32, live: Option<LiveExport> },
    Done { file: PathBuf, length: Duration },
    Failed(String),
}

pub enum News {
    Exported(Duration),
    ExportFailed,
}

/// One recording open for trimming: where its in and out points are, what
/// the preview shows, and the export that is running.
pub struct Editor {
    pub file: PathBuf,
    pub name: String,
    pub info: MediaInfo,
    pub markers: Vec<Duration>,
    pub start: Duration,
    pub end: Duration,
    pub position: Duration,
    pub playing: bool,
    pub system: Level,
    pub microphone: Level,
    pub cut: Cut,
    pub exporting: Exporting,
    pub picture: Pixels,
    pub failure: Option<String>,
    pub hovered: bool,
    pub watching: Option<Watching>,
    frame: Duration,
    awaiting_picture: bool,
    volume_percent: Arc<AtomicU8>,
    speaker: Option<Speaker>,
    player: Option<Player>,
}

fn name_of(file: &Path) -> String {
    file.file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Editor {
    pub fn open(file: &Path) -> Result<Self, String> {
        let options = PlayerOptions {
            picture_bounds: PREVIEW_PIXELS,
            hardware: true,
        };
        let player = Player::open(file, options).map_err(|error| error.to_string())?;
        let info = player.info().clone();
        let mut markers = read_markers(file).unwrap_or_default();
        markers.retain(|marker| *marker <= info.length);
        let volume_percent = Arc::new(AtomicU8::new(FULL_VOLUME));
        Ok(Self {
            file: file.to_owned(),
            name: name_of(file),
            markers,
            start: Duration::ZERO,
            end: info.length,
            position: Duration::ZERO,
            playing: false,
            system: Level::AS_RECORDED,
            microphone: Level::AS_RECORDED,
            cut: Cut::Fast,
            exporting: Exporting::Idle,
            picture: Pixels::new(),
            failure: None,
            hovered: false,
            watching: None,
            frame: FALLBACK_FRAME,
            awaiting_picture: true,
            speaker: Speaker::open(player.sound(), Arc::clone(&volume_percent)),
            volume_percent,
            player: Some(player),
            info,
        })
    }

    /// A made-up recording with nothing behind it, for snapshots and tests.
    pub fn sample(file: &Path) -> Self {
        let picture = Pixels::new();
        let painted = sample_picture((640, 360), [38, 70, 120]);
        picture.set(painted.width, painted.height, painted.rgba);
        Self {
            file: file.to_owned(),
            name: name_of(file),
            info: MediaInfo {
                length: Duration::from_secs(125),
                width: 2560,
                height: 1440,
                video_codec: Some(VideoCodec::H264),
                audio_tracks: 3,
            },
            markers: [12, 33, 80].map(Duration::from_secs).into(),
            start: Duration::from_secs(20),
            end: Duration::from_secs(62),
            position: Duration::from_secs(31),
            playing: false,
            system: Level::AS_RECORDED,
            microphone: Level::AS_RECORDED,
            cut: Cut::Fast,
            exporting: Exporting::Idle,
            picture,
            failure: None,
            hovered: false,
            watching: None,
            frame: FALLBACK_FRAME,
            awaiting_picture: false,
            volume_percent: Arc::new(AtomicU8::new(FULL_VOLUME)),
            speaker: None,
            player: None,
        }
    }

    /// Waits until the frame under the playhead is decoded, up to `longest`.
    pub fn wait_for_the_picture(&mut self, longest: Duration) {
        let asked = std::time::Instant::now();
        while self.awaiting_picture && asked.elapsed() < longest {
            self.follow_the_player();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Whether the transport and timeline show over the video watched full
    /// screen at `clock`: always while paused, and while playing until the
    /// pointer has been still for a moment away from them.
    pub fn controls_shown(&self, clock: f64) -> bool {
        self.watching.is_none_or(|watching| {
            !self.playing
                || watching.on_controls
                || clock - watching.moved_at < CONTROLS_LINGER_SECONDS
        })
    }

    /// Whether the preview has no sound device to play through.
    pub fn plays_silently(&self) -> bool {
        self.player.is_some() && self.speaker.is_none()
    }

    /// How loud the preview plays, from 0 to 100. The clip is not changed.
    pub fn volume(&self) -> u8 {
        self.volume_percent.load(Ordering::Relaxed)
    }

    pub fn set_volume(&mut self, percent: u8) {
        self.volume_percent
            .store(percent.min(FULL_VOLUME), Ordering::Relaxed);
    }

    pub fn length(&self) -> Duration {
        self.info.length
    }

    pub fn clip_length(&self) -> Duration {
        (self.end.saturating_sub(self.start) + self.frame).min(self.length())
    }

    pub fn mix(&self) -> ClipMix {
        ClipMix {
            system: self.system.gain(),
            microphone: self.microphone.gain(),
        }
    }

    pub fn level(&self, source: Source) -> Level {
        match source {
            Source::System => self.system,
            Source::Microphone => self.microphone,
        }
    }

    pub fn set_level(&mut self, source: Source, change: impl FnOnce(&mut Level)) {
        change(match source {
            Source::System => &mut self.system,
            Source::Microphone => &mut self.microphone,
        });
        if let Some(player) = &self.player {
            player.set_mix(self.mix());
        }
    }

    pub fn is_exporting(&self) -> bool {
        matches!(self.exporting, Exporting::Running { .. })
    }

    pub fn pause(&mut self) {
        self.playing = false;
        if let Some(player) = &self.player {
            player.pause();
        }
    }

    pub fn toggle_play(&mut self) {
        if self.playing {
            return self.pause();
        }
        if self.position + self.frame > self.end || self.position + self.frame > self.length() {
            self.seek(self.start);
        }
        self.playing = true;
        if let Some(player) = &self.player {
            player.play();
        }
    }

    pub fn seek(&mut self, to: Duration) {
        self.position = to.min(self.length());
        if let Some(player) = &self.player {
            player.seek(self.position);
            self.awaiting_picture = true;
        }
    }

    pub fn step(&mut self, frames: i32) {
        self.pause();
        let by = self.frame * frames.unsigned_abs();
        self.seek(if frames < 0 {
            self.position.saturating_sub(by)
        } else {
            self.position + by
        });
    }

    pub fn skip(&mut self, seconds: i32) {
        let by = Duration::from_secs(u64::from(seconds.unsigned_abs()));
        self.seek(if seconds < 0 {
            self.position.saturating_sub(by)
        } else {
            self.position + by
        });
    }

    /// Moves the in point, never past the out point, and shows that frame.
    pub fn set_start(&mut self, to: Duration) {
        self.start = to.min(self.end.saturating_sub(self.frame));
        self.pause();
        self.seek(self.start);
    }

    /// Moves the out point, never before the in point, and shows that frame.
    pub fn set_end(&mut self, to: Duration) {
        self.end = to.clamp(
            self.start + self.frame,
            self.length().max(self.start + self.frame),
        );
        self.pause();
        self.seek(self.end);
    }

    /// Starts writing the clip into `folder` under `name`, on another thread.
    pub fn export(&mut self, folder: &Path, name: &str) {
        if self.is_exporting() {
            return;
        }
        let control = Arc::new(ExportControl::new());
        let shared = Arc::clone(&control);
        let (source, folder, name) = (self.file.clone(), folder.to_owned(), name.to_owned());
        let (start, end, cut, mix) = (self.start, self.end, self.cut, self.mix());
        let job = Job::spawn("ngine-export", move || {
            let request = ExportRequest {
                source,
                target: unused_path(&folder, &name),
                start,
                end,
                cut,
                mix,
                encoders: match cut {
                    Cut::Fast => Vec::new(),
                    Cut::Exact => working_video_encoders(),
                },
            };
            export_clip(&request, &shared).map_err(|error| error.to_string())
        });
        self.exporting = match job {
            Ok(job) => Exporting::Running {
                done: 0.0,
                live: Some((control, job)),
            },
            Err(error) => Exporting::Failed(error.to_string()),
        };
    }

    pub fn cancel_export(&mut self) {
        if let Exporting::Running {
            live: Some((control, _)),
            ..
        } = &self.exporting
        {
            control.cancel();
        }
    }

    pub fn wants_ticks(&self) -> bool {
        let live_export = matches!(self.exporting, Exporting::Running { live: Some(_), .. });
        self.playing || self.awaiting_picture || live_export
    }

    fn follow_the_player(&mut self) -> bool {
        let Some(player) = &self.player else {
            return false;
        };
        let playhead = player.advance();
        let mut changed = false;
        self.frame = playhead.frame_time;
        if let Some(picture) = playhead.picture {
            let picture = Arc::unwrap_or_clone(picture);
            self.picture
                .set(picture.width, picture.height, picture.rgba);
            self.awaiting_picture = false;
        }
        if playhead.failure.is_some() && self.failure != playhead.failure {
            self.failure = playhead.failure;
            self.awaiting_picture = false;
            changed = true;
        }
        if self.playing {
            changed |= self.position != playhead.position;
            self.position = playhead.position;
            let past_the_out_point = self.position >= self.end && self.end < self.length();
            if past_the_out_point || !playhead.playing {
                self.pause();
                changed = true;
            }
        }
        changed
    }

    fn follow_the_export(&mut self) -> Option<News> {
        let Exporting::Running {
            done,
            live: Some((control, job)),
        } = &mut self.exporting
        else {
            return None;
        };
        *done = control.progress();
        let outcome = match job.progress() {
            Progress::Running => return None,
            Progress::Done(outcome) => outcome,
            Progress::Lost => Err("the export stopped without saying why".into()),
        };
        Some(match outcome {
            Ok(summary) => {
                self.exporting = Exporting::Done {
                    file: summary.file.clone(),
                    length: summary.length,
                };
                News::Exported(summary.length)
            }
            Err(error) => {
                self.exporting = Exporting::Failed(error);
                News::ExportFailed
            }
        })
    }

    /// Moves the playhead and the export along. Returns whether the window
    /// has something new to draw, and what happened.
    pub fn tick(&mut self) -> (bool, Option<News>) {
        let was_exporting = self.is_exporting();
        let moved = self.follow_the_player();
        let news = self.follow_the_export();
        (moved || was_exporting, news)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    #[ignore = "needs a sound device and a real recording: set NGINE_REAL_RECORDING to its path"]
    fn a_real_recording_plays_through_the_sound_device_at_the_speed_of_its_sound() {
        let file = PathBuf::from(std::env::var_os("NGINE_REAL_RECORDING").expect("a recording"));
        let mut editor = Editor::open(&file).expect("the recording opens");
        assert!(!editor.plays_silently(), "no sound device was opened");
        editor.wait_for_the_picture(Duration::from_secs(20));
        assert!(editor.picture.size().0 > 0);
        editor.toggle_play();
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(1_500) {
            editor.tick();
            std::thread::sleep(Duration::from_millis(8));
        }
        let played = editor.position.as_secs_f64();
        let waited = started.elapsed().as_secs_f64();
        println!("the device played {played:.3} s of sound in {waited:.3} s");
        assert!((played - waited).abs() < 0.3);
        editor.pause();
        let rested = editor.position;
        std::thread::sleep(Duration::from_millis(200));
        editor.tick();
        assert_eq!(editor.position, rested);

        let folder = std::env::temp_dir().join(format!("ngine-editor-{}", std::process::id()));
        editor.set_start(Duration::from_secs(1));
        editor.set_end(Duration::from_secs(3));
        editor.cut = Cut::Exact;
        editor.export(&folder, "Clip");
        let asked = Instant::now();
        let news = loop {
            if let (_, Some(news)) = editor.tick() {
                break news;
            }
            assert!(
                asked.elapsed() < Duration::from_secs(60),
                "the export never ended"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let News::Exported(length) = news else {
            panic!("the export failed");
        };
        println!("exported {length:?} in {:?}", asked.elapsed());
        assert!((length.as_secs_f64() - 2.0).abs() < 0.1);
        assert!(folder.join("Clip.mp4").exists());
        std::fs::remove_dir_all(&folder).expect("temp folder removed");
    }
}
