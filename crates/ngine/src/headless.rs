use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ngine_capture::{Capturer, Region, Target, Targets};
use ngine_recorder::replay::{BackendChoice, ReplayLength, SaveRange};
use ngine_recorder::{
    AudioCodec, AudioSettings, CaptureSetup, Quality, Recorder, RecorderConfig, RecorderEvent,
    RecordingStart, ReplaySaveRequest, ReplaySetup, Tracks, VideoCodec, VideoSettings,
    working_video_encoders,
};

use crate::cli::parse_length;

pub const USAGE: &str = "\
Recording without the window:
  ngine record --seconds N --out FILE [options]
                            Record for N seconds into FILE
  ngine replay --seconds N --last LENGTH --out FOLDER [options]
                            Keep instant replay on for N seconds, then save
                            the last LENGTH into FOLDER

Options for both:
  --screen N | --window N | --app N
                            What to record, numbered from 1 as --list shows
                            them (default: --screen 1)
  --region X,Y,W,H          Record only this part of the screen, in its
                            pixels from the top-left corner
  --list                    List what can be recorded and stop
  --codec h264|hevc|av1     (default: h264)
  --encoder NAME            Use this FFmpeg encoder instead of the first
                            working one for the codec
  --quality low|medium|high|highest
                            (default: high)
  --fps N                   (default: 60)
  --no-system-sound         Leave system sound out
  --no-microphone           Leave the microphone out
  --all-sound               Record everything the computer plays, also when
                            a window or an app is recorded
  --mix-only                Write one mixed audio track instead of three
  --marker-at SECONDS       Drop a marker that long into a recording";

const POLL: Duration = Duration::from_millis(50);
const SAVE_WAIT: Duration = Duration::from_mins(2);
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Record,
    Replay,
}

#[derive(Clone, Copy)]
enum Kind {
    Screen,
    Window,
    App,
}

struct Request {
    seconds: u64,
    out: Option<PathBuf>,
    last: Option<Duration>,
    kind: Kind,
    number: usize,
    region: Option<Region>,
    list: bool,
    video: VideoSettings,
    audio: AudioSettings,
    all_sound: bool,
    marker_at: Option<Duration>,
}

fn value<'a>(
    name: &str,
    arguments: &mut impl Iterator<Item = &'a String>,
) -> Result<&'a str, String> {
    arguments
        .next()
        .map(String::as_str)
        .ok_or_else(|| format!("{name} needs a value"))
}

const REGION_OF_A_SCREEN: &str = "--region is a part of a screen, so it goes with --screen";

fn region(text: &str) -> Result<Region, String> {
    let parts: Vec<u32> = text
        .split(',')
        .map(|part| number("--region", part.trim()))
        .collect::<Result<_, _>>()?;
    match parts[..] {
        [x, y, width, height] if width > 0 && height > 0 => Ok(Region::new(x, y, width, height)),
        _ => Err(format!(
            "\"{text}\" is not a region; give X,Y,W,H with a width and height above zero"
        )),
    }
}

fn number<T: std::str::FromStr>(name: &str, text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("\"{text}\" is not a number for {name}"))
}

