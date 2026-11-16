use std::fmt;

use crate::backend::{Backend, ListAudioApps, ListTargets, PickTarget};
use crate::error::CaptureError;
use crate::options::SessionOptions;
use crate::session::{self, Session};
use crate::target::{AudioApp, MicrophoneDevice, PickedTarget, PickerOptions, Target, Targets};

/// What a system cannot do is absent rather than failing. Wayland has no
/// [`Capturer::lister`], X11 has no [`Capturer::picker`], and only Linux has
/// [`Capturer::audio_apps`].
pub struct Capturer {
    backend: Box<dyn Backend>,
}

impl Capturer {
    pub fn system() -> Result<Self, CaptureError> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self::with_backend(Box::new(crate::macos::MacBackend)))
        }
        #[cfg(target_os = "windows")]
        {
            Ok(Self::with_backend(Box::new(crate::windows::WindowsBackend)))
        }
        #[cfg(target_os = "linux")]
        {
            crate::linux::system_backend().map(Self::with_backend)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Err(CaptureError::Unsupported(
                "ngine-capture has no backend for this operating system yet".to_owned(),
            ))
        }
    }

    pub(crate) fn with_backend(backend: Box<dyn Backend>) -> Self {
        Self { backend }
    }

    /// What captures on this system, for a person to read.
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// Lists screens, windows and apps. `None` where the system does not let
    /// an app list them, and the picker is the only way to choose.
    pub fn lister(&self) -> Option<TargetLister<'_>> {
        self.backend
            .target_lister()
            .map(|lister| TargetLister { lister })
    }

    /// The system's own picker. `None` where the system has none.
    pub fn picker(&self) -> Option<TargetPicker<'_>> {
        self.backend
            .target_picker()
            .map(|picker| TargetPicker { picker })
    }

    /// Lists the apps that are playing sound. `Some` only where a target does
    /// not say which app it belongs to, so the user has to pick the app.
    pub fn audio_apps(&self) -> Option<AudioAppLister<'_>> {
        self.backend
            .audio_app_lister()
            .map(|lister| AudioAppLister { lister })
    }

    pub fn microphones(&self) -> Result<Vec<MicrophoneDevice>, CaptureError> {
        self.backend.microphones()
    }

    pub fn start(
        &self,
        target: &Target,
        options: &SessionOptions,
    ) -> Result<Session, CaptureError> {
        let (sink, pending) = session::channel();
        let capture = self.backend.start(target, options, sink)?;
        Ok(pending.run(capture))
    }
}

impl fmt::Debug for Capturer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Capturer")
            .field("lists_targets", &self.lister().is_some())
            .field("has_picker", &self.picker().is_some())
            .field("lists_audio_apps", &self.audio_apps().is_some())
            .finish()
    }
}

pub struct TargetLister<'a> {
    lister: &'a dyn ListTargets,
}

impl TargetLister<'_> {
    pub fn targets(&self) -> Result<Targets, CaptureError> {
        self.lister.targets()
    }
}

pub struct TargetPicker<'a> {
    picker: &'a dyn PickTarget,
}

impl TargetPicker<'_> {
    /// Shows the picker and returns at once. `done` runs once, on a thread of
    /// the system's choosing, with `Ok(None)` when the user cancelled.
    pub fn pick(
        &self,
        options: PickerOptions,
        done: impl FnOnce(Result<Option<PickedTarget>, CaptureError>) + Send + 'static,
    ) {
        self.picker.pick(options, Box::new(done));
    }
}

pub struct AudioAppLister<'a> {
    lister: &'a dyn ListAudioApps,
}

impl AudioAppLister<'_> {
    pub fn playing(&self) -> Result<Vec<AudioApp>, CaptureError> {
        self.lister.playing()
    }
}
