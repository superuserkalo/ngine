//! A capture backend that plays a script, for tests that have no screen.
//!
//! A script is any iterator of [`Event`]s. It is delivered in order and as
//! fast as the session is read, never dropped, so a test can run through
//! simulated hours in moments. When a script does not end with
//! [`Event::Ended`], the session ends with [`EndReason::Stopped`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use crate::audio::{AudioBuffer, AudioSource};
use crate::backend::{Backend, ListAudioApps, ListTargets, PickDone, PickTarget};
use crate::capturer::Capturer;
use crate::error::CaptureError;
use crate::event::{EndReason, Event};
use crate::frame::{CpuFrame, Size, VideoFrame};
use crate::options::SessionOptions;
use crate::session::{EventSink, RunningCapture};
use crate::target::{AudioApp, MicrophoneDevice, PickedTarget, PickerOptions, Target, Targets};
use crate::time::Timestamp;

type Script = Box<dyn Iterator<Item = Event> + Send>;

const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Start {
    pub target: Target,
    pub options: SessionOptions,
}

#[derive(Clone, Debug, Default)]
pub struct StartLog {
    starts: Arc<Mutex<Vec<Start>>>,
}

impl StartLog {
    pub fn starts(&self) -> Vec<Start> {
        self.starts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Builds a [`Capturer`] that needs no screen, GPU, audio device or
/// permission.
#[derive(Default)]
pub struct FakeCapture {
    targets: Option<Targets>,
    pick: Option<Result<Option<PickedTarget>, CaptureError>>,
    audio_apps: Option<Vec<AudioApp>>,
    microphones: Vec<MicrophoneDevice>,
    scripts: VecDeque<Script>,
    start_error: Option<CaptureError>,
    log: StartLog,
}

impl FakeCapture {
    /// A fake with no lister, no picker, no list of audio apps and no script.
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_targets(mut self, targets: Targets) -> Self {
        self.targets = Some(targets);
        self
    }

    #[must_use]
    pub fn with_picker(mut self, outcome: Result<Option<PickedTarget>, CaptureError>) -> Self {
        self.pick = Some(outcome);
        self
    }

    #[must_use]
    pub fn with_audio_apps(mut self, apps: Vec<AudioApp>) -> Self {
        self.audio_apps = Some(apps);
        self
    }

    #[must_use]
    pub fn with_microphones(mut self, microphones: Vec<MicrophoneDevice>) -> Self {
        self.microphones = microphones;
        self
    }

    /// Queues the events of one session. Each start plays the next script.
    #[must_use]
    pub fn with_script<S>(mut self, script: S) -> Self
    where
        S: IntoIterator<Item = Event>,
        S::IntoIter: Send + 'static,
    {
        self.scripts.push_back(Box::new(script.into_iter()));
        self
    }

    /// Makes every start fail with `error`, for example a missing permission.
    #[must_use]
    pub fn failing_to_start(mut self, error: CaptureError) -> Self {
        self.start_error = Some(error);
        self
    }

    pub fn start_log(&self) -> StartLog {
        self.log.clone()
    }

    pub fn into_capturer(self) -> Capturer {
        Capturer::with_backend(Box::new(FakeBackend {
            lister: self.targets.map(FakeLister),
            picker: self.pick.map(FakePicker),
            audio_apps: self.audio_apps.map(FakeAudioApps),
            microphones: self.microphones,
            scripts: Mutex::new(self.scripts),
            start_error: self.start_error,
            log: self.log,
        }))
    }
}

struct FakeLister(Targets);

impl ListTargets for FakeLister {
    fn targets(&self) -> Result<Targets, CaptureError> {
        Ok(self.0.clone())
    }
}

struct FakePicker(Result<Option<PickedTarget>, CaptureError>);

impl PickTarget for FakePicker {
    fn pick(&self, _options: PickerOptions, done: PickDone) {
        done(self.0.clone());
    }
}

struct FakeAudioApps(Vec<AudioApp>);

impl ListAudioApps for FakeAudioApps {
    fn playing(&self) -> Result<Vec<AudioApp>, CaptureError> {
        Ok(self.0.clone())
    }
}

struct FakeBackend {
    lister: Option<FakeLister>,
    picker: Option<FakePicker>,
    audio_apps: Option<FakeAudioApps>,
    microphones: Vec<MicrophoneDevice>,
    scripts: Mutex<VecDeque<Script>>,
    start_error: Option<CaptureError>,
    log: StartLog,
}

impl Backend for FakeBackend {
    fn target_lister(&self) -> Option<&dyn ListTargets> {
        self.lister
            .as_ref()
            .map(|lister| lister as &dyn ListTargets)
    }

    fn target_picker(&self) -> Option<&dyn PickTarget> {
        self.picker.as_ref().map(|picker| picker as &dyn PickTarget)
    }

    fn audio_app_lister(&self) -> Option<&dyn ListAudioApps> {
        self.audio_apps
            .as_ref()
            .map(|lister| lister as &dyn ListAudioApps)
    }

    fn microphones(&self) -> Result<Vec<MicrophoneDevice>, CaptureError> {
        Ok(self.microphones.clone())
    }

    fn start(
        &self,
        target: &Target,
        options: &SessionOptions,
        sink: EventSink,
    ) -> Result<Box<dyn RunningCapture>, CaptureError> {
        self.log
            .starts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Start {
                target: target.clone(),
                options: options.clone(),
            });
        if let Some(error) = &self.start_error {
            return Err(error.clone());
        }
        let script = self
            .scripts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| Box::new(std::iter::empty()));
        std::thread::Builder::new()
            .name("ngine-capture-fake".to_owned())
            .spawn(move || play(script, &sink))
            .map_err(|error| CaptureError::Os(error.to_string()))?;
        Ok(Box::new(FakeRun))
    }
}

