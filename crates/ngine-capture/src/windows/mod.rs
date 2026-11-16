#[cfg(target_os = "windows")]
mod audio;
mod clock;
#[cfg(target_os = "windows")]
mod content;
mod convert;
#[cfg(target_os = "windows")]
mod picker;
#[cfg(target_os = "windows")]
mod stream;
#[cfg(target_os = "windows")]
mod surface;

#[cfg(target_os = "windows")]
pub use self::surface::D3d11Surface;
#[cfg(target_os = "windows")]
pub(crate) use self::system::WindowsBackend;

#[cfg(target_os = "windows")]
mod system {
    use super::{audio, content, picker, stream};
    use crate::backend::{Backend, ListAudioApps, ListTargets, PickDone, PickTarget};
    use crate::error::CaptureError;
    use crate::options::SessionOptions;
    use crate::session::{EventSink, RunningCapture};
    use crate::target::{MicrophoneDevice, PickerOptions, Target, Targets};

    pub(crate) struct WindowsBackend;

    impl ListTargets for WindowsBackend {
        fn targets(&self) -> Result<Targets, CaptureError> {
            content::list()
        }
    }

    impl PickTarget for WindowsBackend {
        fn pick(&self, _options: PickerOptions, done: PickDone) {
            picker::pick(done);
        }
    }

    impl Backend for WindowsBackend {
        fn name(&self) -> &'static str {
            "Windows.Graphics.Capture"
        }

        fn target_lister(&self) -> Option<&dyn ListTargets> {
            Some(self)
        }

        fn target_picker(&self) -> Option<&dyn PickTarget> {
            Some(self)
        }

        fn audio_app_lister(&self) -> Option<&dyn ListAudioApps> {
            None
        }

        fn microphones(&self) -> Result<Vec<MicrophoneDevice>, CaptureError> {
            audio::microphones()
        }

        fn start(
            &self,
            target: &Target,
            options: &SessionOptions,
            sink: EventSink,
        ) -> Result<Box<dyn RunningCapture>, CaptureError> {
            stream::start(target, options, sink)
        }
    }
}
