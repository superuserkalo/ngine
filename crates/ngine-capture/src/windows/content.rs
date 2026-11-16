use std::ffi::c_void;

use windows_capture::GraphicsCaptureItem;
use windows_capture::monitor::Monitor;
use windows_capture::settings::GraphicsCaptureItemType;
use windows_capture::window::Window as NativeWindow;

use super::convert;
use super::picker::Picked;
use crate::error::CaptureError;
use crate::frame::Size;
use crate::target::{Screen, ScreenId, Target, TargetSource, Targets, Window, WindowId};

fn id_of(handle: *mut c_void) -> u64 {
    u64::try_from(handle.expose_provenance()).unwrap_or(u64::MAX)
}

fn handle_of(id: u64) -> Option<*mut c_void> {
    usize::try_from(id)
        .ok()
        .map(std::ptr::with_exposed_provenance_mut)
}

fn screen_from(monitor: Monitor) -> Option<Screen> {
    let size = Size::new(monitor.width().ok()?, monitor.height().ok()?);
    let name = monitor
        .name()
        .or_else(|_| monitor.device_name())
        .unwrap_or_else(|_| "Display".to_owned());
    Some(Screen::new(
        ScreenId(id_of(monitor.as_raw_hmonitor())),
        name,
        size,
    ))
}

fn window_from(window: NativeWindow) -> Option<Window> {
    let title = window.title().ok().filter(|title| !title.is_empty())?;
    let size = convert::size(window.width().ok()?, window.height().ok()?)?;
    let process_id = window.process_id().ok();
    Some(Window::new(
        WindowId(id_of(window.as_raw_hwnd())),
        title,
        window.process_name().unwrap_or_default(),
        process_id,
        size,
    ))
}

fn belongs_to_this_process(window: &Window) -> bool {
    window.process_id == Some(std::process::id())
}

pub(super) fn list() -> Result<Targets, CaptureError> {
    let monitors = Monitor::enumerate().map_err(|error| CaptureError::Os(error.to_string()))?;
    let windows = NativeWindow::enumerate().map_err(|error| CaptureError::Os(error.to_string()))?;
    Ok(Targets::new(
        monitors.into_iter().filter_map(screen_from).collect(),
        windows
            .into_iter()
            .filter_map(window_from)
            .filter(|window| !belongs_to_this_process(window))
            .collect(),
        Vec::new(),
    ))
}

pub(super) enum Item {
    Monitor(Monitor),
    Window(NativeWindow),
    Picked(GraphicsCaptureItem),
}

fn monitor_that_windows_capture_stores_beside_a_picked_item_and_never_reads()
-> Result<Monitor, CaptureError> {
    Monitor::primary().map_err(|error| CaptureError::Os(error.to_string()))
}

impl TryFrom<Item> for GraphicsCaptureItemType {
    type Error = CaptureError;

    fn try_from(item: Item) -> Result<Self, CaptureError> {
        match item {
            Item::Monitor(monitor) => monitor.try_into().map_err(|_| CaptureError::TargetGone),
            Item::Window(window) => window.try_into().map_err(|_| CaptureError::TargetGone),
            Item::Picked(item) => Ok(Self::Monitor((
                item,
                monitor_that_windows_capture_stores_beside_a_picked_item_and_never_reads()?,
            ))),
        }
    }
}

pub(super) fn resolve(target: &Target) -> Result<Item, CaptureError> {
    match target.source() {
        TargetSource::Screen(screen) => {
            let connected = Monitor::enumerate().unwrap_or_default();
            connected
                .into_iter()
                .find(|monitor| id_of(monitor.as_raw_hmonitor()) == screen.id.0)
                .map(Item::Monitor)
                .ok_or(CaptureError::TargetGone)
        }
        TargetSource::Window(window) => handle_of(window.id.0)
            .map(NativeWindow::from_raw_hwnd)
            .filter(NativeWindow::is_valid)
            .map(Item::Window)
            .ok_or(CaptureError::TargetGone),
        TargetSource::App(_) => Err(CaptureError::Unsupported(
            "Windows captures a screen or a window, not a whole app".to_owned(),
        )),
        TargetSource::Picked(handle) => handle
            .native
            .downcast_ref::<Picked>()
            .map(|picked| Item::Picked(picked.item()))
            .ok_or(CaptureError::TargetGone),
    }
}
