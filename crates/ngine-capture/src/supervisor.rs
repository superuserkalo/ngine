use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::CaptureError;
use crate::event::{EndReason, Event};
#[cfg(target_os = "macos")]
use crate::frame::Size;
use crate::session::EventSink;

pub(crate) enum Signal {
    Shutdown,
    Stopped {
        generation: u64,
        error: CaptureError,
        by_user: bool,
    },
    #[cfg(target_os = "macos")]
    NativeSize {
        generation: u64,
        size: Size,
    },
}

pub(crate) fn send_unless_the_supervisor_is_gone(signals: &Sender<Signal>, signal: Signal) {
    drop(signals.send(signal));
}

pub(crate) trait Supervisee: Send + 'static {
    fn stop(self);
    #[cfg(target_os = "macos")]
    fn follow(&mut self, size: Size);
}

pub(crate) trait Launch: Send + 'static {
    type Capture: Supervisee;

    fn launch(
        &self,
        generation: u64,
        sink: &EventSink,
        signals: &Sender<Signal>,
    ) -> Result<Self::Capture, CaptureError>;
}

struct Shared<C> {
    capture: Mutex<Option<C>>,
    stopping: AtomicBool,
}

impl<C: Supervisee> Shared<C> {
    fn take(&self) -> Option<C> {
        self.capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn adopt(&self, capture: C) -> bool {
        let mut current = self.capture.lock().unwrap_or_else(PoisonError::into_inner);
        if self.stopping.load(Ordering::Acquire) {
            capture.stop();
            false
        } else {
            *current = Some(capture);
            true
        }
    }

    #[cfg(target_os = "macos")]
    fn follow(&self, size: Size) {
        let mut current = self.capture.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(capture) = current.as_mut() {
            capture.follow(size);
        }
    }
}

enum Restart {
    Running,
    Failed(CaptureError),
    ShutDown,
}

struct Supervisor<L: Launch> {
    launcher: L,
    sink: EventSink,
    signals: Sender<Signal>,
    inbox: Receiver<Signal>,
    shared: Arc<Shared<L::Capture>>,
    generation: u64,
    restart_delays: &'static [Duration],
}

impl<L: Launch> Supervisor<L> {
    fn run(mut self) {
        while let Ok(signal) = self.inbox.recv() {
            match signal {
                Signal::Shutdown => return,
                #[cfg(target_os = "macos")]
                Signal::NativeSize { generation, size } if generation == self.generation => {
                    self.shared.follow(size);
                }
                Signal::Stopped {
                    generation,
                    error,
                    by_user,
                } if generation == self.generation => {
                    if let Some(broken) = self.shared.take() {
                        broken.stop();
                    }
                    if by_user {
                        self.sink.deliver(Event::Ended(EndReason::StoppedByUser));
                        return;
                    }
                    self.sink.deliver(Event::Interrupted(error));
                    match self.restart() {
                        Restart::Running => {
                            self.sink.deliver(Event::Resumed);
                        }
                        Restart::Failed(error) => {
                            self.sink.deliver(Event::Ended(EndReason::Failed(error)));
                            return;
                        }
                        Restart::ShutDown => return,
                    }
                }
                #[cfg(target_os = "macos")]
                Signal::NativeSize { .. } => {}
                Signal::Stopped { .. } => {}
            }
        }
    }

    fn restart(&mut self) -> Restart {
        let mut last_error = CaptureError::Os("capture did not restart".to_owned());
        for delay in self.restart_delays {
            if self.shutdown_arrives_within(*delay) {
                return Restart::ShutDown;
            }
            self.generation += 1;
            match self
                .launcher
                .launch(self.generation, &self.sink, &self.signals)
            {
                Ok(capture) => {
                    return if self.shared.adopt(capture) {
                        Restart::Running
                    } else {
                        Restart::ShutDown
                    };
                }
                Err(error @ (CaptureError::TargetGone | CaptureError::PermissionDenied(_))) => {
                    return Restart::Failed(error);
                }
                Err(error) => last_error = error,
            }
        }
        Restart::Failed(last_error)
    }

    fn shutdown_arrives_within(&self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.inbox.recv_timeout(left) {
                Ok(Signal::Shutdown) | Err(RecvTimeoutError::Disconnected) => return true,
                #[cfg(target_os = "macos")]
                Ok(Signal::NativeSize { .. }) => {}
                Ok(Signal::Stopped { .. }) => {}
                Err(RecvTimeoutError::Timeout) => return false,
            }
        }
    }
}

