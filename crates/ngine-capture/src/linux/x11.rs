use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::errors::{ReplyError, ReplyOrIdError};
use x11rb::protocol::ErrorKind;
use x11rb::protocol::composite::{ConnectionExt as _, Redirect};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder};
use x11rb::rust_connection::RustConnection;

use super::lifecycle::{Change, Lifecycle};
use super::monotonic;
use super::pixels::{self, Cursor, XImageLayout};
use super::pw::{self, PipeWireCapture, Reporter, Request};
use crate::backend::{Backend, ListAudioApps, ListTargets, PickTarget};
use crate::error::CaptureError;
use crate::frame::{CpuFrame, Size, VideoFrame};
use crate::options::SessionOptions;
use crate::session::{EventSink, RunningCapture};
use crate::supervisor::{self, Launch, Signal, Supervised, Supervisee};
use crate::target::{
    AudioApp, MicrophoneDevice, Screen, ScreenId, Target, TargetSource, Targets, Window, WindowId,
};
use crate::time::Timestamp;

const EVERY_PLANE: u32 = u32::MAX;
const LONGEST_PROPERTY: u32 = 4096;
const BYTES_PER_PIXEL: u32 = 4;

pub(super) struct X11Backend;

struct Display {
    connection: RustConnection,
    screen: usize,
    root: u32,
}

struct Monitor {
    id: u64,
    name: String,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

fn lost(error: &dyn std::fmt::Display) -> CaptureError {
    CaptureError::Unavailable(format!("the X server could not be reached: {error}"))
}

fn server_failure(error: &ReplyError) -> CaptureError {
    match error {
        ReplyError::ConnectionError(error) => lost(error),
        ReplyError::X11Error(error) => {
            CaptureError::Os(format!("the X server refused a request: {error:?}"))
        }
    }
}

impl Display {
    fn open() -> Result<Self, CaptureError> {
        let (connection, screen) = x11rb::connect(None).map_err(|error| lost(&error))?;
        let root = connection
            .setup()
            .roots
            .get(screen)
            .map(|root| root.root)
            .ok_or_else(|| CaptureError::Unavailable("the X server has no screen".to_owned()))?;
        Ok(Self {
            connection,
            screen,
            root,
        })
    }

    fn whole_root(&self) -> Option<Monitor> {
        let root = self.connection.setup().roots.get(self.screen)?;
        Some(Monitor {
            id: u64::from(root.root),
            name: "Screen".to_owned(),
            x: 0,
            y: 0,
            width: root.width_in_pixels,
            height: root.height_in_pixels,
        })
    }

    fn randr_monitors(&self) -> Result<Vec<Monitor>, ReplyError> {
        let listed = self
            .connection
            .randr_get_monitors(self.root, true)?
            .reply()?;
        listed
            .monitors
            .into_iter()
            .map(|monitor| {
                let name = self.connection.get_atom_name(monitor.name)?.reply()?.name;
                Ok(Monitor {
                    id: u64::from(monitor.name),
                    name: String::from_utf8_lossy(&name).into_owned(),
                    x: monitor.x,
                    y: monitor.y,
                    width: monitor.width,
                    height: monitor.height,
                })
            })
            .collect()
    }

    fn monitors(&self) -> Vec<Monitor> {
        let listed = self.randr_monitors().unwrap_or_default();
        if listed.is_empty() {
            self.whole_root().into_iter().collect()
        } else {
            listed
        }
    }

    fn atom(&self, name: &str) -> Result<u32, ReplyError> {
        Ok(self
            .connection
            .intern_atom(false, name.as_bytes())?
            .reply()?
            .atom)
    }

    fn bytes(&self, window: u32, property: u32) -> Option<Vec<u8>> {
        let reply = self
            .connection
            .get_property(false, window, property, AtomEnum::ANY, 0, LONGEST_PROPERTY)
            .ok()?
            .reply()
            .ok()?;
        (!reply.value.is_empty()).then_some(reply.value)
    }

