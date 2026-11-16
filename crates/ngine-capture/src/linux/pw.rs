use std::cell::{Cell, RefCell};
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::context::ContextRc;
use pw::core::CoreRc;
use pw::main_loop::MainLoopRc;
use pw::properties::PropertiesBox;
use pw::spa;
use pw::stream::{StreamFlags, StreamRc, StreamState};
use spa::buffer::DataType;
use spa::buffer::meta::MetaHeader;
use spa::param::ParamType;
use spa::param::audio::{AudioFormat, AudioInfoRaw};
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::serialize::PodSerializer;
use spa::pod::{ChoiceValue, Object, Pod, Property, PropertyFlags, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Direction, Fraction, Id, Rectangle, SpaTypes};

use super::clock::{self, FrameLimiter, GraphTime};
use super::lifecycle::{self, Change, Lifecycle};
use super::nodes::{self, Node};
use super::pixels::{self, Chunk, Layout};
use super::surface::{DmaBufPlane, DmaBufSurface};
use super::{descriptor, monotonic, samples};
use crate::audio::{AudioBuffer, AudioSource};
use crate::error::CaptureError;
use crate::event::{EndReason, Event};
use crate::frame::{PixelFormat, Size, Surface, VideoFrame};
use crate::session::EventSink;
use crate::supervisor::{self, Signal};
use crate::time::Timestamp;

const STARTS_WITHIN: Duration = Duration::from_secs(5);
const ANSWERS_WITHIN: Duration = Duration::from_secs(3);
const LARGEST_SIDE: u32 = 16_384;
const BROKEN_PIPE: i32 = -32;
const DEFAULT_SOURCE_KEY: &str = "default.audio.source";
const MODIFIER_THE_DRIVER_CHOSE: i64 = 0x00ff_ffff_ffff_ffff;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Memory {
    Shared,
    DmaBuf,
}

