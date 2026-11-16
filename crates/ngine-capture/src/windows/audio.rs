use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use wasapi::{
    AudioCaptureClient, AudioClient, DeviceEnumerator, DeviceEventCallbacks,
    DeviceEventRegistration, Direction, Handle, Role, SampleType, StreamMode, WasapiError,
    WaveFormat,
};

use super::clock::{BufferTimeline, PerformanceCounterReadThroughInstant};
use super::convert::{self, Input};
use crate::audio::{AudioBuffer, AudioSource};
use crate::error::CaptureError;
use crate::session::EventSink;
use crate::supervisor::{self, Signal};
use crate::target::MicrophoneDevice;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: usize = 32;
const DEVICE_BUFFER_IN_HUNDREDS_OF_NANOS: i64 = 2_000_000;
const SMALLEST_BUFFER_WINDOWS_ALLOWS: i64 = 0;
const LONGEST_WAIT_FOR_SOUND_IN_MILLIS: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Source {
    EverythingButThisProcess,
    App(u32),
    DefaultMicrophone,
    Microphone(String),
}

impl Source {
    fn input(&self) -> Input {
        match self {
            Self::EverythingButThisProcess | Self::App(_) => Input::SystemSound,
            Self::DefaultMicrophone | Self::Microphone(_) => Input::Microphone,
        }
    }

    fn delivers(&self) -> AudioSource {
        match self.input() {
            Input::Microphone => AudioSource::Microphone,
            Input::SystemSound | Input::Picture => AudioSource::System,
        }
    }
}

pub(super) struct Tap {
    pub(super) sink: EventSink,
    pub(super) signals: Sender<Signal>,
    pub(super) generation: u64,
    pub(super) clock: Arc<PerformanceCounterReadThroughInstant>,
}

fn error_from(error: &WasapiError, input: Input) -> CaptureError {
    match error {
        WasapiError::Windows(error) | WasapiError::RegisterNotifications(error) => {
            convert::error_from_code(error.code().0, &error.message(), input)
        }
        WasapiError::DeviceNotFound(_) => CaptureError::Unavailable(error.to_string()),
        _ => CaptureError::Os(error.to_string()),
    }
}

fn format() -> WaveFormat {
    WaveFormat::new(
        BITS_PER_SAMPLE,
        BITS_PER_SAMPLE,
        SampleType::Float,
        SAMPLE_RATE as usize,
        usize::from(CHANNELS),
        None,
    )
}

fn shared_with_events(buffer_duration_hns: i64) -> StreamMode {
    StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns,
    }
}

struct DefaultDeviceWatch {
    changed: Arc<AtomicBool>,
    _registration: DeviceEventRegistration,
}

fn watch_default(
    devices: &DeviceEnumerator,
    watched: Direction,
) -> Result<DefaultDeviceWatch, WasapiError> {
    let changed = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&changed);
    let mut callbacks = DeviceEventCallbacks::new();
    callbacks.set_default_device_callback(move |direction, role, _device_id| {
        if direction == watched && role == Role::Console {
            seen.store(true, Ordering::Release);
        }
    });
    Ok(DefaultDeviceWatch {
        changed,
        _registration: devices.register_notification_callback(callbacks)?,
    })
}

struct Stream {
    client: AudioClient,
    capture: AudioCaptureClient,
    wake: Handle,
    default_device: Option<DefaultDeviceWatch>,
}

fn process_loopback(
    process_id: u32,
    includes_the_process: bool,
) -> Result<AudioClient, WasapiError> {
    let mut client =
        AudioClient::new_application_loopback_client(process_id, includes_the_process)?;
    client.initialize_client(
        &format(),
        Direction::Capture,
        shared_with_events(SMALLEST_BUFFER_WINDOWS_ALLOWS),
        None,
    )?;
    Ok(client)
}

fn default_device(
    plays_or_records: Direction,
) -> Result<(AudioClient, DefaultDeviceWatch), WasapiError> {
    let devices = DeviceEnumerator::new()?;
    let mut client = devices
        .get_default_device(plays_or_records)?
        .get_iaudioclient()?;
    client.initialize_client(
        &format(),
        Direction::Capture,
        shared_with_events(DEVICE_BUFFER_IN_HUNDREDS_OF_NANOS),
        None,
    )?;
    Ok((client, watch_default(&devices, plays_or_records)?))
}

fn microphone(id: &str) -> Result<AudioClient, WasapiError> {
    let mut client = DeviceEnumerator::new()?
        .get_device(id)?
        .get_iaudioclient()?;
    client.initialize_client(
        &format(),
        Direction::Capture,
        shared_with_events(DEVICE_BUFFER_IN_HUNDREDS_OF_NANOS),
        None,
    )?;
    Ok(client)
}

fn open(source: &Source) -> Result<Stream, WasapiError> {
    let (client, default_device) = match source {
        Source::App(process_id) => (process_loopback(*process_id, true)?, None),
        Source::EverythingButThisProcess => match process_loopback(std::process::id(), false) {
            Ok(client) => (client, None),
            Err(_older_than_windows_10_2004) => {
                let (client, watch) = default_device(Direction::Render)?;
                (client, Some(watch))
            }
        },
        Source::DefaultMicrophone => {
            let (client, watch) = default_device(Direction::Capture)?;
            (client, Some(watch))
        }
        Source::Microphone(id) => (microphone(id)?, None),
    };
    let wake = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    Ok(Stream {
        client,
        capture,
        wake,
        default_device,
    })
}

fn link_the_clock(stream: &Stream, clock: &PerformanceCounterReadThroughInstant) {
    let counter_now = stream
        .client
        .get_audioclock()
        .and_then(|device_clock| device_clock.get_position())
        .ok()
        .and_then(|(_position, counter)| convert::device_timestamp(counter, true));
    if let Some(now) = counter_now {
        clock.observe(now);
    }
}