    fn numbers(&self, window: u32, property: u32) -> Vec<u32> {
        self.connection
            .get_property(false, window, property, AtomEnum::ANY, 0, LONGEST_PROPERTY)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(Iterator::collect))
            .unwrap_or_default()
    }

    fn listed_window(&self, window: u32, atoms: &WindowAtoms) -> Option<Window> {
        let title = self
            .bytes(window, atoms.name)
            .or_else(|| self.bytes(window, AtomEnum::WM_NAME.into()))?;
        let title = String::from_utf8_lossy(&title).into_owned();
        let class = self
            .bytes(window, AtomEnum::WM_CLASS.into())
            .unwrap_or_default();
        let app_name = class
            .split(|byte| *byte == 0)
            .rfind(|part| !part.is_empty())
            .map_or_else(
                || title.clone(),
                |name| String::from_utf8_lossy(name).into_owned(),
            );
        let process_id = self.numbers(window, atoms.process_id).first().copied();
        let geometry = self.connection.get_geometry(window).ok()?.reply().ok()?;
        let is_ours = process_id == Some(std::process::id());
        (!is_ours && !title.is_empty()).then(|| {
            Window::new(
                WindowId(u64::from(window)),
                title,
                app_name,
                process_id,
                Size::new(u32::from(geometry.width), u32::from(geometry.height)),
            )
        })
    }

    fn targets(&self) -> Result<Targets, ReplyError> {
        let atoms = WindowAtoms {
            clients: self.atom("_NET_CLIENT_LIST")?,
            name: self.atom("_NET_WM_NAME")?,
            process_id: self.atom("_NET_WM_PID")?,
        };
        let screens = self
            .monitors()
            .into_iter()
            .map(|monitor| {
                Screen::new(
                    ScreenId(monitor.id),
                    monitor.name,
                    Size::new(u32::from(monitor.width), u32::from(monitor.height)),
                )
            })
            .collect();
        let windows = self
            .numbers(self.root, atoms.clients)
            .into_iter()
            .filter_map(|window| self.listed_window(window, &atoms))
            .collect();
        Ok(Targets::new(screens, windows, Vec::new()))
    }

    fn layout_of(&self, depth: u8, visual: u32) -> Option<XImageLayout> {
        let setup = self.connection.setup();
        let root = setup.roots.get(self.screen)?;
        let visuals = || root.allowed_depths.iter().flat_map(|depth| &depth.visuals);
        let visual = visuals()
            .find(|candidate| candidate.visual_id == visual)
            .or_else(|| visuals().find(|candidate| candidate.visual_id == root.root_visual))?;
        let format = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == depth)?;
        Some(XImageLayout {
            bits_per_pixel: format.bits_per_pixel,
            most_significant_byte_first: setup.image_byte_order == ImageOrder::MSB_FIRST,
            red_mask: visual.red_mask,
            blue_mask: visual.blue_mask,
        })
    }
}

struct WindowAtoms {
    clients: u32,
    name: u32,
    process_id: u32,
}

impl ListTargets for X11Backend {
    fn targets(&self) -> Result<Targets, CaptureError> {
        Display::open()?
            .targets()
            .map_err(|error| server_failure(&error))
    }
}

impl ListAudioApps for X11Backend {
    fn playing(&self) -> Result<Vec<AudioApp>, CaptureError> {
        pw::playing_apps()
    }
}

#[derive(Clone, Copy)]
enum Area {
    Screen(u64),
    Window(u32),
}

#[derive(Clone, Copy)]
struct Rectangle {
    drawable: u32,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    left_on_root: i16,
    top_on_root: i16,
}

enum GrabError {
    Paused(CaptureError),
    Gone,
    Broken(CaptureError),
}

fn grab_error(error: &ReplyError) -> GrabError {
    match error {
        ReplyError::X11Error(error)
            if matches!(error.error_kind, ErrorKind::Window | ErrorKind::Drawable) =>
        {
            GrabError::Gone
        }
        ReplyError::X11Error(error) if error.error_kind == ErrorKind::Match => GrabError::Paused(
            CaptureError::Os("the window is hidden or partly off the screen".to_owned()),
        ),
        other => GrabError::Broken(server_failure(other)),
    }
}

