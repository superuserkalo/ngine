use std::cell::Cell;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
use std::sync::mpsc::TrySendError;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::error::CaptureError;
use crate::event::{EndReason, Event};
use crate::target::Region;
use crate::time::Timestamp;

const QUEUED_EVENTS: usize = 512;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
const QUEUED_VIDEO_FRAMES: usize = 4;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
const FULL_QUEUE_RETRY: Duration = Duration::from_millis(2);
const STOP_POLL: Duration = Duration::from_millis(50);

pub(crate) trait RunningCapture: Send {
    fn stop(&mut self);
    fn now(&self) -> Option<Timestamp>;
}

#[derive(Default)]
struct Counters {
    queued_video: AtomicUsize,
    dropped_video: AtomicU64,
    dropped_audio: AtomicU64,
    closed: AtomicBool,
}

#[derive(Clone)]
pub(crate) struct EventSink {
    sender: SyncSender<Event>,
    counters: Arc<Counters>,
}

impl EventSink {
    fn is_closed(&self) -> bool {
        self.counters.closed.load(Ordering::Acquire)
    }

    fn send_with<E>(
        &self,
        event: Event,
        send: impl FnOnce(&SyncSender<Event>, Event) -> Result<(), E>,
    ) -> Result<(), E> {
        let is_video = matches!(event, Event::Video(_));
        let ends = matches!(event, Event::Ended(_));
        if is_video {
            self.counters.queued_video.fetch_add(1, Ordering::AcqRel);
        }
        let outcome = send(&self.sender, event);
        match &outcome {
            Ok(()) if ends => self.counters.closed.store(true, Ordering::Release),
            Err(_) if is_video => {
                self.counters.queued_video.fetch_sub(1, Ordering::AcqRel);
            }
            Ok(()) | Err(_) => {}
        }
        outcome
    }

    pub(crate) fn deliver_blocking(&self, event: Event) -> bool {
        !self.is_closed() && self.send_with(event, SyncSender::send).is_ok()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
impl EventSink {
    fn try_send(&self, event: Event) -> Result<(), TrySendError<Event>> {
        self.send_with(event, SyncSender::try_send)
    }

    pub(crate) fn video(&self, frame: crate::frame::VideoFrame) {
        if self.is_closed() {
            return;
        }
        let consumer_is_behind =
            self.counters.queued_video.load(Ordering::Acquire) >= QUEUED_VIDEO_FRAMES;
        if consumer_is_behind || self.try_send(Event::Video(frame)).is_err() {
            self.counters.dropped_video.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn audio(&self, buffer: crate::audio::AudioBuffer) {
        if !self.is_closed() && self.try_send(Event::Audio(buffer)).is_err() {
            self.counters.dropped_audio.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn deliver(&self, event: Event) -> bool {
        let mut waiting = event;
        loop {
            if self.is_closed() {
                return false;
            }
            match self.try_send(waiting) {
                Ok(()) => return true,
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(returned)) => {
                    waiting = returned;
                    std::thread::sleep(FULL_QUEUE_RETRY);
                }
            }
        }
    }
}

pub(crate) struct PendingSession {
    events: Receiver<Event>,
    counters: Arc<Counters>,
}

pub(crate) fn channel() -> (EventSink, PendingSession) {
    let (sender, events) = mpsc::sync_channel(QUEUED_EVENTS);
    let counters = Arc::new(Counters::default());
    (
        EventSink {
            sender,
            counters: Arc::clone(&counters),
        },
        PendingSession { events, counters },
    )
}

impl PendingSession {
    pub(crate) fn run(self, capture: Box<dyn RunningCapture>) -> Session {
        Session {
            events: self.events,
            crop: None,
            finished: Cell::new(false),
            control: Arc::new(Control {
                counters: self.counters,
                capture: Mutex::new(Some(capture)),
                stop_requested: AtomicBool::new(false),
            }),
        }
    }
}

struct Control {
    counters: Arc<Counters>,
    capture: Mutex<Option<Box<dyn RunningCapture>>>,
    stop_requested: AtomicBool,
}

impl Control {
    fn stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
        self.counters.closed.store(true, Ordering::Release);
        let capture = self
            .capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut capture) = capture {
            capture.stop();
        }
    }
}

#[derive(Clone)]
pub struct Stopper {
    control: Arc<Control>,
}

impl Stopper {
    pub fn stop(&self) {
        self.control.stop();
    }
}

impl fmt::Debug for Stopper {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Stopper").finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecvError {
    TimedOut,
    /// [`Event::Ended`] was already received.
    Finished,
}

impl fmt::Display for RecvError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TimedOut => "no capture event arrived in time",
            Self::Finished => "the capture session has ended",
        })
    }
}