pub(super) struct VideoSource {
    pub(super) remote: OwnedFd,
    pub(super) node_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SystemSound {
    Off,
    Everything,
    Node(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MicrophoneChoice {
    Off,
    SystemDefault,
    Named(String),
}

pub(super) struct Reporter {
    pub(super) signals: Sender<Signal>,
    pub(super) generation: u64,
}

impl Reporter {
    pub(super) fn stopped(&self, error: CaptureError, by_user: bool) {
        supervisor::send_unless_the_supervisor_is_gone(
            &self.signals,
            Signal::Stopped {
                generation: self.generation,
                error,
                by_user,
            },
        );
    }
}

pub(super) struct Request {
    pub(super) video: Option<VideoSource>,
    pub(super) system: SystemSound,
    pub(super) microphone: MicrophoneChoice,
    pub(super) frame_rate: u32,
    pub(super) pixel_format: PixelFormat,
    pub(super) gpu_surfaces: bool,
    pub(super) reporter: Option<Reporter>,
}

impl Request {
    pub(super) fn wants_sound(&self) -> bool {
        self.system != SystemSound::Off || self.microphone != MicrophoneChoice::Off
    }
}

pub(super) struct PipeWireCapture {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl PipeWireCapture {
    pub(super) fn stop(&mut self) {
        let _already_stopped = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _panicked = thread.join();
        }
    }
}

impl Drop for PipeWireCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn unavailable(what: &str, error: &pw::Error) -> CaptureError {
    CaptureError::Unavailable(format!("{what}: {error}"))
}

fn failed(what: &str, error: &pw::Error) -> CaptureError {
    CaptureError::Os(format!("{what}: {error}"))
}

pub(super) fn start(request: Request, sink: EventSink) -> Result<PipeWireCapture, CaptureError> {
    let (stop, stop_requests) = pw::channel::channel::<()>();
    let (ready, started) = mpsc::channel();
    let stop_from_inside = stop.clone();
    let thread = std::thread::Builder::new()
        .name("ngine-capture-pipewire".to_owned())
        .spawn(move || {
            let outcome = run(request, &sink, stop_requests, &stop_from_inside, &ready);
            if let Err(error) = outcome {
                let _start_already_returned = ready.send(Err(error));
            }
        })
        .map_err(|error| {
            CaptureError::Os(format!("could not start the capture thread: {error}"))
        })?;
    let mut capture = PipeWireCapture {
        stop,
        thread: Some(thread),
    };
    let outcome = started.recv_timeout(STARTS_WITHIN).unwrap_or_else(|_| {
        Err(CaptureError::Unavailable(
            "PipeWire did not start the capture in time".to_owned(),
        ))
    });
    match outcome {
        Ok(()) => Ok(capture),
        Err(error) => {
            capture.stop();
            Err(error)
        }
    }
}

fn serialized(object: Object) -> Result<Vec<u8>, CaptureError> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|error| CaptureError::Os(format!("could not describe a stream format: {error:?}")))
}

fn spa_format(layout: Layout) -> VideoFormat {
    match layout {
        Layout::Bgrx => VideoFormat::BGRx,
        Layout::Bgra => VideoFormat::BGRA,
        Layout::Rgbx => VideoFormat::RGBx,
        Layout::Rgba => VideoFormat::RGBA,
        Layout::Nv12 => VideoFormat::NV12,
    }
}

fn layout_of(format: VideoFormat) -> Option<Layout> {
    [
        Layout::Bgrx,
        Layout::Bgra,
        Layout::Rgbx,
        Layout::Rgba,
        Layout::Nv12,
    ]
    .into_iter()
    .find(|layout| spa_format(*layout) == format)
}

fn video_formats(
    wanted: PixelFormat,
    frame_rate: u32,
    memory: Memory,
) -> Result<Vec<u8>, CaptureError> {
    let layouts = match memory {
        Memory::Shared => pixels::preference(wanted),
        Memory::DmaBuf => pixels::preference_in_one_plane(wanted),
    };
    let modifier = (memory == Memory::DmaBuf).then(|| Property {
        key: FormatProperties::VideoModifier.as_raw(),
        flags: PropertyFlags::MANDATORY,
        value: Value::Long(MODIFIER_THE_DRIVER_CHOSE),
    });
    let offered: Vec<Id> = layouts
        .into_iter()
        .map(|layout| Id(spa_format(layout).as_raw()))
        .collect();
    let first = offered
        .first()
        .copied()
        .unwrap_or(Id(VideoFormat::BGRx.as_raw()));
    let side = |length| Rectangle {
        width: length,
        height: length,
    };
    let rate = |num| Fraction { num, denom: 1 };
    let property = |key: FormatProperties, value| Property::new(key.as_raw(), value);
    let kind = [
        property(
            FormatProperties::MediaType,
            Value::Id(Id(MediaType::Video.as_raw())),
        ),
        property(
            FormatProperties::MediaSubtype,
            Value::Id(Id(MediaSubtype::Raw.as_raw())),
        ),
        property(
            FormatProperties::VideoFormat,
            Value::Choice(ChoiceValue::Id(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: first,
                    alternatives: offered,
                },
            ))),
        ),
    ];
    serialized(Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: kind
            .into_iter()
            .chain(modifier)
            .chain([
                property(
                    FormatProperties::VideoSize,
                    Value::Choice(ChoiceValue::Rectangle(Choice(
                        ChoiceFlags::empty(),
                        ChoiceEnum::Range {
                            default: Rectangle {
                                width: 1920,
                                height: 1080,
                            },
                            min: side(1),
                            max: side(LARGEST_SIDE),
                        },
                    ))),
                ),
                property(
                    FormatProperties::VideoFramerate,
                    Value::Choice(ChoiceValue::Fraction(Choice(
                        ChoiceFlags::empty(),
                        ChoiceEnum::Range {
                            default: rate(frame_rate.max(1)),
                            min: rate(0),
                            max: rate(frame_rate.max(1)),
                        },
                    ))),
                ),
            ])
            .collect(),
    })
}

fn frame_header_meta() -> Result<Vec<u8>, CaptureError> {
    let header_size = i32::try_from(size_of::<spa::sys::spa_meta_header>()).unwrap_or(i32::MAX);
    serialized(Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_META_type,
                Value::Id(Id(spa::sys::SPA_META_Header)),
            ),
            Property::new(spa::sys::SPA_PARAM_META_size, Value::Int(header_size)),
        ],
    })
}

fn float_audio() -> Result<Vec<u8>, CaptureError> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    serialized(Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    })
}

fn raw_format_in(id: u32, param: Option<&Pod>, media: MediaType) -> Option<&Pod> {
    let param = param.filter(|_| id == ParamType::Format.as_raw())?;
    let (media_type, subtype) = format_utils::parse_format(param).ok()?;
    (media_type == media && subtype == MediaSubtype::Raw).then_some(param)
}