impl Request {
    fn parse(mode: Mode, arguments: &[String]) -> Result<Self, String> {
        let mut request = Self {
            seconds: 0,
            out: None,
            last: None,
            kind: Kind::Screen,
            number: 1,
            region: None,
            list: false,
            video: VideoSettings {
                codec: VideoCodec::H264,
                quality: Quality::High,
                frame_rate: 60,
                encoder: None,
            },
            audio: AudioSettings {
                system_gain: Some(1.0),
                microphone_gain: Some(1.0),
                tracks: Tracks::Three,
                codec: AudioCodec::Aac,
            },
            all_sound: false,
            marker_at: None,
        };
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            let name = argument.as_str();
            match name {
                "--seconds" => request.seconds = number(name, value(name, &mut arguments)?)?,
                "--out" => request.out = Some(PathBuf::from(value(name, &mut arguments)?)),
                "--last" => {
                    let text = value(name, &mut arguments)?;
                    let length = parse_length(text)
                        .ok_or_else(|| format!("\"{text}\" is not a length, such as 30s"))?;
                    request.last = Some(length);
                }
                "--screen" | "--window" | "--app" => {
                    request.kind = match name {
                        "--window" => Kind::Window,
                        "--app" => Kind::App,
                        _ => Kind::Screen,
                    };
                    request.number = number(name, value(name, &mut arguments)?)?;
                }
                "--region" => request.region = Some(region(value(name, &mut arguments)?)?),
                "--list" => request.list = true,
                "--codec" => {
                    request.video.codec = match value(name, &mut arguments)? {
                        "h264" => VideoCodec::H264,
                        "hevc" => VideoCodec::Hevc,
                        "av1" => VideoCodec::Av1,
                        other => return Err(format!("\"{other}\" is not h264, hevc or av1")),
                    };
                }
                "--encoder" => {
                    request.video.encoder = Some(value(name, &mut arguments)?.to_owned());
                }
                "--quality" => {
                    request.video.quality = match value(name, &mut arguments)? {
                        "low" => Quality::Low,
                        "medium" => Quality::Medium,
                        "high" => Quality::High,
                        "highest" => Quality::Highest,
                        other => {
                            return Err(format!("\"{other}\" is not low, medium, high or highest"));
                        }
                    };
                }
                "--fps" => request.video.frame_rate = number(name, value(name, &mut arguments)?)?,
                "--no-system-sound" => request.audio.system_gain = None,
                "--no-microphone" => request.audio.microphone_gain = None,
                "--all-sound" => request.all_sound = true,
                "--mix-only" => request.audio.tracks = Tracks::MixOnly,
                "--marker-at" => {
                    let seconds: f64 = number(name, value(name, &mut arguments)?)?;
                    request.marker_at = Duration::try_from_secs_f64(seconds).ok();
                }
                other => return Err(format!("ngine does not know the option \"{other}\"")),
            }
        }
        if !request.list {
            if request.seconds == 0 {
                return Err("--seconds needs a number above zero".into());
            }
            if request.out.is_none() {
                return Err("--out says where to write".into());
            }
            if mode == Mode::Replay && request.last.is_none() {
                return Err("--last says how much to save, such as 30s".into());
            }
        }
        Ok(request)
    }

    fn target(&self, targets: &Targets) -> Result<Target, String> {
        if self.region.is_some() && !matches!(self.kind, Kind::Screen) {
            return Err(REGION_OF_A_SCREEN.into());
        }
        let index = self
            .number
            .checked_sub(1)
            .ok_or("targets are numbered from 1")?;
        let target = match self.kind {
            Kind::Screen => targets.screens.get(index).cloned().map(Target::from),
            Kind::Window => targets.windows.get(index).cloned().map(Target::from),
            Kind::App => targets.apps.get(index).cloned().map(Target::from),
        };
        let target = target
            .ok_or_else(|| format!("there is no target number {}; see --list", self.number))?;
        Ok(match self.region {
            Some(region) => target.with_region(region),
            None => target,
        })
    }
}

fn print_targets(targets: &Targets) {
    for (index, screen) in targets.screens.iter().enumerate() {
        let size = screen.size;
        println!(
            "screen {}: {} ({} x {})",
            index + 1,
            screen.name,
            size.width,
            size.height
        );
    }
    for (index, window) in targets.windows.iter().enumerate() {
        println!(
            "window {}: {} ({})",
            index + 1,
            window.title,
            window.app_name
        );
    }
    for (index, app) in targets.apps.iter().enumerate() {
        println!("app {}: {}", index + 1, app.name);
    }
}

fn describe(event: &RecorderEvent) -> Option<String> {
    Some(match event {
        RecorderEvent::RecordingStarted { file } => format!("recording into {}", file.display()),
        RecorderEvent::RecordingFinished { file, length, .. } => {
            format!("wrote {} ({:.3} s)", file.display(), length.as_secs_f64())
        }
        RecorderEvent::RecordingFailed {
            file,
            length,
            error,
        } => format!(
            "FAILED writing {} after {:.3} s: {error}",
            file.display(),
            length.as_secs_f64()
        ),
        RecorderEvent::RecordingDiscarded { .. } => "no frame arrived, so no file".into(),
        RecorderEvent::MarkerAdded { at } => format!("marker at {:.3} s", at.as_secs_f64()),
        RecorderEvent::ReplayStarted => "instant replay is on".into(),
        RecorderEvent::ReplaySaved { file, length } => {
            format!("saved {} ({:.3} s)", file.display(), length.as_secs_f64())
        }
        RecorderEvent::ReplaySaveFailed { error } => format!("FAILED saving the replay: {error}"),
        RecorderEvent::ReplayStorageFailing { error } => {
            format!("the replay buffer has trouble: {error}")
        }
        RecorderEvent::CaptureInterrupted(error) => format!("capture was interrupted: {error}"),
        RecorderEvent::CaptureResumed => "capture runs again".into(),
        RecorderEvent::CaptureLost(Some(error)) => format!("FAILED: capture was lost: {error}"),
        RecorderEvent::CaptureLost(None) => "FAILED: capture was ended from outside".into(),
        RecorderEvent::Failed(error) => format!("FAILED: {error}"),
        _ => return None,
    })
}

struct Run {
    recorder: Recorder,
    failed: bool,
    ended: bool,
}

impl Run {
    fn show(&mut self, event: &RecorderEvent) {
        if let Some(line) = describe(event) {
            self.failed |= line.starts_with("FAILED");
            println!("{line}");
        }
        self.ended |= matches!(event, RecorderEvent::Stopped);
    }

    fn wait_for_the_first_frame(&mut self) {
        let asked = Instant::now();
        while asked.elapsed() < FIRST_FRAME_WAIT
            && !self.ended
            && self.recorder.status().frames_captured == 0
        {
            if let Some(event) = self.recorder.wait_event(POLL) {
                self.show(&event);
            }
        }
    }

