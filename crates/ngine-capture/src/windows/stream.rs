use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use windows_capture::capture::{
    CaptureControl, Context, GraphicsCaptureApiError, GraphicsCaptureApiHandler,
};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::{
    Error as SessionError, GraphicsCaptureApi, InternalCaptureControl,
};
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use super::audio::{self, Sound, Source, Tap};
use super::clock::PerformanceCounterReadThroughInstant;
use super::convert::{self, Input};
use super::{content, surface};
use crate::error::CaptureError;
use crate::frame::{PixelFormat, Size, Surface, VideoFrame};
use crate::options::{Microphone, SessionOptions, SystemAudio};
use crate::session::{EventSink, RunningCapture};
use crate::supervisor::{self, Launch, Signal, Supervised, Supervisee};
use crate::target::Target;
use crate::time::Timestamp;

static RESTART_DELAYS: [Duration; 5] = [
    Duration::from_millis(100),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

struct Pictures {
    sink: EventSink,
    signals: Sender<Signal>,
    generation: u64,
    clock: Arc<PerformanceCounterReadThroughInstant>,
    stopping: Arc<AtomicBool>,
}

fn delivered_format(format: ColorFormat) -> Option<PixelFormat> {
    match format {
        ColorFormat::Bgra8 => Some(PixelFormat::Bgra8),
        ColorFormat::Rgba8 => Some(PixelFormat::Rgba8),
        ColorFormat::Rgba16F => None,
    }
}

fn native_format(format: PixelFormat) -> ColorFormat {
    match format {
        PixelFormat::Rgba8 => ColorFormat::Rgba8,
        PixelFormat::Bgra8 | PixelFormat::Nv12 => ColorFormat::Bgra8,
    }
}

impl GraphicsCaptureApiHandler for Pictures {
    type Flags = Self;
    type Error = CaptureError;

    fn new(context: Context<Self>) -> Result<Self, CaptureError> {
        Ok(context.flags)
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _stops_from_inside: InternalCaptureControl,
    ) -> Result<(), CaptureError> {
        let timestamp = frame
            .timestamp()
            .ok()
            .and_then(|since_boot| convert::timestamp(since_boot.Duration));
        let Some((timestamp, format)) = timestamp.zip(delivered_format(frame.color_format()))
        else {
            return Ok(());
        };
        self.clock.observe(timestamp);
        let size = Size::new(frame.width(), frame.height());
        let (lent_surface, lent) = surface::lend(frame, size, format);
        self.sink.video(VideoFrame::from_surface(
            timestamp,
            size,
            format,
            Surface::D3d11(lent_surface),
        ));
        lent.serve_until_returned(frame, &self.stopping);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), CaptureError> {
        supervisor::send_unless_the_supervisor_is_gone(
            &self.signals,
            Signal::Stopped {
                generation: self.generation,
                error: CaptureError::TargetGone,
                by_user: false,
            },
        );
        Ok(())
    }
}

fn error_from(error: GraphicsCaptureApiError<CaptureError>) -> CaptureError {
    match error {
        GraphicsCaptureApiError::NewHandlerError(error)
        | GraphicsCaptureApiError::FrameHandlerError(error) => error,
        GraphicsCaptureApiError::ItemConvertFailed => CaptureError::TargetGone,
        GraphicsCaptureApiError::GraphicsCaptureApiError(SessionError::Unsupported) => {
            CaptureError::Unsupported(SessionError::Unsupported.to_string())
        }
        GraphicsCaptureApiError::GraphicsCaptureApiError(SessionError::WindowsError(error)) => {
            convert::error_from_code(error.code().0, &error.message(), Input::Picture)
        }
        other => CaptureError::Os(other.to_string()),
    }
}

fn this_windows_can(check: fn() -> Result<bool, SessionError>) -> bool {
    check().unwrap_or(false)
}

fn cursor(show_cursor: bool) -> CursorCaptureSettings {
    if !this_windows_can(GraphicsCaptureApi::is_cursor_settings_supported) {
        CursorCaptureSettings::Default
    } else if show_cursor {
        CursorCaptureSettings::WithCursor
    } else {
        CursorCaptureSettings::WithoutCursor
    }
}

fn border() -> DrawBorderSettings {
    if this_windows_can(GraphicsCaptureApi::is_border_settings_supported) {
        DrawBorderSettings::WithoutBorder
    } else {
        DrawBorderSettings::Default
    }
}

fn pace(frame_rate: u32) -> MinimumUpdateIntervalSettings {
    convert::time_between_frames(frame_rate)
        .filter(|_| this_windows_can(GraphicsCaptureApi::is_minimum_update_interval_supported))
        .map_or(
            MinimumUpdateIntervalSettings::Default,
            MinimumUpdateIntervalSettings::Custom,
        )
}

fn wanted_sounds(options: &SessionOptions) -> Result<Vec<Source>, CaptureError> {
    let system = match &options.system_audio {
        SystemAudio::Off => None,
        SystemAudio::Everything => Some(Source::EverythingButThisProcess),
        SystemAudio::App(app) => Some(Source::App(u32::try_from(app.id).map_err(|_| {
            CaptureError::Unavailable("the app whose sound was asked for does not exist".to_owned())
        })?)),
    };
    let microphone = match &options.microphone {
        Microphone::Off => None,
        Microphone::SystemDefault => Some(Source::DefaultMicrophone),
        Microphone::Device(id) => Some(Source::Microphone(id.clone())),
    };
    Ok(system.into_iter().chain(microphone).collect())
}

struct Running {
    picture: CaptureControl<Pictures, CaptureError>,
    stopping: Arc<AtomicBool>,
    sounds: Vec<Sound>,
}

impl Supervisee for Running {
    fn stop(self) {
        for sound in self.sounds {
            sound.stop();
        }
        self.stopping.store(true, Ordering::Release);
        let already_ended_when_the_target_closed = self.picture.stop();
        drop(already_ended_when_the_target_closed);
    }
}

struct Plan {
    target: Target,
    options: SessionOptions,
    clock: Arc<PerformanceCounterReadThroughInstant>,
}

impl Launch for Plan {
    type Capture = Running;

    fn launch(
        &self,
        generation: u64,
        sink: &EventSink,
        signals: &Sender<Signal>,
    ) -> Result<Running, CaptureError> {
        let item = content::resolve(&self.target)?;
        let sources = wanted_sounds(&self.options)?;
        let stopping = Arc::new(AtomicBool::new(false));
        let settings = Settings::new(
            item,
            cursor(self.options.show_cursor),
            border(),
            SecondaryWindowSettings::Default,
            pace(self.options.frame_rate),
            DirtyRegionSettings::Default,
            native_format(self.options.pixel_format),
            Pictures {
                sink: sink.clone(),
                signals: signals.clone(),
                generation,
                clock: Arc::clone(&self.clock),
                stopping: Arc::clone(&stopping),
            },
        );
        let picture = Pictures::start_free_threaded(settings).map_err(error_from)?;
        let mut running = Running {
            picture,
            stopping,
            sounds: Vec::new(),
        };
        for source in sources {
            let tap = Tap {
                sink: sink.clone(),
                signals: signals.clone(),
                generation,
                clock: Arc::clone(&self.clock),
            };
            match audio::start(source, tap) {
                Ok(sound) => running.sounds.push(sound),
                Err(error) => {
                    running.stop();
                    return Err(error);
                }
            }
        }
        Ok(running)
    }
}

struct WindowsCapture {
    supervised: Supervised<Running>,
    clock: Arc<PerformanceCounterReadThroughInstant>,
}

impl RunningCapture for WindowsCapture {
    fn stop(&mut self) {
        self.supervised.stop();
    }

    fn now(&self) -> Option<Timestamp> {
        self.clock.now()
    }
}

pub(super) fn start(
    target: &Target,
    options: &SessionOptions,
    sink: EventSink,
) -> Result<Box<dyn RunningCapture>, CaptureError> {
    let clock = Arc::new(PerformanceCounterReadThroughInstant::new());
    let plan = Plan {
        target: target.clone(),
        options: options.clone(),
        clock: Arc::clone(&clock),
    };
    let supervised = supervisor::supervise(plan, sink, &RESTART_DELAYS)?;
    Ok(Box::new(WindowsCapture { supervised, clock }))
}