struct SharedSegment {
    id: u32,
    memory: File,
    len: u32,
}

struct Image {
    depth: u8,
    visual: u32,
    data: Vec<u8>,
}

struct Grabber {
    display: Display,
    area: Area,
    draws_cursor: bool,
    shares_memory: bool,
    segment: Option<SharedSegment>,
    measured_screen: Option<Rectangle>,
}

impl Grabber {
    fn new(area: Area, show_cursor: bool) -> Result<Self, CaptureError> {
        let display = Display::open()?;
        let connection = &display.connection;
        let shares_memory = connection
            .shm_query_version()
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|version| (version.major_version, version.minor_version) >= (1, 2));
        let has_xfixes = connection
            .xfixes_query_version(4, 0)
            .is_ok_and(|cookie| cookie.reply().is_ok());
        if let Area::Window(window) = area {
            let composites = connection
                .composite_query_version(0, 4)
                .is_ok_and(|cookie| cookie.reply().is_ok());
            if composites
                && let Ok(cookie) =
                    connection.composite_redirect_window(window, Redirect::AUTOMATIC)
            {
                let _another_client_already_redirects_it = cookie.check();
            }
        }
        Ok(Self {
            display,
            area,
            draws_cursor: show_cursor && has_xfixes,
            shares_memory,
            segment: None,
            measured_screen: None,
        })
    }

    fn rectangle(&self) -> Result<Rectangle, GrabError> {
        match self.area {
            Area::Screen(_) if self.measured_screen.is_some() => {
                self.measured_screen.ok_or(GrabError::Gone)
            }
            Area::Screen(id) => {
                let monitor = self
                    .display
                    .monitors()
                    .into_iter()
                    .find(|monitor| monitor.id == id)
                    .ok_or(GrabError::Gone)?;
                Ok(Rectangle {
                    drawable: self.display.root,
                    x: monitor.x,
                    y: monitor.y,
                    width: monitor.width,
                    height: monitor.height,
                    left_on_root: monitor.x,
                    top_on_root: monitor.y,
                })
            }
            Area::Window(window) => {
                let connection = &self.display.connection;
                let measured = || -> Result<Rectangle, ReplyError> {
                    let geometry = connection.get_geometry(window)?.reply()?;
                    let origin = connection
                        .translate_coordinates(window, self.display.root, 0, 0)?
                        .reply()?;
                    Ok(Rectangle {
                        drawable: window,
                        x: 0,
                        y: 0,
                        width: geometry.width,
                        height: geometry.height,
                        left_on_root: origin.dst_x,
                        top_on_root: origin.dst_y,
                    })
                };
                measured().map_err(|error| grab_error(&error))
            }
        }
    }

    fn segment_of(&mut self, len: u32) -> Result<&SharedSegment, ReplyOrIdError> {
        if let Some(stale) = self.segment.take_if(|segment| segment.len != len) {
            self.display.connection.shm_detach(stale.id)?;
        }
        if self.segment.is_none() {
            let id = self.display.connection.generate_id()?;
            let created = self
                .display
                .connection
                .shm_create_segment(id, len, false)?
                .reply()?;
            self.segment = Some(SharedSegment {
                id,
                memory: File::from(created.shm_fd),
                len,
            });
        }
        self.segment.as_ref().ok_or(ReplyOrIdError::IdsExhausted)
    }

    fn shared_image(&mut self, area: Rectangle) -> Result<Image, ReplyOrIdError> {
        let len = u32::from(area.width) * u32::from(area.height) * BYTES_PER_PIXEL;
        let segment = self.segment_of(len)?.id;
        let written = self
            .display
            .connection
            .shm_get_image(
                area.drawable,
                area.x,
                area.y,
                area.width,
                area.height,
                EVERY_PLANE,
                ImageFormat::Z_PIXMAP.into(),
                segment,
                0,
            )?
            .reply()?;
        let mut data = vec![0; usize::try_from(written.size.min(len)).unwrap_or(0)];
        let read = self
            .segment
            .as_ref()
            .is_some_and(|segment| segment.memory.read_exact_at(&mut data, 0).is_ok());
        if read {
            Ok(Image {
                depth: written.depth,
                visual: written.visual,
                data,
            })
        } else {
            Err(ReplyOrIdError::IdsExhausted)
        }
    }

    fn sent_image(&self, area: Rectangle) -> Result<Image, ReplyError> {
        let sent = self
            .display
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                area.drawable,
                area.x,
                area.y,
                area.width,
                area.height,
                EVERY_PLANE,
            )?
            .reply()?;
        Ok(Image {
            depth: sent.depth,
            visual: sent.visual,
            data: sent.data,
        })
    }

    fn image(&mut self, area: Rectangle) -> Result<Image, GrabError> {
        if self.shares_memory {
            match self.shared_image(area) {
                Ok(image) => return Ok(image),
                Err(ReplyOrIdError::X11Error(error))
                    if matches!(
                        error.error_kind,
                        ErrorKind::Match | ErrorKind::Window | ErrorKind::Drawable
                    ) =>
                {
                    return Err(grab_error(&ReplyError::X11Error(error)));
                }
                Err(_) => self.shares_memory = false,
            }
        }
        self.sent_image(area).map_err(|error| grab_error(&error))
    }

    fn draw_cursor(
        &self,
        pixels: &mut [u8],
        size: Size,
        format: crate::PixelFormat,
        area: Rectangle,
    ) {
        let shown = self
            .display
            .connection
            .xfixes_get_cursor_image()
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let Some(shown) = shown else {
            return;
        };
        let offset = |position: i16, hotspot: u16, origin: i16| {
            i32::from(position) - i32::from(hotspot) - i32::from(origin)
        };
        let cursor = Cursor {
            left: offset(shown.x, shown.xhot, area.left_on_root),
            top: offset(shown.y, shown.yhot, area.top_on_root),
            width: shown.width,
            premultiplied_argb: &shown.cursor_image,
        };
        pixels::draw_cursor(pixels, size, format, &cursor);
    }

    fn grab(&mut self) -> Result<VideoFrame, GrabError> {
        let area = self.rectangle()?;
        let size = Size::new(u32::from(area.width), u32::from(area.height));
        self.measured_screen = None;
        let image = self.image(area)?;
        self.measured_screen = matches!(self.area, Area::Screen(_)).then_some(area);
        let layout = self
            .display
            .layout_of(image.depth, image.visual)
            .ok_or_else(|| {
                GrabError::Broken(CaptureError::Unsupported(format!(
                    "an X11 image of depth {} whose layout the server did not describe",
                    image.depth
                )))
            })?;
        let (format, mut pixels) =
            pixels::packed_from_ximage(layout, size, image.data).map_err(GrabError::Broken)?;
        if self.draws_cursor {
            self.draw_cursor(&mut pixels, size, format, area);
        }
        let stride = usize::from(area.width) * 4;
        let frame = CpuFrame::new(size, format, stride, pixels).ok_or_else(|| {
            GrabError::Broken(CaptureError::Os(
                "the X server sent a short image".to_owned(),
            ))
        })?;
        let timestamp = monotonic::now().unwrap_or_default();
        Ok(VideoFrame::from_cpu(timestamp, Arc::new(frame)))
    }
}