fn now() -> Timestamp {
    monotonic::now().unwrap_or_default()
}

struct Ender {
    sink: EventSink,
    stop: pw::channel::Sender<()>,
    reporter: Option<Reporter>,
}

impl Ender {
    fn report(&self, event: Option<Event>) {
        let (error, by_user) = match event {
            None => return,
            Some(Event::Ended(EndReason::Failed(error))) => (error, false),
            Some(Event::Ended(_)) => (
                CaptureError::Os("the screen stream was closed".to_owned()),
                true,
            ),
            Some(other) => {
                self.sink.deliver_blocking(other);
                return;
            }
        };
        if let Some(reporter) = &self.reporter {
            reporter.stopped(error, by_user);
        }
        let _loop_already_gone = self.stop.send(());
    }
}

struct VideoState {
    format: VideoInfoRaw,
    lifecycle: Lifecycle,
    limiter: FrameLimiter,
    ender: Rc<Ender>,
}

fn change_of(state: &StreamState) -> Option<Change> {
    match state {
        StreamState::Streaming => Some(Change::Streaming),
        StreamState::Paused => Some(Change::Paused(CaptureError::Os(
            "the screen stream paused to renegotiate its format".to_owned(),
        ))),
        StreamState::Unconnected => Some(Change::Disconnected),
        StreamState::Error(message) => Some(Change::Failed(lifecycle::stream_error(message))),
        StreamState::Connecting => None,
    }
}

fn dma_buf_surface(
    layout: Layout,
    size: Size,
    modifier: u64,
    datas: &[spa::buffer::Data],
) -> Option<DmaBufSurface> {
    let planes = datas
        .iter()
        .map(|data| {
            let stride = u32::try_from(data.chunk().stride()).ok()?;
            let ours = descriptor::duplicate_dma_buf(data)?;
            Some(DmaBufPlane::new(ours, data.chunk().offset(), stride))
        })
        .collect::<Option<Vec<DmaBufPlane>>>()?;
    Some(DmaBufSurface::new(
        size,
        layout.drm_fourcc(),
        modifier,
        planes,
    ))
}

fn deliver_frame(stream: &pw::stream::Stream, state: &mut VideoState) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let stamped = buffer.find_meta::<MetaHeader>().map(MetaHeader::pts);
    let timestamp = clock::video_timestamp(stamped, now());
    let Some(layout) = layout_of(state.format.format()) else {
        return;
    };
    let size = Size::new(state.format.size().width, state.format.size().height);
    if !state.limiter.accepts(timestamp) {
        return;
    }
    let datas = buffer.datas_mut();
    if datas.first().map(spa::buffer::Data::type_) == Some(DataType::DmaBuf) {
        let shared = dma_buf_surface(layout, size, state.format.modifier(), datas);
        if let Some(surface) = shared {
            state.ender.sink.video(VideoFrame::from_surface(
                timestamp,
                size,
                layout.pixel_format(),
                Surface::DmaBuf(surface),
            ));
        }
        return;
    }
    let Some(data) = datas.first_mut() else {
        return;
    };
    let chunk = Chunk {
        offset: usize::try_from(data.chunk().offset()).unwrap_or(usize::MAX),
        size: usize::try_from(data.chunk().size()).unwrap_or(0),
        stride: data.chunk().stride(),
    };
    let frame = data
        .data()
        .and_then(|mapped| pixels::frame_from_mapped(layout, size, chunk, mapped));
    if let Some(frame) = frame {
        state
            .ender
            .sink
            .video(VideoFrame::from_cpu(timestamp, Arc::new(frame)));
    }
}

type VideoListener = pw::stream::StreamListener<VideoState>;

