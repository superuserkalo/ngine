use std::collections::BTreeSet;
use std::error::Error;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ngine_capture::{
    AudioBuffer, AudioSource, Capturer, EndReason, Event, Microphone, PickerOptions, PixelFormat,
    RecvError, RestoreToken, Session, SessionOptions, SystemAudio, Target, Targets, Timestamp,
    VideoFrame,
};

const RESTORE_TOKEN_VARIABLE: &str = "NGINE_RESTORE_TOKEN";

const USAGE: &str = "usage: capture_stats [screen|window|app] [NUMBER] \
[--seconds N] [--no-cursor] [--no-microphone] [--no-sound] [--all-sound] [--nv12] [--list]";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Screen,
    Window,
    App,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sound {
    OfTheTarget,
    Everything,
    Off,
}

struct Request {
    kind: Kind,
    number: usize,
    seconds: u64,
    cursor: bool,
    microphone: Microphone,
    sound: Sound,
    pixel_format: PixelFormat,
    list_only: bool,
}

impl Request {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn Error>> {
        let mut request = Self {
            kind: Kind::Screen,
            number: 1,
            seconds: 5,
            cursor: true,
            microphone: Microphone::SystemDefault,
            sound: Sound::OfTheTarget,
            pixel_format: PixelFormat::Bgra8,
            list_only: false,
        };
        let mut arguments = arguments.peekable();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "screen" => request.kind = Kind::Screen,
                "window" => request.kind = Kind::Window,
                "app" => request.kind = Kind::App,
                "--seconds" => {
                    request.seconds = arguments.next().ok_or(USAGE)?.parse()?;
                }
                "--no-cursor" => request.cursor = false,
                "--no-microphone" => request.microphone = Microphone::Off,
                "--no-sound" => request.sound = Sound::Off,
                "--all-sound" => request.sound = Sound::Everything,
                "--nv12" => request.pixel_format = PixelFormat::Nv12,
                "--list" => request.list_only = true,
                number => request.number = number.parse().map_err(|_| USAGE)?,
            }
        }
        Ok(request)
    }

    fn target(&self, targets: &Targets) -> Result<Target, Box<dyn Error>> {
        let index = self
            .number
            .checked_sub(1)
            .ok_or("targets are numbered from 1")?;
        let target = match self.kind {
            Kind::Screen => targets.screens.get(index).cloned().map(Target::from),
            Kind::Window => targets.windows.get(index).cloned().map(Target::from),
            Kind::App => targets.apps.get(index).cloned().map(Target::from),
        };
        target.ok_or_else(|| "no target has that number".into())
    }

    fn options(&self, target: &Target) -> SessionOptions {
        SessionOptions::new()
            .with_cursor(self.cursor)
            .with_pixel_format(self.pixel_format)
            .with_system_audio(match self.sound {
                Sound::OfTheTarget => SystemAudio::of(target),
                Sound::Everything => SystemAudio::Everything,
                Sound::Off => SystemAudio::Off,
            })
            .with_microphone(self.microphone.clone())
    }
}

#[derive(Default)]
struct Span {
    first: Option<Timestamp>,
    last: Option<Timestamp>,
}

impl Span {
    fn include(&mut self, timestamp: Timestamp) {
        self.first.get_or_insert(timestamp);
        self.last = Some(timestamp);
    }

    fn seconds(&self) -> f64 {
        self.first.zip(self.last).map_or(0.0, |(first, last)| {
            last.saturating_duration_since(first).as_secs_f64()
        })
    }
}

#[derive(Default)]
struct VideoStats {
    frames: u32,
    span: Span,
    shapes: BTreeSet<String>,
    last: Option<VideoFrame>,
}

impl VideoStats {
    fn include(&mut self, frame: &VideoFrame) {
        self.frames += 1;
        self.span.include(frame.timestamp());
        self.shapes.insert(format!(
            "{}x{} {:?}",
            frame.size().width,
            frame.size().height,
            frame.format()
        ));
        self.last = Some(frame.clone());
    }

    fn cpu_copy_of_the_last_frame(&self) -> String {
        match self.last.as_ref().map(VideoFrame::to_cpu) {
            Some(Ok(pixels)) => format!(
                "{}x{} {:?}, {} bytes",
                pixels.size().width,
                pixels.size().height,
                pixels.format(),
                pixels.data().len()
            ),
            Some(Err(error)) => format!("failed: {error}"),
            None => "no frame to copy".to_owned(),
        }
    }

    fn print(&self) {
        let between_frames = f64::from(self.frames.saturating_sub(1));
        let seconds = self.span.seconds();
        let rate = if seconds > 0.0 {
            between_frames / seconds
        } else {
            0.0
        };
        println!(
            "video:        {} frames over {seconds:.2} s, {rate:.1} per second",
            self.frames
        );
        println!("  shapes:     {:?}", self.shapes);
        println!("  CPU copy:   {}", self.cpu_copy_of_the_last_frame());
    }
}

#[derive(Default)]
struct AudioStats {
    buffers: u32,
    frames: usize,
    span: Span,
    shapes: BTreeSet<String>,
    peak: f32,
}

impl AudioStats {
    fn include(&mut self, buffer: &AudioBuffer) {
        self.buffers += 1;
        self.frames += buffer.frames();
        self.span.include(buffer.timestamp());
        self.shapes.insert(format!(
            "{} Hz, {} channels",
            buffer.sample_rate(),
            buffer.channels()
        ));
        self.peak = buffer
            .samples()
            .iter()
            .fold(self.peak, |peak, sample| peak.max(sample.abs()));
    }