    fn run_for(&mut self, length: Duration, marker_at: Option<Duration>) {
        self.wait_for_the_first_frame();
        let started = Instant::now();
        let mut marker_at = marker_at;
        while started.elapsed() < length && !self.ended {
            if let Some(event) = self.recorder.wait_event(POLL) {
                self.show(&event);
            }
            if marker_at.is_some_and(|at| started.elapsed() >= at) {
                marker_at = None;
                if let Err(error) = self.recorder.add_marker() {
                    println!("the marker was not added: {error}");
                }
            }
        }
    }

    fn wait_for_the_save(&mut self) {
        let started = Instant::now();
        while started.elapsed() < SAVE_WAIT && !self.ended {
            let Some(event) = self.recorder.wait_event(POLL) else {
                continue;
            };
            self.show(&event);
            if matches!(
                event,
                RecorderEvent::ReplaySaved { .. } | RecorderEvent::ReplaySaveFailed { .. }
            ) {
                return;
            }
        }
    }
}

pub fn run(command: &str, arguments: &[String]) -> Result<(), String> {
    let mode = if command == "replay" {
        Mode::Replay
    } else {
        Mode::Record
    };
    let request = Request::parse(mode, arguments).map_err(|error| format!("{error}\n\n{USAGE}"))?;
    let capturer = Capturer::system().map_err(|error| error.to_string())?;
    let targets = capturer
        .lister()
        .ok_or("this system only offers its own picker, which needs the window")?
        .targets()
        .map_err(|error| error.to_string())?;
    if request.list {
        print_targets(&targets);
        return Ok(());
    }
    let target = request.target(&targets)?;
    let out = request.out.clone().ok_or("--out says where to write")?;
    println!("capturing {}", target.label());
    let config = RecorderConfig {
        video: request.video.clone(),
        audio: request.audio,
        encoders: working_video_encoders(),
    };
    let setup = CaptureSetup {
        target,
        show_cursor: true,
        app_sound_only: !request.all_sound,
        sound_of: None,
        microphone: None,
    };
    let recorder =
        Recorder::start(Arc::new(capturer), setup, &config).map_err(|error| error.to_string())?;
    let length = Duration::from_secs(request.seconds);
    let mut run = Run {
        recorder,
        failed: false,
        ended: false,
    };
    if let (Mode::Replay, Some(last)) = (mode, request.last) {
        let longest = ReplayLength::LONGEST.get();
        let kept = length.clamp(ReplayLength::SHORTEST.get(), longest);
        let setup = ReplaySetup {
            length: ReplayLength::new(kept).map_err(|error| error.to_string())?,
            backend: BackendChoice::Memory,
            disk_folder: None,
            clear_after_save: false,
        };
        run.recorder
            .set_replay(Some(setup))
            .map_err(|error| error.to_string())?;
        run.run_for(length, None);
        let save = ReplaySaveRequest {
            range: SaveRange::Last(last),
            folder: out,
            name: "Replay".to_owned(),
        };
        run.recorder
            .save_replay(save)
            .map_err(|error| error.to_string())?;
        run.wait_for_the_save();
    } else {
        let start = RecordingStart {
            file: out,
            include_buffered: None,
        };
        run.recorder
            .start_recording(start)
            .map_err(|error| error.to_string())?;
        run.run_for(length, request.marker_at);
    }
    let status = run.recorder.status();
    println!(
        "encoder {} ({}), {} frames captured, {} repeated",
        status.encoder.as_deref().unwrap_or("none"),
        if status.zero_copy {
            "reads the GPU surface"
        } else {
            "copies each frame on the CPU"
        },
        status.frames_captured,
        status.frames_repeated
    );
    let Run {
        recorder,
        mut failed,
        ..
    } = run;
    for event in recorder.stop() {
        if let Some(line) = describe(&event) {
            failed |= line.starts_with("FAILED");
            println!("{line}");
        }
    }
    if failed {
        Err("the run did not finish cleanly".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<Request, String> {
        let arguments: Vec<String> = line.split(' ').map(String::from).collect();
        Request::parse(Mode::Record, &arguments)
    }

    #[test]
    fn a_region_is_four_numbers_and_only_goes_with_a_screen() {
        let request = parsed("--seconds 3 --out a.mp4 --region 10,20,640,360").expect("parses");
        assert_eq!(request.region, Some(Region::new(10, 20, 640, 360)));
        assert!(parsed("--seconds 3 --out a.mp4 --region 10,20,640").is_err());
        assert!(parsed("--seconds 3 --out a.mp4 --region 10,20,0,360").is_err());
        let windowed =
            parsed("--seconds 3 --out a.mp4 --window 1 --region 0,0,8,8").expect("parses");
        assert_eq!(
            windowed.target(&Targets::default()).err(),
            Some(REGION_OF_A_SCREEN.to_owned())
        );
    }
}
