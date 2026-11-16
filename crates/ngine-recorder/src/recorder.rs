use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ngine_capture::{
    Capturer, EndReason, Event, Microphone, PixelFormat, RecvError, Session, SessionOptions,
    SystemAudio, Target,
};

use crate::error::RecorderError;
use crate::event::{Events, RecorderEvent};
use crate::pipeline::{
    Pipeline, RecorderConfig, RecordingStart, ReplaySaveRequest, ReplaySetup, Status,
};
use crate::settings::AudioSettings;

const QUEUED_COMMANDS: usize = 64;
const LONGEST_WAIT_FOR_AN_EVENT: Duration = Duration::from_millis(8);
const RESTART_DELAYS: [Duration; 5] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];
const STOP_CHECK: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub struct CaptureSetup {
    pub target: Target,
    pub show_cursor: bool,
    /// Record only the sound of the app that [`Target::audio_app`] names,
    /// and everything the computer plays when it names none.
    pub app_sound_only: bool,
}

enum Command {
    StartRecording(RecordingStart),
    StopRecording,
    AddMarker,
    SetReplay(Option<ReplaySetup>),
    SaveReplay(ReplaySaveRequest),
    ChangeCapture(CaptureSetup, AudioSettings),
    Stop,
}

/// A capture session and its [`Pipeline`] on a thread of their own.
///
/// Commands return at once and their outcome arrives as a
/// [`RecorderEvent`]. Dropping the recorder stops it and completes the
/// recording file.
pub struct Recorder {
    commands: SyncSender<Command>,
    events: Receiver<RecorderEvent>,
    status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}

fn session_options(
    setup: &CaptureSetup,
    audio: &AudioSettings,
    frame_rate: u32,
    gpu_surfaces: bool,
) -> SessionOptions {
    let system_audio = match (audio.system_gain, setup.app_sound_only) {
        (None, _) => SystemAudio::Off,
        (Some(_), true) => SystemAudio::of(&setup.target),
        (Some(_), false) => SystemAudio::Everything,
    };
    let microphone = match audio.microphone_gain {
        Some(_) => Microphone::SystemDefault,
        None => Microphone::Off,
    };
    SessionOptions::new()
        .with_cursor(setup.show_cursor)
        .with_frame_rate(frame_rate)
        .with_pixel_format(PixelFormat::Nv12)
        .with_system_audio(system_audio)
        .with_microphone(microphone)
        .with_gpu_surfaces(gpu_surfaces)
}

impl Recorder {
    /// Starts capturing `setup.target` on a new thread. A capture that
    /// cannot start is reported as [`RecorderEvent::CaptureLost`].
    pub fn start(
        capturer: Arc<Capturer>,
        setup: CaptureSetup,
        config: &RecorderConfig,
    ) -> Result<Self, RecorderError> {
        let (event_sender, events) = mpsc::channel();
        let pipeline = Pipeline::new(config, event_sender.clone())?;
        let (commands, inbox) = mpsc::sync_channel(QUEUED_COMMANDS);
        let status = Arc::new(Mutex::new(Status::default()));
        let worker = Worker {
            options: session_options(
                &setup,
                &config.audio,
                config.video.frame_rate,
                pipeline.reads_dma_bufs(),
            ),
            frame_rate: config.video.frame_rate,
            replace_the_session: false,
            target: setup.target,
            capturer,
            pipeline,
            inbox,
            events: Events::new(event_sender),
            status: Arc::clone(&status),
            stopping: false,
        };
        let thread = thread::Builder::new()
            .name("ngine-recorder".to_owned())
            .spawn(move || worker.run())
            .map_err(|_| RecorderError::Busy)?;
        Ok(Self {
            commands,
            events,
            status,
            thread: Some(thread),
        })
    }