impl std::error::Error for RecvError {}

/// A running capture. Dropping it stops the capture.
///
/// Receive promptly. When the receiver falls behind, video frames are dropped
/// first and counted in [`Session::dropped_video_frames`].
pub struct Session {
    events: Receiver<Event>,
    crop: Option<Region>,
    finished: Cell<bool>,
    control: Arc<Control>,
}

impl Session {
    pub(crate) fn showing_only(mut self, crop: Option<Region>) -> Self {
        self.crop = crop;
        self
    }

    /// Waits for the next event. Returns `None` once [`Event::Ended`] has been
    /// received.
    pub fn recv(&self) -> Option<Event> {
        self.next_before(None).ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, RecvError> {
        self.next_before(Some(Instant::now() + timeout))
    }

    /// Stops the capture. Events already queued can still be received, and
    /// [`Event::Ended`] with [`EndReason::Stopped`] follows them.
    pub fn stop(&self) {
        self.control.stop();
    }

    pub fn stopper(&self) -> Stopper {
        Stopper {
            control: Arc::clone(&self.control),
        }
    }

    /// The current time on the clock that stamps this session's frames and
    /// buffers. `None` after the session stopped, or when the backend has no
    /// clock to read.
    pub fn now(&self) -> Option<Timestamp> {
        self.control
            .capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|capture| capture.now())
    }

    pub fn dropped_video_frames(&self) -> u64 {
        self.control.counters.dropped_video.load(Ordering::Relaxed)
    }

    pub fn dropped_audio_buffers(&self) -> u64 {
        self.control.counters.dropped_audio.load(Ordering::Relaxed)
    }

    fn next_before(&self, deadline: Option<Instant>) -> Result<Event, RecvError> {
        loop {
            let event = self.next_event_before(deadline)?;
            if let Some(shown) = self.shown(event) {
                return Ok(shown);
            }
        }
    }

    fn shown(&self, event: Event) -> Option<Event> {
        match (event, self.crop) {
            (Event::Video(frame), Some(region)) => frame.cropped(region).map(Event::Video),
            (event, _) => Some(event),
        }
    }