fn pump(
    stream: &Stream,
    delivers: AudioSource,
    tap: &Tap,
    stopping: &AtomicBool,
) -> Result<(), CaptureError> {
    let input = match delivers {
        AudioSource::Microphone => Input::Microphone,
        AudioSource::System => Input::SystemSound,
    };
    let failed = |error: WasapiError| error_from(&error, input);
    let mut bytes = VecDeque::new();
    let mut timeline = BufferTimeline::default();
    while !stopping.load(Ordering::Acquire) {
        let _silence_wakes_nobody = stream
            .wake
            .wait_for_event(LONGEST_WAIT_FOR_SOUND_IN_MILLIS)
            .is_ok();
        let default_changed = stream
            .default_device
            .as_ref()
            .is_some_and(|watch| watch.changed.load(Ordering::Acquire));
        if default_changed {
            return Err(CaptureError::Unavailable(
                "the default sound device changed".to_owned(),
            ));
        }
        while stream
            .capture
            .get_next_packet_size()
            .map_err(failed)?
            .unwrap_or(0)
            > 0
        {
            bytes.clear();
            let info = stream
                .capture
                .read_from_device_to_deque(&mut bytes)
                .map_err(failed)?;
            let samples = convert::samples(bytes.make_contiguous());
            let frames = samples.len() / usize::from(CHANNELS);
            let from_device =
                convert::device_timestamp(info.timestamp, !info.flags.timestamp_error);
            if let Some(stamp) = from_device {
                tap.clock.observe(stamp);
            }
            let timestamp = timeline.stamp(
                from_device,
                convert::length(frames, SAMPLE_RATE),
                tap.clock.now(),
            );
            let buffer = timestamp.and_then(|timestamp| {
                AudioBuffer::new(delivers, timestamp, SAMPLE_RATE, CHANNELS, samples)
            });
            if let Some(buffer) = buffer {
                tap.sink.audio(buffer);
            }
        }
    }
    Ok(())
}

fn record(
    source: &Source,
    tap: &Tap,
    stopping: &AtomicBool,
    opened: &mpsc::SyncSender<Result<(), CaptureError>>,
) {
    let input = source.input();
    let started = open(source).and_then(|stream| {
        link_the_clock(&stream, &tap.clock);
        stream.client.start_stream()?;
        Ok(stream)
    });
    let stream = match started {
        Ok(stream) => stream,
        Err(error) => {
            drop(opened.send(Err(error_from(&error, input))));
            return;
        }
    };
    drop(opened.send(Ok(())));
    let outcome = pump(&stream, source.delivers(), tap, stopping);
    let already_stopped_when_the_device_went_away = stream.client.stop_stream();
    drop(already_stopped_when_the_device_went_away);
    if let Err(error) = outcome {
        supervisor::send_unless_the_supervisor_is_gone(
            &tap.signals,
            Signal::Stopped {
                generation: tap.generation,
                error,
                by_user: false,
            },
        );
    }
}

fn on_a_thread_with_com<T>(work: impl FnOnce() -> T) -> Result<T, CaptureError> {
    if wasapi::initialize_mta().is_err() {
        return Err(CaptureError::Os(
            "COM did not start on the sound thread".to_owned(),
        ));
    }
    let done = work();
    wasapi::deinitialize();
    Ok(done)
}

pub(super) struct Sound {
    stopping: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Sound {
    pub(super) fn stop(self) {
        self.stopping.store(true, Ordering::Release);
        drop(self.thread.join());
    }
}

pub(super) fn start(source: Source, tap: Tap) -> Result<Sound, CaptureError> {
    let stopping = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stopping);
    let (opened, outcome) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("ngine-capture-sound".to_owned())
        .spawn(move || {
            let com = on_a_thread_with_com(|| record(&source, &tap, &stop_flag, &opened));
            if let Err(error) = com {
                drop(opened.send(Err(error)));
            }
        })
        .map_err(|error| CaptureError::Os(error.to_string()))?;
    let sound = Sound { stopping, thread };
    match outcome.recv() {
        Ok(Ok(())) => Ok(sound),
        Ok(Err(error)) => {
            sound.stop();
            Err(error)
        }
        Err(_thread_ended_without_a_word) => {
            sound.stop();
            Err(CaptureError::Os("the sound thread ended early".to_owned()))
        }
    }
}

fn list_microphones() -> Result<Vec<MicrophoneDevice>, WasapiError> {
    let devices = DeviceEnumerator::new()?;
    let default_id = devices
        .get_default_device(Direction::Capture)
        .and_then(|device| device.get_id())
        .ok();
    let connected = devices.get_device_collection(Direction::Capture)?;
    let mut microphones = Vec::new();
    for index in 0..connected.get_nbr_devices()? {
        let device = connected.get_device_at_index(index)?;
        let id = device.get_id()?;
        let name = device
            .get_friendlyname()
            .unwrap_or_else(|_| "Microphone".to_owned());
        let is_default = default_id.as_ref() == Some(&id);
        microphones.push(MicrophoneDevice::new(id, name, is_default));
    }
    Ok(microphones)
}

pub(super) fn microphones() -> Result<Vec<MicrophoneDevice>, CaptureError> {
    let listing = std::thread::Builder::new()
        .name("ngine-capture-microphones".to_owned())
        .spawn(|| on_a_thread_with_com(list_microphones))
        .map_err(|error| CaptureError::Os(error.to_string()))?;
    listing
        .join()
        .map_err(|_| CaptureError::Os("listing microphones panicked".to_owned()))??
        .map_err(|error| error_from(&error, Input::Microphone))
}