    fn send(&self, command: Command) -> Result<(), RecorderError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => RecorderError::Busy,
                TrySendError::Disconnected(_) => RecorderError::Stopped,
            })
    }

    pub fn start_recording(&self, start: RecordingStart) -> Result<(), RecorderError> {
        self.send(Command::StartRecording(start))
    }

    pub fn stop_recording(&self) -> Result<(), RecorderError> {
        self.send(Command::StopRecording)
    }

    pub fn add_marker(&self) -> Result<(), RecorderError> {
        self.send(Command::AddMarker)
    }

    pub fn set_replay(&self, setup: Option<ReplaySetup>) -> Result<(), RecorderError> {
        self.send(Command::SetReplay(setup))
    }

    pub fn save_replay(&self, request: ReplaySaveRequest) -> Result<(), RecorderError> {
        self.send(Command::SaveReplay(request))
    }

    /// Captures something else, or with other volumes and sources, without
    /// a new recorder: the replay buffer keeps what it holds and a running
    /// recording goes on. `audio` must keep the number of tracks and their
    /// codec. A change that does not is ignored, so compare the settings
    /// first and start a new recorder for those.
    pub fn change_capture(
        &self,
        setup: CaptureSetup,
        audio: AudioSettings,
    ) -> Result<(), RecorderError> {
        self.send(Command::ChangeCapture(setup, audio))
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn try_event(&self) -> Option<RecorderEvent> {
        self.events.try_recv().ok()
    }

    /// Waits for the next event. `None` when none arrived in time or the
    /// recorder has ended and every event was taken.
    pub fn wait_event(&self, timeout: Duration) -> Option<RecorderEvent> {
        self.events.recv_timeout(timeout).ok()
    }

    /// Asks the recorder to stop. It completes its files and ends with
    /// [`RecorderEvent::Stopped`].
    pub fn request_stop(&self) {
        let _already_stopped = self.send(Command::Stop);
    }

    /// Stops the recorder, waits until its files are complete and returns
    /// the events that were still waiting, ending with
    /// [`RecorderEvent::Stopped`].
    pub fn stop(mut self) -> Vec<RecorderEvent> {
        self.request_stop();
        if let Some(thread) = self.thread.take() {
            let _worker_panicked = thread.join();
        }
        std::iter::from_fn(|| self.try_event()).collect()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(thread) = self.thread.take() {
            let _worker_panicked = thread.join();
        }
    }
}

struct Worker {
    capturer: Arc<Capturer>,
    target: Target,
    options: SessionOptions,
    frame_rate: u32,
    replace_the_session: bool,
    pipeline: Pipeline,
    inbox: Receiver<Command>,
    events: Events,
    status: Arc<Mutex<Status>>,
    stopping: bool,
}

enum SessionEnd {
    Stop,
    Replace,
    Broke(Option<ngine_capture::CaptureError>),
    Failed(RecorderError),
}

impl Worker {
    fn report<T>(
        &self,
        outcome: Result<T, RecorderError>,
        failed: fn(RecorderError) -> RecorderEvent,
    ) {
        if let Err(error) = outcome {
            self.events.send(failed(error));
        }
    }

    fn obey(&mut self, command: Command) {
        match command {
            Command::StartRecording(start) => {
                let file = start.file.clone();
                if let Err(error) = self.pipeline.start_recording(&start) {
                    self.events.send(RecorderEvent::RecordingFailed {
                        file,
                        length: Duration::ZERO,
                        error,
                    });
                }
            }
            Command::StopRecording => {
                let _was_not_recording = self.pipeline.stop_recording();
            }
            Command::AddMarker => {
                let _no_recording_to_mark = self.pipeline.add_marker();
            }
            Command::SetReplay(setup) => {
                let outcome = self.pipeline.set_replay(setup);
                self.report(outcome, |error| RecorderEvent::ReplayStorageFailing {
                    error,
                });
            }
            Command::SaveReplay(request) => {
                let outcome = self.pipeline.save_replay(request);
                self.report(outcome, |error| RecorderEvent::ReplaySaveFailed { error });
            }
            Command::ChangeCapture(setup, audio) => {
                if self.pipeline.set_audio(audio).is_ok() {
                    self.options =
                        session_options(&setup, &audio, self.frame_rate, self.options.gpu_surfaces);
                    self.target = setup.target;
                    self.replace_the_session = true;
                }
            }
            Command::Stop => self.stopping = true,
        }
    }

