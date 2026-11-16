use std::any::Any;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};

use super::portal::{self, Live, PortalSession};
use super::pw::{self, PipeWireCapture, Reporter, Request, VideoSource};
use super::{monotonic, portal_plan};
use crate::backend::{Backend, ListAudioApps, ListTargets, PickDone, PickTarget};
use crate::error::{CaptureError, Permission};
use crate::options::SessionOptions;
use crate::session::{EventSink, RunningCapture};
use crate::supervisor::{self, Launch, Signal, Supervised, Supervisee};
use crate::target::TargetKind;
use crate::target::{
    AudioApp, MicrophoneDevice, ParentWindow, PickedHandle, PickedTarget, PickerOptions,
    RestoreToken, Target, TargetSource,
};
use crate::time::Timestamp;

pub(super) struct WaylandBackend;

struct Granted {
    live: Option<Live>,
    restore_token: Option<String>,
    parent: Option<ParentWindow>,
}

struct Grant {
    granted: Mutex<Granted>,
}

impl Grant {
    fn stream_showing_the_cursor(&self, show_cursor: bool) -> Result<Live, CaptureError> {
        let mut granted = self.granted.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(live) = granted
            .live
            .take_if(|live| live.shows_cursor == show_cursor)
        {
            return Ok(live);
        }
        if let Some(wrong_cursor) = granted.live.take() {
            portal::close(&wrong_cursor.session);
        }
        let opened = portal::open_unless_cancelled(
            show_cursor,
            granted.restore_token.as_deref(),
            granted.parent.as_ref(),
        )?
        .ok_or(CaptureError::PermissionDenied(Permission::ScreenRecording))?;
        if opened.restore_token.is_some() {
            granted.restore_token = opened.restore_token;
        }
        Ok(opened.live)
    }
}

fn picked(options: PickerOptions) -> Result<Option<PickedTarget>, CaptureError> {
    let asked_token = options.restore_token.as_ref().map(RestoreToken::as_str);
    let Some(opened) = portal::open_unless_cancelled(true, asked_token, options.parent.as_ref())?
    else {
        return Ok(None);
    };
    let kind: TargetKind = portal_plan::kind_of(opened.live.source_type);
    let restore_token = opened.restore_token;
    let grant = Grant {
        granted: Mutex::new(Granted {
            live: Some(opened.live),
            restore_token: restore_token.clone(),
            parent: options.parent,
        }),
    };
    let target = Target::picked(PickedHandle {
        kind,
        label: portal_plan::label_of(kind).to_owned(),
        audio_app: None,
        native: Arc::new(grant),
    });
    Ok(Some(PickedTarget::new(
        target,
        restore_token.map(RestoreToken::new),
    )))
}

impl PickTarget for WaylandBackend {
    fn pick(&self, options: PickerOptions, done: PickDone) {
        let done = Arc::new(Mutex::new(Some(done)));
        let finish = {
            let done = Arc::clone(&done);
            move |outcome| {
                let callback = done.lock().unwrap_or_else(PoisonError::into_inner).take();
                if let Some(callback) = callback {
                    callback(outcome);
                }
            }
        };
        let from_the_thread = finish.clone();
        let spawned = std::thread::Builder::new()
            .name("ngine-capture-portal".to_owned())
            .spawn(move || from_the_thread(picked(options)));
        if let Err(error) = spawned {
            finish(Err(CaptureError::Os(format!(
                "could not start the picker thread: {error}"
            ))));
        }
    }
}

impl ListAudioApps for WaylandBackend {
    fn playing(&self) -> Result<Vec<AudioApp>, CaptureError> {
        pw::playing_apps()
    }
}

struct WaylandRunning {
    pipewire: PipeWireCapture,
    session: PortalSession,
}

impl Supervisee for WaylandRunning {
    fn stop(mut self) {
        self.pipewire.stop();
        portal::close(&self.session);
    }
}

struct WaylandLaunch {
    grant: Arc<dyn Any + Send + Sync>,
    options: SessionOptions,
}

impl Launch for WaylandLaunch {
    type Capture = WaylandRunning;

    fn launch(
        &self,
        generation: u64,
        sink: &EventSink,
        signals: &Sender<Signal>,
    ) -> Result<WaylandRunning, CaptureError> {
        let grant = self.grant.downcast_ref::<Grant>().ok_or_else(only_picked)?;
        let system = pw::system_sound(&self.options.system_audio)?;
        let live = grant.stream_showing_the_cursor(self.options.show_cursor)?;
        let request = Request {
            video: Some(VideoSource {
                remote: live.remote,
                node_id: live.node_id,
            }),
            system,
            microphone: pw::microphone_choice(&self.options.microphone),
            frame_rate: self.options.frame_rate,
            pixel_format: self.options.pixel_format,
            gpu_surfaces: self.options.gpu_surfaces,
            reporter: Some(Reporter {
                signals: signals.clone(),
                generation,
            }),
        };
        match pw::start(request, sink.clone()) {
            Ok(pipewire) => Ok(WaylandRunning {
                pipewire,
                session: live.session,
            }),
            Err(error) => {
                portal::close(&live.session);
                Err(error)
            }
        }
    }
}

struct WaylandCapture {
    supervised: Supervised<WaylandRunning>,
}

impl RunningCapture for WaylandCapture {
    fn stop(&mut self) {
        self.supervised.stop();
    }

    fn now(&self) -> Option<Timestamp> {
        monotonic::now()
    }
}

fn only_picked() -> CaptureError {
    CaptureError::Unsupported(
        "capturing a listed target on Wayland: an app captures only what the user chose in the picker"
            .to_owned(),
    )
}

impl Backend for WaylandBackend {
    fn name(&self) -> &'static str {
        "PipeWire through the desktop portal (Wayland)"
    }

    fn target_lister(&self) -> Option<&dyn ListTargets> {
        None
    }

    fn target_picker(&self) -> Option<&dyn PickTarget> {
        Some(self)
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
        let TargetSource::Picked(handle) = target.source() else {
            return Err(only_picked());
        };
        let launch = WaylandLaunch {
            grant: Arc::clone(&handle.native),
            options: options.clone(),
        };
        let supervised = supervisor::supervise(launch, sink, &super::RESTART_DELAYS)?;
        Ok(Box::new(WaylandCapture { supervised }))
    }
}
