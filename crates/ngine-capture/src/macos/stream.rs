use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use screencapturekit::audio_devices::AudioInputDevice;
use screencapturekit::cm::{
    CMClock, CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, FrameInfo,
};
use screencapturekit::cv::CVPixelBuffer;
use screencapturekit::stream::configuration::{
    PixelFormat as NativePixelFormat, SCStreamConfiguration,
};
use screencapturekit::stream::content_filter::SCContentFilter;
use screencapturekit::stream::output_type::SCStreamOutputType;
use screencapturekit::stream::{ErrorHandler, SCStream};

use super::content::{self, Resolved, Sound};
use super::surface::CoreVideoSurface;
use super::{audio, convert};
use crate::audio::AudioSource;
use crate::error::CaptureError;
use crate::frame::{PixelFormat, Size, Surface, VideoFrame};
use crate::options::{Microphone, SessionOptions, SystemAudio};
use crate::session::{EventSink, RunningCapture};
use crate::supervisor::{self, Launch, Signal, Supervised, Supervisee};
use crate::target::Target;
use crate::time::Timestamp;

const SAMPLE_RATE: i32 = 48_000;
const CHANNELS: i32 = 2;
const FRAMES_SCREENCAPTUREKIT_MAY_HOLD: u32 = 8;
const SOUND_ONLY_PICTURE: Size = Size::new(16, 16);
const SOUND_ONLY_FRAME_RATE: u32 = 1;
static RESTART_DELAYS: [Duration; 5] = [
    Duration::ZERO,
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const NV12_VIDEO_RANGE: u32 = u32::from_be_bytes(*b"420v");

fn stop_unless_already_stopped(stream: &SCStream) {
    drop(stream.stop_capture());
}

struct Streams {
    picture: SCStream,
    sound_only: Option<SCStream>,
    configuration: SCStreamConfiguration,
    size: Size,
}

impl Supervisee for Streams {
    fn stop(self) {
        stop_unless_already_stopped(&self.picture);
        if let Some(sound_only) = &self.sound_only {
            stop_unless_already_stopped(sound_only);
        }
    }

    fn follow(&mut self, size: Size) {
        if self.size == size {
            return;
        }
        self.configuration
            .set_width(size.width)
            .set_height(size.height);
        if self
            .picture
            .update_configuration(&self.configuration)
            .is_ok()
        {
            self.size = size;
        }
    }
}

fn native_format(format: PixelFormat) -> NativePixelFormat {
    match format {
        PixelFormat::Nv12 => NativePixelFormat::YCbCr_420v,
        PixelFormat::Bgra8 | PixelFormat::Rgba8 => NativePixelFormat::BGRA,
    }
}

fn delivered_format(four_character_code: u32) -> Option<PixelFormat> {
    match four_character_code {
        BGRA => Some(PixelFormat::Bgra8),
        NV12_VIDEO_RANGE => Some(PixelFormat::Nv12),
        _ => None,
    }
}

fn size_of(pixel_buffer: &CVPixelBuffer) -> Option<Size> {
    let width = u32::try_from(pixel_buffer.width()).ok()?;
    let height = u32::try_from(pixel_buffer.height()).ok()?;
    Some(Size::new(width, height))
}

fn native_size(info: &FrameInfo) -> Option<Size> {
    let content = info.content_rect?.size;
    let pixels_per_point = info.scale_factor?;
    let scaling = info.content_scale.filter(|scaling| *scaling > 0.0)?;
    let width = convert::whole_number(content.width / scaling * pixels_per_point)?;
    let height = convert::whole_number(content.height / scaling * pixels_per_point)?;
    (width > 0 && height > 0).then_some(Size::new(width, height))
}

fn packed(size: Size) -> u64 {
    u64::from(size.width) << 32 | u64::from(size.height)
}

struct NativeSizeWatch {
    last_seen: AtomicU64,
    generation: u64,
    signals: Sender<Signal>,
}

impl NativeSizeWatch {
    fn see(&self, native: Size, delivered: Size) {
        let changed = self.last_seen.swap(packed(native), Ordering::AcqRel) != packed(native);
        if changed && native != delivered {
            supervisor::send_unless_the_supervisor_is_gone(
                &self.signals,
                Signal::NativeSize {
                    generation: self.generation,
                    size: native,
                },
            );
        }
    }
}

fn deliver_picture(sample: CMSampleBuffer, sink: &EventSink, watch: &NativeSizeWatch) {
    let info = sample.frame_info();
    let is_empty = info
        .as_ref()
        .and_then(|info| info.frame_status)
        .is_some_and(|status| !status.has_content());
    if is_empty {
        return;
    }
    let Some(timestamp) = convert::timestamp(sample.presentation_timestamp()) else {
        return;
    };
    let Some(pixel_buffer) = sample.pixel_buffer() else {
        return;
    };
    let Some((size, format)) =
        size_of(&pixel_buffer).zip(delivered_format(pixel_buffer.pixel_format()))
    else {
        return;
    };
    if let Some(native) = info.as_ref().and_then(native_size) {
        watch.see(native, size);
    }
    let surface = Surface::CoreVideo(CoreVideoSurface::new(sample, pixel_buffer));
    sink.video(VideoFrame::from_surface(timestamp, size, format, surface));
}

fn deliver_sound(sample: &CMSampleBuffer, source: AudioSource, sink: &EventSink) {
    if let Some(buffer) = audio::buffer_from(sample, source) {
        sink.audio(buffer);
    }
}

fn wanted_sound(system_audio: &SystemAudio) -> Option<Sound> {
    match system_audio {
        SystemAudio::Off => None,
        SystemAudio::Everything => Some(Sound::Everything),
        SystemAudio::App(app) => Some(Sound::App(app.id)),
    }
}

fn require_connected(microphone: &Microphone) -> Result<(), CaptureError> {
    let devices = match microphone {
        Microphone::Off => return Ok(()),
        Microphone::SystemDefault | Microphone::Device(_) => AudioInputDevice::list(),
    };
    let connected = match microphone {
        Microphone::Device(id) => devices.iter().any(|device| &device.id == id),
        Microphone::Off | Microphone::SystemDefault => !devices.is_empty(),
    };
    if connected {
        Ok(())
    } else {
        Err(CaptureError::Unavailable(
            "the microphone that was asked for is not connected".to_owned(),
        ))
    }
}

fn with_sound(configuration: SCStreamConfiguration) -> SCStreamConfiguration {
    configuration
        .with_captures_audio(true)
        .with_sample_rate(SAMPLE_RATE)
        .with_channel_count(CHANNELS)
        .with_excludes_current_process_audio(true)
}

fn open(
    filter: &SCContentFilter,
    configuration: &SCStreamConfiguration,
    generation: u64,
    signals: &Sender<Signal>,
) -> Result<SCStream, CaptureError> {
    let signals = signals.clone();
    let delegate = ErrorHandler::new(move |error| {
        supervisor::send_unless_the_supervisor_is_gone(
            &signals,
            Signal::Stopped {
                generation,
                error: convert::error_from(&error),
                by_user: convert::was_stopped_by_the_user(&error),
            },
        );
    });
    SCStream::new_with_delegate(filter, configuration, delegate)
        .map_err(|error| convert::error_from(&error))
}

fn listen(
    stream: &mut SCStream,
    source: AudioSource,
    sink: &EventSink,
) -> Result<(), CaptureError> {
    let output = match source {
        AudioSource::System => SCStreamOutputType::Audio,
        AudioSource::Microphone => SCStreamOutputType::Microphone,
    };
    let sink = sink.clone();
    stream
        .add_output_handler(
            move |sample: CMSampleBuffer, _: SCStreamOutputType| {
                deliver_sound(&sample, source, &sink);
            },
            output,
        )
        .map(|_handler_id| ())
        .map_err(|error| convert::error_from(&error))
}

fn begin(stream: &SCStream) -> Result<(), CaptureError> {
    stream
        .start_capture()
        .map_err(|error| convert::error_from(&error))
}

#[derive(Clone)]
struct Filters {
    picture: Resolved,
    sound_only: Option<SCContentFilter>,
}

impl Filters {
    fn resolve(target: &Target, wanted: Option<Sound>) -> Result<Self, CaptureError> {
        let picture = content::resolve(target)?;
        let sound_only = match wanted {
            Some(sound) if sound != picture.sound => Some(match sound {
                Sound::App(process_id) => content::only_audible(process_id)?,
                Sound::Everything | Sound::Unknown => content::everything_audible()?,
            }),
            Some(_) | None => None,
        };
        Ok(Self {
            picture,
            sound_only,
        })
    }
}

struct Plan {
    target: Target,
    options: SessionOptions,
    filters_of_the_last_launch: Mutex<Option<Filters>>,
}

impl Plan {
    fn picture_configuration(
        &self,
        size: Size,
        carries_sound: bool,
    ) -> Result<SCStreamConfiguration, CaptureError> {
        let mut configuration = SCStreamConfiguration::new()
            .with_width(size.width)
            .with_height(size.height)
            .with_fps(self.options.frame_rate)
            .with_shows_cursor(self.options.show_cursor)
            .with_pixel_format(native_format(self.options.pixel_format))
            .with_queue_depth(FRAMES_SCREENCAPTUREKIT_MAY_HOLD);
        if carries_sound {
            configuration = with_sound(configuration);
        }
        require_connected(&self.options.microphone)?;
        if self.options.microphone != Microphone::Off {
            configuration
                .set_captures_microphone(true)
                .map_err(|error| convert::error_from(&error))?;
        }
        if let Microphone::Device(id) = &self.options.microphone {
            configuration
                .set_microphone_capture_device_id(id)
                .map_err(|error| convert::error_from(&error))?;
        }
        Ok(configuration)
    }

    fn sound_only_stream(
        filter: &SCContentFilter,
        generation: u64,
        sink: &EventSink,
        signals: &Sender<Signal>,
    ) -> Result<SCStream, CaptureError> {
        let configuration = with_sound(
            SCStreamConfiguration::new()
                .with_width(SOUND_ONLY_PICTURE.width)
                .with_height(SOUND_ONLY_PICTURE.height)
                .with_fps(SOUND_ONLY_FRAME_RATE)
                .with_shows_cursor(false),
        );
        let mut stream = open(filter, &configuration, generation, signals)?;
        listen(&mut stream, AudioSource::System, sink)?;
        Ok(stream)
    }

    fn new(target: &Target, options: &SessionOptions) -> Self {
        Self {
            target: target.clone(),
            options: options.clone(),
            filters_of_the_last_launch: Mutex::new(None),
        }
    }

    fn remembered_filters(&self) -> Option<Filters> {
        self.filters_of_the_last_launch
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn remember(&self, filters: Option<Filters>) {
        *self
            .filters_of_the_last_launch
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = filters;
    }

    fn launch_with(
        &self,
        filters: &Filters,
        generation: u64,
        sink: &EventSink,
        signals: &Sender<Signal>,
    ) -> Result<Streams, CaptureError> {
        let resolved = &filters.picture;
        let picture_carries_sound =
            wanted_sound(&self.options.system_audio) == Some(resolved.sound);
        let configuration = self.picture_configuration(resolved.size, picture_carries_sound)?;
        let mut picture = open(&resolved.filter, &configuration, generation, signals)?;
        let watch = NativeSizeWatch {
            last_seen: AtomicU64::new(packed(resolved.size)),
            generation,
            signals: signals.clone(),
        };
        let picture_sink = sink.clone();
        picture
            .add_output_handler(
                move |sample: CMSampleBuffer, _: SCStreamOutputType| {
                    deliver_picture(sample, &picture_sink, &watch);
                },
                SCStreamOutputType::Screen,
            )
            .map_err(|error| convert::error_from(&error))?;
        if picture_carries_sound {
            listen(&mut picture, AudioSource::System, sink)?;
        }
        if self.options.microphone != Microphone::Off {
            listen(&mut picture, AudioSource::Microphone, sink)?;
        }
        let sound_only = filters
            .sound_only
            .as_ref()
            .map(|filter| Self::sound_only_stream(filter, generation, sink, signals))
            .transpose()?;
        let streams = Streams {
            picture,
            sound_only,
            configuration,
            size: resolved.size,
        };
        let begun = streams
            .sound_only
            .as_ref()
            .map_or(Ok(()), begin)
            .and_then(|()| begin(&streams.picture));
        if let Err(error) = begun {
            streams.stop();
            return Err(error);
        }
        Ok(streams)
    }
}

impl Launch for Plan {
    type Capture = Streams;

    fn launch(
        &self,
        generation: u64,
        sink: &EventSink,
        signals: &Sender<Signal>,
    ) -> Result<Streams, CaptureError> {
        if let Some(filters) = self.remembered_filters()
            && let Ok(streams) = self.launch_with(&filters, generation, sink, signals)
        {
            return Ok(streams);
        }
        self.remember(None);
        let filters = Filters::resolve(&self.target, wanted_sound(&self.options.system_audio))?;
        let streams = self.launch_with(&filters, generation, sink, signals)?;
        self.remember(Some(filters));
        Ok(streams)
    }
}

struct MacCapture {
    supervised: Supervised<Streams>,
}

impl RunningCapture for MacCapture {
    fn stop(&mut self) {
        self.supervised.stop();
    }

    fn now(&self) -> Option<Timestamp> {
        convert::timestamp(CMClock::host_time_clock().time())
    }
}

pub(super) fn start(
    target: &Target,
    options: &SessionOptions,
    sink: EventSink,
) -> Result<Box<dyn RunningCapture>, CaptureError> {
    let plan = Plan::new(target, options);
    let supervised = supervisor::supervise(plan, sink, &RESTART_DELAYS)?;
    Ok(Box::new(MacCapture { supervised }))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use screencapturekit::cg::CGRect;

    use super::*;

    fn info(points: (f64, f64), pixels_per_point: f64, scaling: f64) -> FrameInfo {
        FrameInfo {
            content_rect: Some(CGRect::new(0.0, 0.0, points.0, points.1)),
            scale_factor: Some(pixels_per_point),
            content_scale: Some(scaling),
            ..FrameInfo::default()
        }
    }

    #[test]
    fn unscaled_content_has_its_points_times_the_display_scale_in_pixels() {
        assert_eq!(
            native_size(&info((800.0, 600.0), 2.0, 1.0)),
            Some(Size::new(1600, 1200))
        );
    }

    #[test]
    fn content_that_the_stream_shrank_is_larger_on_screen_than_in_the_frame() {
        assert_eq!(
            native_size(&info((400.0, 300.0), 2.0, 0.5)),
            Some(Size::new(1600, 1200))
        );
    }

    #[test]
    fn a_frame_without_its_measurements_has_no_native_size() {
        assert_eq!(native_size(&FrameInfo::default()), None);
        assert_eq!(native_size(&info((800.0, 600.0), 2.0, 0.0)), None);
        assert_eq!(native_size(&info((0.0, 0.0), 2.0, 1.0)), None);
    }

    #[test]
    fn a_new_native_size_that_differs_from_the_frame_is_reported_once() {
        let (signals, inbox) = mpsc::channel();
        let configured = Size::new(1600, 1200);
        let watch = NativeSizeWatch {
            last_seen: AtomicU64::new(packed(configured)),
            generation: 3,
            signals,
        };
        let resized = Size::new(2000, 1000);

        watch.see(configured, configured);
        watch.see(resized, configured);
        watch.see(resized, configured);

        assert!(matches!(
            inbox.try_recv(),
            Ok(Signal::NativeSize { generation: 3, size }) if size == resized
        ));
        assert!(inbox.try_recv().is_err());
    }

    #[test]
    fn the_sound_of_the_filter_decides_whether_a_second_stream_is_needed() {
        let app = crate::target::AudioApp::new(7, "Player");

        assert_eq!(wanted_sound(&SystemAudio::Off), None);
        assert_eq!(
            wanted_sound(&SystemAudio::Everything),
            Some(Sound::Everything)
        );
        assert_eq!(wanted_sound(&SystemAudio::App(app)), Some(Sound::App(7)));
        assert_ne!(wanted_sound(&SystemAudio::Everything), Some(Sound::Unknown));
    }

    #[test]
    fn only_the_formats_that_were_asked_for_are_recognised() {
        assert_eq!(delivered_format(BGRA), Some(PixelFormat::Bgra8));
        assert_eq!(delivered_format(NV12_VIDEO_RANGE), Some(PixelFormat::Nv12));
        assert_eq!(delivered_format(u32::from_be_bytes(*b"l10r")), None);
    }

    #[test]
    #[ignore = "needs the screen recording permission and a display"]
    fn a_stream_that_the_system_stopped_delivers_frames_again_within_a_quarter_second() {
        use std::time::Instant;

        use crate::event::Event;
        use crate::session;

        let screen = content::list().unwrap().screens.remove(0);
        let plan = Plan::new(
            &Target::from(screen),
            &SessionOptions::new()
                .with_pixel_format(PixelFormat::Nv12)
                .with_system_audio(SystemAudio::Everything),
        );
        let (sink, pending) = session::channel();
        let supervised = supervisor::supervise(plan, sink, &RESTART_DELAYS).unwrap();
        let signals = supervised.signals();
        let session = pending.run(Box::new(MacCapture { supervised }));
        let wait_for = |wanted: fn(&Event) -> bool| {
            let asked = Instant::now();
            loop {
                assert!(asked.elapsed() < Duration::from_secs(10), "nothing arrived");
                match session.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) if wanted(&event) => return asked.elapsed(),
                    _ => {}
                }
            }
        };
        wait_for(|event| matches!(event, Event::Video(_)));

        for generation in 0..3 {
            std::thread::sleep(Duration::from_secs(1));
            signals
                .send(Signal::Stopped {
                    generation,
                    error: CaptureError::Os("System stopped the stream".to_owned()),
                    by_user: false,
                })
                .unwrap();
            let resumed = wait_for(|event| matches!(event, Event::Resumed));
            let gap = resumed + wait_for(|event| matches!(event, Event::Video(_)));
            eprintln!(
                "restart {generation}: resumed after {} ms, frames again after {} ms",
                resumed.as_millis(),
                gap.as_millis()
            );
            assert!(gap < Duration::from_millis(250), "{gap:?}");
        }
    }

    #[test]
    fn a_microphone_that_is_off_needs_no_device() {
        assert_eq!(require_connected(&Microphone::Off), Ok(()));
    }
}