    fn obey_waiting_commands(&mut self) {
        loop {
            match self.inbox.try_recv() {
                Ok(command) => self.obey(command),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.stopping = true;
                    return;
                }
            }
        }
    }

    fn publish_status(&self) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = self.pipeline.status();
    }

    fn run_session(&mut self, session: &Session) -> SessionEnd {
        loop {
            self.obey_waiting_commands();
            if self.stopping {
                session.stop();
            }
            if self.pipeline.wants_cpu_frames() && self.options.gpu_surfaces {
                self.options.gpu_surfaces = false;
                self.replace_the_session = true;
            }
            if self.replace_the_session && !self.stopping {
                self.replace_the_session = false;
                return SessionEnd::Replace;
            }
            let handled = match session.recv_timeout(LONGEST_WAIT_FOR_AN_EVENT) {
                Ok(Event::Ended(EndReason::Failed(error))) => {
                    return SessionEnd::Broke(Some(error));
                }
                Ok(Event::Ended(EndReason::StoppedByUser)) => return SessionEnd::Broke(None),
                Ok(Event::Ended(_)) | Err(RecvError::Finished) => return SessionEnd::Stop,
                Ok(event) => self.pipeline.handle(&event),
                Err(RecvError::TimedOut) => Ok(()),
            };
            let ticked = handled.and_then(|()| match session.now() {
                Some(now) => self.pipeline.tick(now),
                None => Ok(()),
            });
            if let Err(error) = ticked {
                return SessionEnd::Failed(error);
            }
            self.publish_status();
        }
    }

    fn wait_before_restart(&mut self, delay: Duration) {
        let until = Instant::now() + delay;
        while !self.stopping && Instant::now() < until {
            thread::sleep(STOP_CHECK.min(delay));
            self.obey_waiting_commands();
        }
    }

    fn restart(&mut self) -> Option<Session> {
        for delay in RESTART_DELAYS {
            self.wait_before_restart(delay);
            if self.stopping {
                return None;
            }
            if let Ok(session) = self.capturer.start(&self.target, &self.options) {
                self.pipeline.capture_restarted();
                self.events.send(RecorderEvent::CaptureResumed);
                return Some(session);
            }
        }
        None
    }

    fn run(mut self) {
        let mut session = match self.capturer.start(&self.target, &self.options) {
            Ok(session) => Some(session),
            Err(error) => {
                self.events.send(RecorderEvent::CaptureLost(Some(error)));
                None
            }
        };
        while let Some(running) = session.take() {
            match self.run_session(&running) {
                SessionEnd::Stop => {}
                SessionEnd::Replace => {
                    drop(running);
                    session = match self.capturer.start(&self.target, &self.options) {
                        Ok(replacement) => {
                            self.pipeline.capture_restarted();
                            Some(replacement)
                        }
                        Err(error) => {
                            self.events
                                .send(RecorderEvent::CaptureInterrupted(error.clone()));
                            let restarted = self.restart();
                            if restarted.is_none() && !self.stopping {
                                self.events.send(RecorderEvent::CaptureLost(Some(error)));
                            }
                            restarted
                        }
                    };
                }
                SessionEnd::Failed(error) => {
                    running.stop();
                    self.pipeline.finish();
                    self.events.send(RecorderEvent::Failed(error));
                }
                SessionEnd::Broke(Some(error)) => {
                    drop(running);
                    self.events
                        .send(RecorderEvent::CaptureInterrupted(error.clone()));
                    session = self.restart();
                    if session.is_none() && !self.stopping {
                        self.events.send(RecorderEvent::CaptureLost(Some(error)));
                    }
                }
                SessionEnd::Broke(None) => self.events.send(RecorderEvent::CaptureLost(None)),
            }
        }
        self.pipeline.finish();
        self.publish_status();
        self.events.send(RecorderEvent::Stopped);
    }
}
