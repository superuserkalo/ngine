mod audio;
mod content;
mod convert;
mod stream;
mod surface;

use std::sync::Arc;

use screencapturekit::audio_devices::AudioInputDevice;
use screencapturekit::content_sharing_picker::{
    SCContentSharingPicker, SCContentSharingPickerConfiguration, SCContentSharingPickerMode,
    SCPickerOutcome, SCPickerResult,
};
use screencapturekit::shareable_content::{SCRunningApplication, SCWindow};
use screencapturekit::stream::content_filter::SCShareableContentStyle;

pub use self::surface::CoreVideoSurface;
use crate::backend::{Backend, ListAudioApps, ListTargets, PickDone, PickTarget};
use crate::error::CaptureError;
use crate::options::SessionOptions;
use crate::session::{EventSink, RunningCapture};
use crate::target::{
    AudioApp, MicrophoneDevice, PickedHandle, PickedTarget, PickerOptions, Target, TargetKind,
    TargetSource, Targets,
};

pub(crate) struct MacBackend;

impl ListTargets for MacBackend {
    fn targets(&self) -> Result<Targets, CaptureError> {
        content::list()
    }
}

fn audio_app_of(app: &SCRunningApplication) -> Option<AudioApp> {
    let process_id = u64::try_from(app.process_id()).ok()?;
    Some(AudioApp::new(process_id, app.application_name()))
}

fn picked_target(result: &SCPickerResult) -> Target {
    let filter = result.filter();
    let window = result.windows().into_iter().next();
    let app = result.applications().into_iter().next();
    let (kind, label, audio_app) = match filter.style() {
        Ok(SCShareableContentStyle::Window) => (
            TargetKind::Window,
            window.as_ref().and_then(SCWindow::title),
            window
                .and_then(|window| window.owning_application())
                .as_ref()
                .and_then(audio_app_of),
        ),
        Ok(SCShareableContentStyle::Application) => (
            TargetKind::App,
            app.as_ref().map(SCRunningApplication::application_name),
            app.as_ref().and_then(audio_app_of),
        ),
        Ok(SCShareableContentStyle::Display) => {
            (TargetKind::Screen, Some("Display".to_owned()), None)
        }
        Ok(_) | Err(_) => (TargetKind::Picked, None, None),
    };
    Target::picked(PickedHandle {
        kind,
        label: label.unwrap_or_else(|| "Picked content".to_owned()),
        audio_app,
        native: Arc::new(filter),
    })
}

impl PickTarget for MacBackend {
    fn pick(&self, _options: PickerOptions, done: PickDone) {
        let mut configuration = match SCContentSharingPickerConfiguration::new() {
            Ok(configuration) => configuration,
            Err(error) => return done(Err(convert::error_from(&error))),
        };
        configuration.set_allowed_picker_modes(&[
            SCContentSharingPickerMode::SingleDisplay,
            SCContentSharingPickerMode::SingleWindow,
            SCContentSharingPickerMode::SingleApplication,
        ]);
        configuration.set_excluded_window_ids(
            &content::window_ids_of_this_process_read_without_the_screen_recording_permission(),
        );
        SCContentSharingPicker::show(&configuration, move |outcome| {
            done(match outcome {
                SCPickerOutcome::Picked(result) => {
                    Ok(Some(PickedTarget::new(picked_target(&result), None)))
                }
                SCPickerOutcome::Cancelled => Ok(None),
                SCPickerOutcome::Error(message) => Err(convert::error_from_text(&message)),
            });
        });
    }
}

impl Backend for MacBackend {
    fn name(&self) -> &'static str {
        "ScreenCaptureKit"
    }

    fn target_lister(&self) -> Option<&dyn ListTargets> {
        Some(self)
    }

    fn target_picker(&self) -> Option<&dyn PickTarget> {
        SCContentSharingPicker::is_available().then_some(self as &dyn PickTarget)
    }

    fn audio_app_lister(&self) -> Option<&dyn ListAudioApps> {
        None
    }

    fn microphones(&self) -> Result<Vec<MicrophoneDevice>, CaptureError> {
        Ok(AudioInputDevice::list()
            .into_iter()
            .map(|device| MicrophoneDevice::new(device.id, device.name, device.is_default))
            .collect())
    }

    fn captures_only_the_region(&self, target: &Target) -> bool {
        matches!(target.source(), TargetSource::Screen(_))
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