fn video_stream(
    core: CoreRc,
    source_node: u32,
    request: &Request,
    ender: Rc<Ender>,
) -> Result<(StreamRc, VideoListener), CaptureError> {
    let mut properties = PropertiesBox::new();
    properties.insert("media.type", "Video");
    properties.insert("media.category", "Capture");
    properties.insert("media.role", "Screen");
    let stream = StreamRc::new(core, "ngine-capture-screen", properties)
        .map_err(|error| failed("could not create the screen stream", &error))?;
    let header_meta = frame_header_meta()?;
    let state = VideoState {
        format: VideoInfoRaw::new(),
        lifecycle: Lifecycle::default(),
        limiter: FrameLimiter::new(request.frame_rate),
        ender,
    };
    let listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(|_, state, _, new| {
            let event = change_of(&new).and_then(|change| state.lifecycle.on(change));
            state.ender.report(event);
        })
        .param_changed(move |stream, state, id, param| {
            let Some(param) = raw_format_in(id, param, MediaType::Video) else {
                return;
            };
            if state.format.parse(param).is_err() {
                return;
            }
            if let Some(meta) = Pod::from_bytes(&header_meta) {
                let _frames_are_stamped_on_arrival_instead = stream.update_params(&mut [meta]);
            }
        })
        .process(|stream, state| {
            if !state.lifecycle.has_ended() {
                deliver_frame(stream, state);
            }
        })
        .register()
        .map_err(|error| failed("could not listen to the screen stream", &error))?;
    let empty = || CaptureError::Os("the video format description is empty".to_owned());
    let in_shared_memory = video_formats(request.pixel_format, request.frame_rate, Memory::Shared)?;
    let in_a_dma_buf = video_formats(request.pixel_format, request.frame_rate, Memory::DmaBuf)?;
    let shared = Pod::from_bytes(&in_shared_memory).ok_or_else(empty)?;
    let mut offered = Vec::new();
    if request.gpu_surfaces {
        offered.push(Pod::from_bytes(&in_a_dma_buf).ok_or_else(empty)?);
    }
    offered.push(shared);
    stream
        .connect(
            Direction::Input,
            Some(source_node),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut offered,
        )
        .map_err(|error| failed("could not connect the screen stream", &error))?;
    Ok((stream, listener))
}

struct AudioState {
    source: AudioSource,
    format: AudioInfoRaw,
    sink: EventSink,
}

fn graph_time(stream: &pw::stream::Stream) -> Option<GraphTime> {
    let time = stream.time().ok()?;
    Some(GraphTime {
        now_nanos: time.now(),
        delay_ticks: time.delay(),
        tick_numerator: time.rate().num,
        tick_denominator: time.rate().denom,
    })
}

fn deliver_sound(stream: &pw::stream::Stream, state: &AudioState) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let Some(data) = buffer.datas_mut().first_mut() else {
        return;
    };
    let offset = usize::try_from(data.chunk().offset()).unwrap_or(usize::MAX);
    let size = usize::try_from(data.chunk().size()).unwrap_or(0);
    let Ok(channels) = u16::try_from(state.format.channels()) else {
        return;
    };
    let Some(mapped) = data.data() else {
        return;
    };
    let decoded = samples::f32_from_le_bytes(mapped, offset, size, channels);
    let timestamp = clock::audio_timestamp(graph_time(stream), now());
    let buffer = AudioBuffer::new(
        state.source,
        timestamp,
        state.format.rate(),
        channels,
        decoded,
    );
    if let Some(buffer) = buffer.filter(|buffer| buffer.frames() > 0) {
        state.sink.audio(buffer);
    }
}

type AudioListener = pw::stream::StreamListener<AudioState>;

fn audio_stream(
    core: CoreRc,
    source: AudioSource,
    target: Option<String>,
    captures_a_sink: bool,
    sink: EventSink,
) -> Result<(StreamRc, AudioListener), CaptureError> {
    let mut properties = PropertiesBox::new();
    properties.insert("media.type", "Audio");
    properties.insert("media.category", "Capture");
    properties.insert("media.role", "Production");
    if captures_a_sink {
        properties.insert("stream.capture.sink", "true");
    }
    if let Some(target) = target {
        properties.insert("target.object", target);
    }
    let name = match source {
        AudioSource::Microphone => "ngine-capture-microphone",
        _ => "ngine-capture-system-sound",
    };
    let stream = StreamRc::new(core, name, properties)
        .map_err(|error| failed("could not create a sound stream", &error))?;
    let state = AudioState {
        source,
        format: AudioInfoRaw::new(),
        sink,
    };
    let listener = stream
        .add_local_listener_with_user_data(state)
        .param_changed(|_, state, id, param| {
            if let Some(param) = raw_format_in(id, param, MediaType::Audio) {
                let _keeps_the_previous_format = state.format.parse(param);
            }
        })
        .process(|stream, state| deliver_sound(stream, state))
        .register()
        .map_err(|error| failed("could not listen to a sound stream", &error))?;
    let formats = float_audio()?;
    let offered = Pod::from_bytes(&formats)
        .ok_or_else(|| CaptureError::Os("the audio format description is empty".to_owned()))?;
    stream
        .connect(
            Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut [offered],
        )
        .map_err(|error| failed("could not connect a sound stream", &error))?;
    Ok((stream, listener))
}