struct Loop {
    sink: EventSink,
    stop: Arc<AtomicBool>,
    frame_rate: u32,
    reporter: Reporter,
}

fn capture(mut grabber: Grabber, running: &Loop) {
    let interval = Duration::from_secs(1) / running.frame_rate.max(1);
    let mut lifecycle = Lifecycle::default();
    while !running.stop.load(Ordering::Acquire) {
        let began = Instant::now();
        let paused = match grabber.grab() {
            Ok(frame) => {
                if let Some(resumed) = lifecycle.on(Change::Streaming) {
                    running.sink.deliver_blocking(resumed);
                }
                running.sink.video(frame);
                None
            }
            Err(GrabError::Paused(why)) => lifecycle.on(Change::Paused(why)),
            Err(GrabError::Gone) => {
                running.reporter.stopped(CaptureError::TargetGone, false);
                return;
            }
            Err(GrabError::Broken(why)) => {
                running.reporter.stopped(why, false);
                return;
            }
        };
        if let Some(paused) = paused {
            running.sink.deliver_blocking(paused);
        }
        std::thread::sleep(interval.saturating_sub(began.elapsed()));
    }
}

struct X11Running {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    sound: Option<PipeWireCapture>,
}

impl Supervisee for X11Running {
    fn stop(self) {
        self.stop.store(true, Ordering::Release);
        let _panicked = self.thread.join();
        if let Some(mut sound) = self.sound {
            sound.stop();
        }
    }
}