pub(crate) struct Supervised<C> {
    shared: Arc<Shared<C>>,
    signals: Sender<Signal>,
    supervisor: Option<JoinHandle<()>>,
}

impl<C: Supervisee> Supervised<C> {
    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn signals(&self) -> Sender<Signal> {
        self.signals.clone()
    }

    pub(crate) fn stop(&mut self) {
        self.shared.stopping.store(true, Ordering::Release);
        send_unless_the_supervisor_is_gone(&self.signals, Signal::Shutdown);
        if let Some(capture) = self.shared.take() {
            capture.stop();
        }
        if let Some(supervisor) = self.supervisor.take() {
            drop(supervisor.join());
        }
    }
}

pub(crate) fn supervise<L: Launch>(
    launcher: L,
    sink: EventSink,
    restart_delays: &'static [Duration],
) -> Result<Supervised<L::Capture>, CaptureError> {
    let (signals, inbox) = mpsc::channel();
    let capture = launcher.launch(0, &sink, &signals)?;
    let shared = Arc::new(Shared {
        capture: Mutex::new(Some(capture)),
        stopping: AtomicBool::new(false),
    });
    let supervisor = Supervisor {
        launcher,
        sink,
        signals: signals.clone(),
        inbox,
        shared: Arc::clone(&shared),
        generation: 0,
        restart_delays,
    };
    let thread = std::thread::Builder::new()
        .name("ngine-capture-supervisor".to_owned())
        .spawn(move || supervisor.run());
    match thread {
        Ok(supervisor) => Ok(Supervised {
            shared,
            signals,
            supervisor: Some(supervisor),
        }),
        Err(error) => {
            if let Some(capture) = shared.take() {
                capture.stop();
            }
            Err(CaptureError::Os(error.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::error::Permission;
    use crate::session::{self, RunningCapture, Session};
    use crate::time::Timestamp;

    static NO_WAIT: [Duration; 3] = [Duration::ZERO; 3];

    #[derive(Clone, Default)]
    struct Record {
        launched: Arc<Mutex<Vec<u64>>>,
        stopped: Arc<Mutex<Vec<u64>>>,
        #[cfg(target_os = "macos")]
        followed: Arc<Mutex<Vec<Size>>>,
        signals: Arc<Mutex<Option<Sender<Signal>>>>,
    }

    impl Record {
        fn send(&self, signal: Signal) {
            let signals = self.signals.lock().unwrap().clone().unwrap();
            signals.send(signal).unwrap();
        }

        #[cfg(target_os = "macos")]
        fn follows_a_size_once_the_stale_signals_are_behind(&self) {
            self.send(Signal::NativeSize {
                generation: 0,
                size: Size::new(1, 1),
            });
            self.send(Signal::NativeSize {
                generation: 1,
                size: Size::new(800, 600),
            });
            let waited_since = Instant::now();
            while self.followed.lock().unwrap().is_empty()
                && waited_since.elapsed() < Duration::from_secs(5)
            {
                std::thread::yield_now();
            }
        }

        fn breaks(&self, generation: u64) {
            self.send(Signal::Stopped {
                generation,
                error: CaptureError::Os("the stream broke".to_owned()),
                by_user: false,
            });
        }
    }

    struct ScriptedCapture {
        generation: u64,
        record: Record,
    }

    impl Supervisee for ScriptedCapture {
        fn stop(self) {
            self.record.stopped.lock().unwrap().push(self.generation);
        }

        #[cfg(target_os = "macos")]
        fn follow(&mut self, size: Size) {
            self.record.followed.lock().unwrap().push(size);
        }
    }

    struct ScriptedLauncher {
        relaunches: Mutex<VecDeque<Result<(), CaptureError>>>,
        record: Record,
    }

    impl Launch for ScriptedLauncher {
        type Capture = ScriptedCapture;

        fn launch(
            &self,
            generation: u64,
            _sink: &EventSink,
            signals: &Sender<Signal>,
        ) -> Result<ScriptedCapture, CaptureError> {
            *self.record.signals.lock().unwrap() = Some(signals.clone());
            let outcome = if generation == 0 {
                Ok(())
            } else {
                let next = self.relaunches.lock().unwrap().pop_front();
                next.unwrap_or_else(|| Err(CaptureError::Os("still broken".to_owned())))
            };
            outcome.map(|()| {
                self.record.launched.lock().unwrap().push(generation);
                ScriptedCapture {
                    generation,
                    record: self.record.clone(),
                }
            })
        }
    }

    struct Running(Supervised<ScriptedCapture>);

    impl RunningCapture for Running {
        fn stop(&mut self) {
            self.0.stop();
        }

        fn now(&self) -> Option<Timestamp> {
            None
        }
    }

    fn supervised(relaunches: Vec<Result<(), CaptureError>>) -> (Session, Record) {
        let record = Record::default();
        let launcher = ScriptedLauncher {
            relaunches: Mutex::new(relaunches.into()),
            record: record.clone(),
        };
        let (sink, pending) = session::channel();
        let running = supervise(launcher, sink, &NO_WAIT).unwrap();
        (pending.run(Box::new(Running(running))), record)
    }

    fn next(session: &Session) -> Event {
        session.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn a_broken_capture_is_reported_restarted_and_reported_again() {
        let (session, record) = supervised(vec![Ok(())]);

        record.breaks(0);

        assert!(matches!(next(&session), Event::Interrupted(_)));
        assert!(matches!(next(&session), Event::Resumed));
        assert_eq!(*record.launched.lock().unwrap(), [0, 1]);
        assert_eq!(*record.stopped.lock().unwrap(), [0]);
    }

    #[test]
    fn a_restart_is_tried_again_after_a_failure_that_may_pass() {
        let still_broken = Err(CaptureError::Os("not yet".to_owned()));
        let (session, record) = supervised(vec![still_broken, Ok(())]);

        record.breaks(0);

        assert!(matches!(next(&session), Event::Interrupted(_)));
        assert!(matches!(next(&session), Event::Resumed));
        assert_eq!(*record.launched.lock().unwrap(), [0, 2]);
    }

    #[test]
    fn a_capture_that_never_restarts_ends_the_session_with_the_last_failure() {
        let (session, record) = supervised(Vec::new());

        record.breaks(0);

        assert!(matches!(next(&session), Event::Interrupted(_)));
        assert!(matches!(
            next(&session),
            Event::Ended(EndReason::Failed(CaptureError::Os(text))) if text == "still broken"
        ));
        assert!(session.recv().is_none());
    }

    #[test]
    fn a_target_that_is_gone_or_a_lost_permission_ends_the_session_at_once() {
        for lasting in [
            CaptureError::TargetGone,
            CaptureError::PermissionDenied(Permission::ScreenRecording),
        ] {
            let (session, record) = supervised(vec![Err(lasting.clone()), Ok(())]);

            record.breaks(0);

            assert!(matches!(next(&session), Event::Interrupted(_)));
            assert!(matches!(
                next(&session),
                Event::Ended(EndReason::Failed(error)) if error == lasting
            ));
            assert_eq!(*record.launched.lock().unwrap(), [0]);
        }
    }

    #[test]
    fn a_stop_by_the_user_ends_the_session_without_a_restart() {
        let (session, record) = supervised(vec![Ok(())]);

        record.send(Signal::Stopped {
            generation: 0,
            error: CaptureError::Os("stopped from the menu bar".to_owned()),
            by_user: true,
        });

        assert!(matches!(
            next(&session),
            Event::Ended(EndReason::StoppedByUser)
        ));
        assert_eq!(*record.launched.lock().unwrap(), [0]);
    }

    #[test]
    fn signals_of_a_replaced_capture_are_ignored() {
        let (session, record) = supervised(vec![Ok(()), Ok(())]);

        record.breaks(0);
        assert!(matches!(next(&session), Event::Interrupted(_)));
        assert!(matches!(next(&session), Event::Resumed));
        record.breaks(0);
        #[cfg(target_os = "macos")]
        record.follows_a_size_once_the_stale_signals_are_behind();
        session.stop();

        assert!(matches!(next(&session), Event::Ended(EndReason::Stopped)));
        assert_eq!(*record.launched.lock().unwrap(), [0, 1]);
        #[cfg(target_os = "macos")]
        assert_eq!(*record.followed.lock().unwrap(), [Size::new(800, 600)]);
    }

    #[test]
    fn stopping_stops_the_capture_that_is_running() {
        let (session, record) = supervised(Vec::new());

        session.stop();

        assert_eq!(*record.stopped.lock().unwrap(), [0]);
        assert!(matches!(next(&session), Event::Ended(EndReason::Stopped)));
    }
}