    fn print(&self, name: &str) {
        println!(
            "{name:<13} {} buffers, {} frames over {:.2} s, peak {:.3}, {:?}",
            self.buffers,
            self.frames,
            self.span.seconds(),
            self.peak,
            self.shapes
        );
    }
}

#[derive(Default)]
struct Stats {
    video: VideoStats,
    system: AudioStats,
    microphone: AudioStats,
    notes: Vec<String>,
}

impl Stats {
    fn include(&mut self, event: &Event) {
        match event {
            Event::Video(frame) => self.video.include(frame),
            Event::Audio(buffer) if buffer.source() == AudioSource::Microphone => {
                self.microphone.include(buffer);
            }
            Event::Audio(buffer) => self.system.include(buffer),
            Event::Interrupted(error) => self.notes.push(format!("interrupted: {error}")),
            Event::Resumed => self.notes.push("resumed".to_owned()),
            Event::Ended(EndReason::Stopped) => {}
            Event::Ended(reason) => self.notes.push(format!("ended: {reason:?}")),
            other => self.notes.push(format!("unknown event: {other:?}")),
        }
    }

    fn print(&self, session: &Session) {
        self.video.print();
        self.system.print("system sound:");
        self.microphone.print("microphone:");
        println!(
            "dropped:      {} video frames, {} audio buffers",
            session.dropped_video_frames(),
            session.dropped_audio_buffers()
        );
        for note in &self.notes {
            println!("note:         {note}");
        }
    }
}

fn print_targets(targets: &Targets) {
    println!("screens:");
    for (number, screen) in (1..).zip(&targets.screens) {
        println!(
            "  {number}. {} ({}x{})",
            screen.name, screen.size.width, screen.size.height
        );
    }
    println!("windows:");
    for (number, window) in (1..).zip(&targets.windows) {
        println!(
            "  {number}. {} - {} ({}x{})",
            window.app_name, window.title, window.size.width, window.size.height
        );
    }
    println!("apps:");
    for (number, app) in (1..).zip(&targets.apps) {
        println!("  {number}. {} (process {})", app.name, app.process_id);
    }
}

fn watch(session: &Session, duration: Duration) -> Stats {
    let mut stats = Stats::default();
    let deadline = Instant::now() + duration;
    let mut clock_lead = None;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match session.recv_timeout(left) {
            Ok(event) => {
                if clock_lead.is_none()
                    && let Some((now, stamped)) = session.now().zip(event.timestamp())
                {
                    clock_lead = Some(now.saturating_duration_since(stamped));
                }
                stats.include(&event);
            }
            Err(RecvError::TimedOut) => break,
            Err(RecvError::Finished) => return stats,
        }
    }
    session.stop();
    while let Some(event) = session.recv() {
        stats.include(&event);
    }
    if let Some(lead) = clock_lead {
        stats.notes.push(format!(
            "the session clock read {lead:?} after the stamp of the first event"
        ));
    }
    stats
}

fn picked_target(capturer: &Capturer) -> Result<Target, Box<dyn Error>> {
    let system_picker = capturer
        .picker()
        .ok_or("this system has neither a list of capture targets nor a picker")?;
    let mut options = PickerOptions::new();
    if let Ok(token) = std::env::var(RESTORE_TOKEN_VARIABLE) {
        options = options.with_restore_token(RestoreToken::new(token));
    }
    let (chosen, choice) = mpsc::channel();
    system_picker.pick(options, move |outcome| drop(chosen.send(outcome)));
    let picked = choice.recv()??.ok_or("the picker was cancelled")?;
    if let Some(token) = &picked.restore_token {
        println!(
            "restore token: {} (set {RESTORE_TOKEN_VARIABLE} to it to skip the picker next time)",
            token.as_str()
        );
    }
    Ok(picked.target)
}

fn main() -> Result<(), Box<dyn Error>> {
    let request = Request::parse(std::env::args().skip(1))?;
    let capturer = Capturer::system()?;
    println!(
        "system picker: {}",
        if capturer.picker().is_some() {
            "available"
        } else {
            "not available"
        }
    );
    println!("microphones:");
    for microphone in capturer.microphones()? {
        let default = if microphone.is_default {
            " (default)"
        } else {
            ""
        };
        println!("  {}{default}", microphone.name);
    }
    if let Some(apps) = capturer.audio_apps() {
        println!("apps playing sound:");
        for app in apps.playing()? {
            println!("  {} (id {})", app.name, app.id);
        }
    }
    let target = match capturer.lister() {
        Some(lister) => {
            let targets = lister.targets()?;
            print_targets(&targets);
            if request.list_only {
                return Ok(());
            }
            request.target(&targets)?
        }
        None if request.list_only => return Ok(()),
        None => picked_target(&capturer)?,
    };
    let options = request.options(&target);
    println!();
    println!(
        "capturing {:?} \"{}\" for {} s",
        target.kind(),
        target.label(),
        request.seconds
    );
    println!("options:      {options:?}");
    let session = capturer.start(&target, &options)?;
    let stats = watch(&session, Duration::from_secs(request.seconds));
    stats.print(&session);
    Ok(())
}