    fn next_event_before(&self, deadline: Option<Instant>) -> Result<Event, RecvError> {
        if self.finished.get() {
            return Err(RecvError::Finished);
        }
        loop {
            if self.control.stop_requested.load(Ordering::Acquire) {
                let queued = self.events.try_recv();
                return Ok(self.account(queued.unwrap_or(Event::Ended(EndReason::Stopped))));
            }
            let wait = deadline.map_or(STOP_POLL, |deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(STOP_POLL)
            });
            match self.events.recv_timeout(wait) {
                Ok(event) => return Ok(self.account(event)),
                Err(RecvTimeoutError::Disconnected) => {
                    return Ok(self.account(Event::Ended(self.reason_the_backend_went_away())));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        return Err(RecvError::TimedOut);
                    }
                }
            }
        }
    }

    fn reason_the_backend_went_away(&self) -> EndReason {
        if self.control.stop_requested.load(Ordering::Acquire) {
            EndReason::Stopped
        } else {
            EndReason::Failed(CaptureError::Os(
                "the capture backend went away without ending the session".to_owned(),
            ))
        }
    }

    fn account(&self, event: Event) -> Event {
        match &event {
            Event::Video(_) => {
                self.control
                    .counters
                    .queued_video
                    .fetch_sub(1, Ordering::AcqRel);
            }
            Event::Ended(_) => {
                self.finished.set(true);
                self.control.counters.closed.store(true, Ordering::Release);
            }
            Event::Audio(_) | Event::Interrupted(_) | Event::Resumed => {}
        }
        event
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.control.stop();
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("finished", &self.finished.get())
            .field("dropped_video_frames", &self.dropped_video_frames())
            .field("dropped_audio_buffers", &self.dropped_audio_buffers())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::audio::{AudioBuffer, AudioSource};
    use crate::frame::{CpuFrame, Size, VideoFrame};

    struct NothingToStop;

    impl RunningCapture for NothingToStop {
        fn stop(&mut self) {}

        fn now(&self) -> Option<Timestamp> {
            Some(Timestamp::from_nanos(42))
        }
    }

    fn frame(nanos: u64) -> VideoFrame {
        let pixels = CpuFrame::solid_bgra(Size::new(2, 2), [0, 0, 0, 255]).unwrap();
        VideoFrame::from_cpu(Timestamp::from_nanos(nanos), Arc::new(pixels))
    }

    fn buffer(nanos: u64) -> AudioBuffer {
        AudioBuffer::new(
            AudioSource::System,
            Timestamp::from_nanos(nanos),
            48_000,
            2,
            vec![0.0; 4],
        )
        .unwrap()
    }

    #[test]
    fn a_reader_that_falls_behind_loses_video_frames_but_no_audio() {
        let (sink, pending) = channel();
        let session = pending.run(Box::new(NothingToStop));
        let sent = u64::try_from(QUEUED_VIDEO_FRAMES).unwrap() + 3;

        for nanos in 0..sent {
            sink.video(frame(nanos));
            sink.audio(buffer(nanos));
        }
        sink.deliver(Event::Ended(EndReason::Stopped));
        let received: Vec<Event> = std::iter::from_fn(|| session.recv()).collect();

        let videos = received
            .iter()
            .filter(|event| matches!(event, Event::Video(_)))
            .count();
        let audios = received
            .iter()
            .filter(|event| matches!(event, Event::Audio(_)))
            .count();
        assert_eq!(videos, QUEUED_VIDEO_FRAMES);
        assert_eq!(u64::try_from(audios).unwrap(), sent);
        assert_eq!(session.dropped_video_frames(), 3);
        assert_eq!(session.dropped_audio_buffers(), 0);
    }

    #[test]
    fn frames_flow_again_once_the_reader_catches_up() {
        let (sink, pending) = channel();
        let session = pending.run(Box::new(NothingToStop));

        for round in 0..3 {
            for nanos in 0..QUEUED_VIDEO_FRAMES {
                sink.video(frame(u64::try_from(round * 10 + nanos).unwrap()));
            }
            for _ in 0..QUEUED_VIDEO_FRAMES {
                assert!(matches!(session.recv(), Some(Event::Video(_))));
            }
        }

        assert_eq!(session.dropped_video_frames(), 0);
    }

    #[test]
    fn nothing_is_accepted_after_the_session_ended() {
        let (sink, pending) = channel();
        let session = pending.run(Box::new(NothingToStop));

        sink.deliver(Event::Ended(EndReason::StoppedByUser));
        sink.video(frame(1));
        sink.audio(buffer(1));

        assert!(!sink.deliver(Event::Resumed));
        assert!(matches!(
            session.recv(),
            Some(Event::Ended(EndReason::StoppedByUser))
        ));
        assert!(session.recv().is_none());
        assert_eq!(
            session.recv_timeout(Duration::ZERO).unwrap_err(),
            RecvError::Finished
        );
    }

    #[test]
    fn a_backend_that_vanishes_ends_the_session_with_a_failure() {
        let (sink, pending) = channel();
        let session = pending.run(Box::new(NothingToStop));

        drop(sink);

        assert!(matches!(
            session.recv(),
            Some(Event::Ended(EndReason::Failed(CaptureError::Os(_))))
        ));
        assert!(session.recv().is_none());
    }

    #[test]
    fn a_session_that_shows_a_region_crops_each_frame_and_skips_those_without_it() {
        let (sink, pending) = channel();
        let session = pending
            .run(Box::new(NothingToStop))
            .showing_only(Some(Region::new(4, 2, 6, 4)));
        let large = CpuFrame::solid_bgra(Size::new(16, 8), [0, 0, 0, 255]).unwrap();
        let small = CpuFrame::solid_bgra(Size::new(2, 2), [0, 0, 0, 255]).unwrap();

        sink.video(VideoFrame::from_cpu(
            Timestamp::from_nanos(1),
            Arc::new(small),
        ));
        sink.video(VideoFrame::from_cpu(
            Timestamp::from_nanos(2),
            Arc::new(large),
        ));
        sink.deliver(Event::Ended(EndReason::Stopped));

        let Some(Event::Video(frame)) = session.recv() else {
            panic!("the frame that holds the region arrives");
        };
        assert_eq!(frame.timestamp(), Timestamp::from_nanos(2));
        assert_eq!(frame.size(), Size::new(6, 4));
        assert_eq!(frame.to_cpu().unwrap().size(), Size::new(6, 4));
        assert!(matches!(session.recv(), Some(Event::Ended(_))));
        assert_eq!(session.dropped_video_frames(), 0);
    }

    #[test]
    fn the_session_clock_is_readable_until_the_session_stops() {
        let (_sink, pending) = channel();
        let session = pending.run(Box::new(NothingToStop));

        assert_eq!(session.now(), Some(Timestamp::from_nanos(42)));
        session.stop();

        assert_eq!(session.now(), None);
    }
}
