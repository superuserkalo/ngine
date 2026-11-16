use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use windows_capture::GraphicsCaptureItem;
use windows_capture::graphics_capture_picker::{Error as PickerError, GraphicsCapturePicker};

use super::convert::{self, Input};
use crate::backend::PickDone;
use crate::error::CaptureError;
use crate::target::{PickedHandle, PickedTarget, Target, TargetKind};

pub(super) struct Picked {
    item: GraphicsCaptureItem,
    _keeps_the_window_that_owns_the_item_alive: Sender<()>,
}

impl Picked {
    pub(super) fn item(&self) -> GraphicsCaptureItem {
        self.item.clone()
    }
}

fn label_of(item: &GraphicsCaptureItem) -> String {
    item.DisplayName()
        .map(|name| name.to_string_lossy())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Picked content".to_owned())
}

fn show_and_wait(done: PickDone) {
    let picked = match GraphicsCapturePicker::pick_item() {
        Ok(Some(picked)) => picked,
        Ok(None) | Err(PickerError::Canceled) => return done(Ok(None)),
        Err(PickerError::WindowsError(error)) => {
            return done(Err(convert::error_from_code(
                error.code().0,
                &error.message(),
                Input::Picture,
            )));
        }
    };
    let (target_is_alive, target_was_dropped) = mpsc::channel::<()>();
    let handle = PickedHandle {
        kind: TargetKind::Picked,
        label: label_of(&picked.item),
        audio_app: None,
        native: Arc::new(Picked {
            item: picked.item.clone(),
            _keeps_the_window_that_owns_the_item_alive: target_is_alive,
        }),
    };
    done(Ok(Some(PickedTarget::new(Target::picked(handle), None))));
    while target_was_dropped.recv().is_ok() {}
    drop(picked);
}

pub(super) fn pick(done: PickDone) {
    let done = Arc::new(Mutex::new(Some(done)));
    let for_the_picker = Arc::clone(&done);
    let take =
        |done: &Mutex<Option<PickDone>>| done.lock().unwrap_or_else(PoisonError::into_inner).take();
    let thread = std::thread::Builder::new()
        .name("ngine-capture-picker".to_owned())
        .spawn(move || {
            if let Some(done) = take(&for_the_picker) {
                show_and_wait(done);
            }
        });
    if let Err(error) = thread
        && let Some(done) = take(&done)
    {
        done(Err(CaptureError::Os(error.to_string())));
    }
}
