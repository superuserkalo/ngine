use screencapturekit::shareable_content::{
    SCDisplay, SCRunningApplication, SCShareableContent, SCShareableContentInfo, SCWindow,
};
use screencapturekit::stream::content_filter::SCContentFilter;

use super::convert;
use crate::error::CaptureError;
use crate::frame::Size;
use crate::target::{
    App, PickedHandle, Screen, ScreenId, Target, TargetSource, Targets, Window, WindowId,
};

const ORDINARY_WINDOW_LAYER: i32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sound {
    Everything,
    App(u64),
    Unknown,
}

#[derive(Clone)]
pub(super) struct Resolved {
    pub(super) filter: SCContentFilter,
    pub(super) size: Size,
    pub(super) sound: Sound,
}

fn all_content() -> Result<SCShareableContent, CaptureError> {
    SCShareableContent::create()
        .with_on_screen_windows_only(false)
        .get()
        .map_err(|error| convert::error_from(&error))
}

fn process_id_of(app: &SCRunningApplication) -> Option<u32> {
    u32::try_from(app.process_id()).ok()
}

fn belongs_to_this_process(window: &SCWindow) -> bool {
    window
        .owning_application()
        .and_then(|app| process_id_of(&app))
        == Some(std::process::id())
}

fn windows_of_this_process(content: &SCShareableContent) -> Vec<SCWindow> {
    content
        .windows()
        .into_iter()
        .filter(belongs_to_this_process)
        .collect()
}

pub(super) fn window_ids_of_this_process_read_without_the_screen_recording_permission() -> Vec<u32>
{
    SCShareableContent::current_process()
        .map(|content| content.windows().iter().map(SCWindow::window_id).collect())
        .unwrap_or_default()
}

fn pixel_size(filter: &SCContentFilter) -> Option<Size> {
    let (width, height) = SCShareableContentInfo::for_filter(filter)?.pixel_size();
    (width > 0 && height > 0).then_some(Size::new(width, height))
}

fn whole_display(
    display: &SCDisplay,
    hidden: &[SCWindow],
) -> Result<SCContentFilter, CaptureError> {
    let hidden: Vec<&SCWindow> = hidden.iter().collect();
    SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&hidden)
        .build()
        .map_err(|error| convert::error_from(&error))
}

fn one_window(window: &SCWindow) -> Result<SCContentFilter, CaptureError> {
    SCContentFilter::create()
        .with_window(window)
        .build()
        .map_err(|error| convert::error_from(&error))
}

fn one_app(
    display: &SCDisplay,
    app: &SCRunningApplication,
) -> Result<SCContentFilter, CaptureError> {
    SCContentFilter::create()
        .with_display(display)
        .with_including_applications(&[app], &[])
        .build()
        .map_err(|error| convert::error_from(&error))
}

fn sized(filter: SCContentFilter, sound: Sound) -> Result<Resolved, CaptureError> {
    let size = pixel_size(&filter).ok_or_else(|| {
        CaptureError::Os("ScreenCaptureKit gave no size for the capture target".to_owned())
    })?;
    Ok(Resolved {
        filter,
        size,
        sound,
    })
}

fn display_showing(content: &SCShareableContent, app_process_id: u32) -> Option<SCDisplay> {
    let displays = content.displays();
    let window_of_the_app = content.windows().into_iter().find(|window| {
        window.is_on_screen()
            && window
                .owning_application()
                .and_then(|app| process_id_of(&app))
                == Some(app_process_id)
    });
    let showing = window_of_the_app.and_then(|window| {
        let frame = window.frame();
        let centre_x = frame.origin.x + frame.size.width / 2.0;
        let centre_y = frame.origin.y + frame.size.height / 2.0;
        displays.iter().find(|display| {
            let screen = display.frame();
            (screen.origin.x..screen.origin.x + screen.size.width).contains(&centre_x)
                && (screen.origin.y..screen.origin.y + screen.size.height).contains(&centre_y)
        })
    });
    showing.or(displays.first()).cloned()
}

fn running_app(content: &SCShareableContent, process_id: u64) -> Option<SCRunningApplication> {
    content
        .applications()
        .into_iter()
        .find(|app| process_id_of(app).map(u64::from) == Some(process_id))
}