fn run(
    request: Request,
    sink: &EventSink,
    stop_requests: pw::channel::Receiver<()>,
    stop: &pw::channel::Sender<()>,
    ready: &mpsc::Sender<Result<(), CaptureError>>,
) -> Result<(), CaptureError> {
    pw::init();
    let main_loop = MainLoopRc::new(None)
        .map_err(|error| unavailable("PipeWire could not create its loop", &error))?;
    let context = ContextRc::new(&main_loop, None)
        .map_err(|error| unavailable("PipeWire could not create its context", &error))?;
    let quitting = main_loop.clone();
    let _stop_listener = stop_requests.attach(main_loop.loop_(), move |()| quitting.quit());
    let mut request = request;
    let ender = Rc::new(Ender {
        sink: sink.clone(),
        stop: stop.clone(),
        reporter: request.reporter.take(),
    });
    let wants_sound = request.wants_sound();
    let mut streams = Vec::new();
    let mut video_listeners = Vec::new();
    let mut audio_listeners = Vec::new();
    let mut core_listeners = Vec::new();
    let mut watch = |core: &CoreRc, what: &'static str| {
        let ender = Rc::clone(&ender);
        core_listeners.push(
            core.add_listener_local()
                .error(move |id, _, result, message| {
                    if id != pw::core::PW_ID_CORE || result != BROKEN_PIPE {
                        return;
                    }
                    let gone = CaptureError::Unavailable(format!("{what} went away: {message}"));
                    ender.report(Some(Event::Ended(EndReason::Failed(gone))));
                })
                .register(),
        );
    };
    if let Some(video) = request.video.take() {
        let core = context
            .connect_fd_rc(video.remote, None)
            .map_err(|error| unavailable("the portal's PipeWire connection failed", &error))?;
        watch(&core, "the portal's PipeWire connection");
        let (stream, listener) = video_stream(core, video.node_id, &request, Rc::clone(&ender))?;
        streams.push(stream);
        video_listeners.push(listener);
    }
    if wants_sound {
        let core = context
            .connect_rc(None)
            .map_err(|error| unavailable("PipeWire is not running", &error))?;
        watch(&core, "PipeWire");
        let system = match &request.system {
            SystemSound::Off => None,
            SystemSound::Everything => Some((None, true)),
            SystemSound::Node(id) => Some((Some(id.to_string()), false)),
        };
        if let Some((target, captures_a_sink)) = system {
            let (stream, listener) = audio_stream(
                core.clone(),
                AudioSource::System,
                target,
                captures_a_sink,
                sink.clone(),
            )?;
            streams.push(stream);
            audio_listeners.push(listener);
        }
        let microphone = match &request.microphone {
            MicrophoneChoice::Off => None,
            MicrophoneChoice::SystemDefault => Some(None),
            MicrophoneChoice::Named(name) => Some(Some(name.clone())),
        };
        if let Some(target) = microphone {
            let (stream, listener) =
                audio_stream(core, AudioSource::Microphone, target, false, sink.clone())?;
            streams.push(stream);
            audio_listeners.push(listener);
        }
    }
    let _start_gave_up_waiting = ready.send(Ok(()));
    main_loop.run();
    drop((video_listeners, audio_listeners, core_listeners, streams));
    Ok(())
}

#[derive(Debug, Default)]
pub(super) struct Graph {
    pub(super) nodes: Vec<Node>,
    pub(super) default_microphone: Option<String>,
}

