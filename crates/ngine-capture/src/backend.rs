use crate::error::CaptureError;
use crate::options::SessionOptions;
use crate::session::{EventSink, RunningCapture};
use crate::target::{AudioApp, MicrophoneDevice, PickedTarget, PickerOptions, Target, Targets};

pub(crate) type PickDone = Box<dyn FnOnce(Result<Option<PickedTarget>, CaptureError>) + Send>;

pub(crate) trait ListTargets {
    fn targets(&self) -> Result<Targets, CaptureError>;
}

pub(crate) trait PickTarget {
    fn pick(&self, options: PickerOptions, done: PickDone);
}

pub(crate) trait ListAudioApps {
    fn playing(&self) -> Result<Vec<AudioApp>, CaptureError>;
}

pub(crate) trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    fn target_lister(&self) -> Option<&dyn ListTargets>;
    fn target_picker(&self) -> Option<&dyn PickTarget>;
    fn audio_app_lister(&self) -> Option<&dyn ListAudioApps>;
    fn microphones(&self) -> Result<Vec<MicrophoneDevice>, CaptureError>;
    fn start(
        &self,
        target: &Target,
        options: &SessionOptions,
        sink: EventSink,
    ) -> Result<Box<dyn RunningCapture>, CaptureError>;
}