pub(super) fn resolve(target: &Target) -> Result<Resolved, CaptureError> {
    let content = all_content()?;
    match target.source() {
        TargetSource::Screen(screen) => {
            let display = content
                .displays()
                .into_iter()
                .find(|display| u64::from(display.display_id()) == screen.id.0)
                .ok_or(CaptureError::TargetGone)?;
            let filter = whole_display(&display, &windows_of_this_process(&content))?;
            sized(filter, Sound::Everything)
        }
        TargetSource::Window(window) => {
            let found = content
                .windows()
                .into_iter()
                .find(|candidate| u64::from(candidate.window_id()) == window.id.0)
                .ok_or(CaptureError::TargetGone)?;
            let sound = found
                .owning_application()
                .and_then(|app| process_id_of(&app))
                .map_or(Sound::Unknown, |process_id| {
                    Sound::App(u64::from(process_id))
                });
            sized(one_window(&found)?, sound)
        }
        TargetSource::App(app) => {
            let running =
                running_app(&content, u64::from(app.process_id)).ok_or(CaptureError::TargetGone)?;
            let display =
                display_showing(&content, app.process_id).ok_or(CaptureError::TargetGone)?;
            sized(
                one_app(&display, &running)?,
                Sound::App(u64::from(app.process_id)),
            )
        }
        TargetSource::Picked(handle) => picked(handle),
    }
}

fn picked(handle: &PickedHandle) -> Result<Resolved, CaptureError> {
    let filter = handle
        .native
        .downcast_ref::<SCContentFilter>()
        .ok_or_else(|| {
            CaptureError::Unsupported(
                "this target was picked by a different capture backend".to_owned(),
            )
        })?;
    let sound = handle
        .audio_app
        .as_ref()
        .map_or(Sound::Unknown, |app| Sound::App(app.id));
    sized(filter.clone(), sound)
}

fn first_display(content: &SCShareableContent) -> Result<SCDisplay, CaptureError> {
    content
        .displays()
        .into_iter()
        .next()
        .ok_or_else(|| CaptureError::Unavailable("no display to capture sound through".to_owned()))
}

pub(super) fn everything_audible() -> Result<SCContentFilter, CaptureError> {
    let content = all_content()?;
    whole_display(&first_display(&content)?, &[])
}

pub(super) fn only_audible(app_process_id: u64) -> Result<SCContentFilter, CaptureError> {
    let content = all_content()?;
    let app = running_app(&content, app_process_id).ok_or_else(|| {
        CaptureError::Unavailable("the app whose sound was asked for is not running".to_owned())
    })?;
    one_app(&first_display(&content)?, &app)
}

pub(super) fn list() -> Result<Targets, CaptureError> {
    let content = SCShareableContent::create()
        .with_on_screen_windows_only(false)
        .with_exclude_desktop_windows(true)
        .get()
        .map_err(|error| convert::error_from(&error))?;
    let screens = content
        .displays()
        .iter()
        .zip(1..)
        .map(|(display, number): (&SCDisplay, u32)| {
            let size = whole_display(display, &[])
                .ok()
                .and_then(|filter| pixel_size(&filter))
                .unwrap_or(Size::new(display.width(), display.height()));
            Screen::new(
                ScreenId(u64::from(display.display_id())),
                format!("Display {number}"),
                size,
            )
        })
        .collect();
    let mut windows = Vec::new();
    let mut apps: Vec<App> = Vec::new();
    for window in content.windows() {
        let Some(listed) = listed_window(&window) else {
            continue;
        };
        if let Some(process_id) = listed.process_id
            && apps.iter().all(|app| app.process_id != process_id)
        {
            apps.push(App::new(process_id, listed.app_name.clone()));
        }
        windows.push(listed);
    }
    Ok(Targets::new(screens, windows, apps))
}

fn listed_window(window: &SCWindow) -> Option<Window> {
    let app = window.owning_application()?;
    let title = window.title().filter(|title| !title.is_empty())?;
    let ordinary = window.window_layer() == ORDINARY_WINDOW_LAYER;
    if !ordinary || belongs_to_this_process(window) {
        return None;
    }
    let frame = window.frame();
    let on_screen = window.is_on_screen();
    let from_frame = || {
        convert::whole_number(frame.size.width)
            .zip(convert::whole_number(frame.size.height))
            .map(|(width, height)| Size::new(width, height))
    };
    let size = if on_screen {
        one_window(window)
            .ok()
            .and_then(|filter| pixel_size(&filter))
            .or_else(from_frame)
    } else {
        from_frame()
    }?;
    Some(
        Window::new(
            WindowId(u64::from(window.window_id())),
            title,
            app.application_name(),
            process_id_of(&app),
            size,
        )
        .with_on_screen(on_screen),
    )
}