fn wait_for_the_answers(main_loop: &MainLoopRc, core: &CoreRc) -> Result<(), CaptureError> {
    let answered = Rc::new(Cell::new(false));
    let pending = core
        .sync(0)
        .map_err(|error| unavailable("PipeWire did not accept a request", &error))?;
    let _listener = core
        .add_listener_local()
        .done({
            let answered = Rc::clone(&answered);
            let main_loop = main_loop.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    answered.set(true);
                    main_loop.quit();
                }
            }
        })
        .register();
    let giving_up = main_loop.clone();
    let timer = main_loop.loop_().add_timer(move |_| giving_up.quit());
    let _armed = timer.update_timer(Some(ANSWERS_WITHIN), None);
    main_loop.run();
    if answered.get() {
        Ok(())
    } else {
        Err(CaptureError::Unavailable(
            "PipeWire did not answer in time".to_owned(),
        ))
    }
}

pub(super) fn graph() -> Result<Graph, CaptureError> {
    pw::init();
    let main_loop = MainLoopRc::new(None)
        .map_err(|error| unavailable("PipeWire could not create its loop", &error))?;
    let context = ContextRc::new(&main_loop, None)
        .map_err(|error| unavailable("PipeWire could not create its context", &error))?;
    let core = context
        .connect_rc(None)
        .map_err(|error| unavailable("PipeWire is not running", &error))?;
    let registry = core
        .get_registry()
        .map_err(|error| unavailable("PipeWire gave no registry", &error))?;
    let found = Rc::new(RefCell::new(Vec::new()));
    let defaults = Rc::new(RefCell::new(None));
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let found = Rc::clone(&found);
            let defaults = Rc::clone(&defaults);
            move |global| {
                let Some(properties) = global.props else {
                    return;
                };
                match global.type_ {
                    pw::types::ObjectType::Node => found.borrow_mut().push(Node {
                        id: global.id,
                        properties: properties
                            .iter()
                            .map(|(key, value)| (key.to_owned(), value.to_owned()))
                            .collect(),
                    }),
                    pw::types::ObjectType::Metadata
                        if properties.get("metadata.name") == Some("default") =>
                    {
                        *defaults.borrow_mut() = Some(global.to_owned());
                    }
                    _ => {}
                }
            }
        })
        .register();
    wait_for_the_answers(&main_loop, &core)?;
    let default_microphone = Rc::new(RefCell::new(None));
    let metadata = defaults
        .borrow()
        .as_ref()
        .and_then(|global| registry.bind::<pw::metadata::Metadata, _>(global).ok());
    let _metadata_listener = metadata.as_ref().map(|metadata| {
        let default_microphone = Rc::clone(&default_microphone);
        metadata
            .add_listener_local()
            .property(move |_, key, _, value| {
                if key == Some(DEFAULT_SOURCE_KEY) {
                    *default_microphone.borrow_mut() = value.and_then(nodes::default_node_name);
                }
                0
            })
            .register()
    });
    if metadata.is_some() {
        wait_for_the_answers(&main_loop, &core)?;
    }
    Ok(Graph {
        nodes: found.take(),
        default_microphone: default_microphone.take(),
    })
}

pub(super) fn playing_apps() -> Result<Vec<crate::target::AudioApp>, CaptureError> {
    Ok(nodes::audio_apps(&graph()?.nodes, std::process::id()))
}

pub(super) fn microphone_devices() -> Result<Vec<crate::target::MicrophoneDevice>, CaptureError> {
    let graph = graph()?;
    Ok(nodes::microphones(
        &graph.nodes,
        graph.default_microphone.as_deref(),
    ))
}

pub(super) fn system_sound(
    wanted: &crate::options::SystemAudio,
) -> Result<SystemSound, CaptureError> {
    use crate::options::SystemAudio;

    match wanted {
        SystemAudio::Off => Ok(SystemSound::Off),
        SystemAudio::Everything => Ok(SystemSound::Everything),
        SystemAudio::App(app) => nodes::playback_node_of(&graph()?.nodes, app)
            .map(SystemSound::Node)
            .ok_or_else(|| {
                CaptureError::Unavailable(format!(
                    "{} is not playing sound, so it has no sound stream to capture",
                    app.name
                ))
            }),
    }
}

pub(super) fn microphone_choice(wanted: &crate::options::Microphone) -> MicrophoneChoice {
    use crate::options::Microphone;

    match wanted {
        Microphone::Off => MicrophoneChoice::Off,
        Microphone::SystemDefault => MicrophoneChoice::SystemDefault,
        Microphone::Device(name) => MicrophoneChoice::Named(name.clone()),
    }
}