fn play(script: Script, sink: &EventSink) {
    for event in script {
        let ends = matches!(event, Event::Ended(_));
        if !sink.deliver_blocking(event) || ends {
            return;
        }
    }
    sink.deliver_blocking(Event::Ended(EndReason::Stopped));
}

struct FakeRun;

impl RunningCapture for FakeRun {
    fn stop(&mut self) {}

    fn now(&self) -> Option<Timestamp> {
        None
    }
}

fn nanos_after(first: Timestamp, index: u64, per_second: u64) -> Timestamp {
    let elapsed = index.saturating_mul(NANOS_PER_SECOND) / per_second.max(1);
    Timestamp::from_nanos(first.as_nanos().saturating_add(elapsed))
}

/// `count` frames of one colour at a steady `frame_rate`, the first at
/// `first`. The frames share one allocation.
pub fn solid_frames(
    pixels: CpuFrame,
    frame_rate: u32,
    count: u64,
    first: Timestamp,
) -> impl Iterator<Item = Event> + Send {
    let pixels = Arc::new(pixels);
    (0..count).map(move |index| {
        let timestamp = nanos_after(first, index, u64::from(frame_rate));
        Event::Video(VideoFrame::from_cpu(timestamp, Arc::clone(&pixels)))
    })
}

/// A solid black frame of `size`, or `None` when it would not fit in memory.
pub fn black(size: Size) -> Option<CpuFrame> {
    CpuFrame::solid_bgra(size, [0, 0, 0, 255])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioShape {
    pub source: AudioSource,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames_per_buffer: u32,
}

/// `count` buffers that follow one another without a gap, the first at
/// `first`. `sample` gives the value of each channel of one frame from the
/// index of that frame since the start. Yields nothing when `shape` has no
/// channels or no sample rate.
pub fn audio_buffers(
    shape: AudioShape,
    count: u64,
    first: Timestamp,
    mut sample: impl FnMut(u64) -> f32 + Send,
) -> impl Iterator<Item = Event> + Send {
    let frames_per_buffer = u64::from(shape.frames_per_buffer);
    (0..count).filter_map(move |index| {
        let first_frame = index.saturating_mul(frames_per_buffer);
        let mut samples =
            Vec::with_capacity(usize::from(shape.channels) * shape.frames_per_buffer as usize);
        for frame in first_frame..first_frame.saturating_add(frames_per_buffer) {
            let value = sample(frame);
            samples.extend(std::iter::repeat_n(value, usize::from(shape.channels)));
        }
        AudioBuffer::new(
            shape.source,
            nanos_after(first, first_frame, u64::from(shape.sample_rate)),
            shape.sample_rate,
            shape.channels,
            samples,
        )
        .map(Event::Audio)
    })
}

/// Merges two scripts so that timestamps never go backwards between them. An
/// event without a timestamp keeps its place in its own script.
pub fn in_time_order(
    one: impl Iterator<Item = Event> + Send,
    other: impl Iterator<Item = Event> + Send,
) -> impl Iterator<Item = Event> + Send {
    let mut one = one.peekable();
    let mut other = other.peekable();
    std::iter::from_fn(move || {
        let one_is_next = match (one.peek(), other.peek()) {
            (Some(ours), Some(theirs)) => ours.timestamp() <= theirs.timestamp(),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if one_is_next {
            one.next()
        } else {
            other.next()
        }
    })
}