struct X11Launch {
    area: Area,
    options: SessionOptions,
}

impl Launch for X11Launch {
    type Capture = X11Running;

    fn launch(
        &self,
        generation: u64,
        sink: &EventSink,
        signals: &Sender<Signal>,
    ) -> Result<X11Running, CaptureError> {
        let grabber = Grabber::new(self.area, self.options.show_cursor)?;
        match grabber.rectangle() {
            Ok(_) | Err(GrabError::Paused(_)) => {}
            Err(GrabError::Gone) => return Err(CaptureError::TargetGone),
            Err(GrabError::Broken(why)) => return Err(why),
        }
        let request = Request {
            video: None,
            system: pw::system_sound(&self.options.system_audio)?,
            microphone: pw::microphone_choice(&self.options.microphone),
            frame_rate: self.options.frame_rate,
            pixel_format: self.options.pixel_format,
            reporter: None,
        };
        let sound = if request.wants_sound() {
            Some(pw::start(request, sink.clone())?)
        } else {
            None
        };
        let stop = Arc::new(AtomicBool::new(false));
        let running = Loop {
            sink: sink.clone(),
            stop: Arc::clone(&stop),
            frame_rate: self.options.frame_rate,
            reporter: Reporter {
                signals: signals.clone(),
                generation,
            },
        };
        let thread = std::thread::Builder::new()
            .name("ngine-capture-x11".to_owned())
            .spawn(move || capture(grabber, &running))
            .map_err(|error| {
                CaptureError::Os(format!("could not start the capture thread: {error}"))
            })?;
        Ok(X11Running {
            stop,
            thread,
            sound,
        })
    }
}

struct X11Capture {
    supervised: Supervised<X11Running>,
}

impl RunningCapture for X11Capture {
    fn stop(&mut self) {
        self.supervised.stop();
    }

    fn now(&self) -> Option<Timestamp> {
        monotonic::now()
    }
}

fn area_of(target: &Target) -> Result<Area, CaptureError> {
    match target.source() {
        TargetSource::Screen(screen) => Ok(Area::Screen(screen.id.0)),
        TargetSource::Window(window) => u32::try_from(window.id.0)
            .map(Area::Window)
            .map_err(|_| CaptureError::TargetGone),
        TargetSource::App(_) | TargetSource::Picked(_) => Err(CaptureError::Unsupported(
            "capturing a whole app or a picked target under X11: list a screen or a window"
                .to_owned(),
        )),
    }
}

impl Backend for X11Backend {
    fn target_lister(&self) -> Option<&dyn ListTargets> {
        Some(self)
    }

    fn target_picker(&self) -> Option<&dyn PickTarget> {
        None
    }

    fn audio_app_lister(&self) -> Option<&dyn ListAudioApps> {
        Some(self)
    }

    fn microphones(&self) -> Result<Vec<MicrophoneDevice>, CaptureError> {
        pw::microphone_devices()
    }

    fn start(
        &self,
        target: &Target,
        options: &SessionOptions,
        sink: EventSink,
    ) -> Result<Box<dyn RunningCapture>, CaptureError> {
        let launch = X11Launch {
            area: area_of(target)?,
            options: options.clone(),
        };
        let supervised = supervisor::supervise(launch, sink, &super::RESTART_DELAYS)?;
        Ok(Box::new(X11Capture { supervised }))
    }
}
