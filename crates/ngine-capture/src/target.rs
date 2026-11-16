#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::any::Any;
use std::fmt;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::Arc;

use crate::frame::Size;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScreenId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Screen {
    pub id: ScreenId,
    pub name: String,
    /// In pixels.
    pub size: Size,
}

impl Screen {
    pub fn new(id: ScreenId, name: impl Into<String>, size: Size) -> Self {
        Self {
            id,
            name: name.into(),
            size,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Window {
    pub id: WindowId,
    pub title: String,
    pub app_name: String,
    pub process_id: Option<u32>,
    /// In pixels.
    pub size: Size,
}

impl Window {
    pub fn new(
        id: WindowId,
        title: impl Into<String>,
        app_name: impl Into<String>,
        process_id: Option<u32>,
        size: Size,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            app_name: app_name.into(),
            process_id,
            size,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct App {
    pub process_id: u32,
    pub name: String,
}

impl App {
    pub fn new(process_id: u32, name: impl Into<String>) -> Self {
        Self {
            process_id,
            name: name.into(),
        }
    }
}

/// An app whose sound can be captured on its own.
///
/// Where the system knows which app a target belongs to, [`Target::audio_app`]
/// gives it. Elsewhere the user picks one from
/// [`AudioAppLister::playing`](crate::AudioAppLister::playing).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct AudioApp {
    pub id: u64,
    pub name: String,
}

impl AudioApp {
    pub fn new(id: u64, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Targets {
    pub screens: Vec<Screen>,
    pub windows: Vec<Window>,
    /// Empty where the system cannot capture a whole app.
    pub apps: Vec<App>,
}

impl Targets {
    pub fn new(screens: Vec<Screen>, windows: Vec<Window>, apps: Vec<App>) -> Self {
        Self {
            screens,
            windows,
            apps,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TargetKind {
    Screen,
    Window,
    App,
    /// Chosen in the system picker, which did not say what it is.
    Picked,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone)]
pub(crate) struct PickedHandle {
    pub(crate) kind: TargetKind,
    pub(crate) label: String,
    pub(crate) audio_app: Option<AudioApp>,
    pub(crate) native: Arc<dyn Any + Send + Sync>,
}

#[derive(Clone)]
pub(crate) enum TargetSource {
    Screen(Screen),
    Window(Window),
    App(App),
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    Picked(PickedHandle),
}

/// What a session captures.
///
/// A target comes from a listing, through `From<Screen>`, `From<Window>` or
/// `From<App>`, or from the system picker.
#[derive(Clone)]
pub struct Target {
    source: TargetSource,
    audio_app: Option<AudioApp>,
}

impl Target {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn picked(handle: PickedHandle) -> Self {
        Self {
            audio_app: handle.audio_app.clone(),
            source: TargetSource::Picked(handle),
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn source(&self) -> &TargetSource {
        &self.source
    }

    pub fn kind(&self) -> TargetKind {
        match &self.source {
            TargetSource::Screen(_) => TargetKind::Screen,
            TargetSource::Window(_) => TargetKind::Window,
            TargetSource::App(_) => TargetKind::App,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            TargetSource::Picked(handle) => handle.kind,
        }
    }

    pub fn label(&self) -> &str {
        match &self.source {
            TargetSource::Screen(screen) => &screen.name,
            TargetSource::Window(window) => &window.title,
            TargetSource::App(app) => &app.name,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            TargetSource::Picked(handle) => &handle.label,
        }
    }

    /// The app this target belongs to, when the system says which one it is.
    /// A screen has none.
    pub fn audio_app(&self) -> Option<&AudioApp> {
        self.audio_app.as_ref()
    }
}

impl From<Screen> for Target {
    fn from(screen: Screen) -> Self {
        Self {
            source: TargetSource::Screen(screen),
            audio_app: None,
        }
    }
}

impl From<Window> for Target {
    fn from(window: Window) -> Self {
        Self {
            audio_app: window
                .process_id
                .map(|process_id| AudioApp::new(u64::from(process_id), window.app_name.clone())),
            source: TargetSource::Window(window),
        }
    }
}

impl From<App> for Target {
    fn from(app: App) -> Self {
        Self {
            audio_app: Some(AudioApp::new(u64::from(app.process_id), app.name.clone())),
            source: TargetSource::App(app),
        }
    }
}

impl fmt::Debug for Target {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Target")
            .field("kind", &self.kind())
            .field("label", &self.label())
            .field("audio_app", &self.audio_app)
            .finish_non_exhaustive()
    }
}

/// Lets the system picker offer the last choice again without asking.
/// Store [`RestoreToken::as_str`] and hand it back in [`PickerOptions`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RestoreToken(String);

impl RestoreToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The window the picker belongs to, on systems whose picker asks for one.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParentWindow {
    /// A Win32 `HWND`.
    Win32(isize),
    /// A handle exported with the `xdg-foreign` Wayland protocol.
    Wayland(String),
    /// An X11 window id.
    X11(u32),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PickerOptions {
    pub restore_token: Option<RestoreToken>,
    pub parent: Option<ParentWindow>,
}

impl PickerOptions {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_restore_token(mut self, token: RestoreToken) -> Self {
        self.restore_token = Some(token);
        self
    }

    #[must_use]
    pub fn with_parent(mut self, parent: ParentWindow) -> Self {
        self.parent = Some(parent);
        self
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PickedTarget {
    pub target: Target,
    /// Present only where the system picker can skip its dialogue next time.
    pub restore_token: Option<RestoreToken>,
}

impl PickedTarget {
    pub fn new(target: Target, restore_token: Option<RestoreToken>) -> Self {
        Self {
            target,
            restore_token,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct MicrophoneDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

impl MicrophoneDevice {
    pub fn new(id: impl Into<String>, name: impl Into<String>, is_default: bool) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            is_default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_and_an_app_name_the_app_whose_sound_they_make() {
        let window = Window::new(
            WindowId(9),
            "Notes",
            "Editor",
            Some(41),
            Size::new(800, 600),
        );
        let app = App::new(41, "Editor");

        assert_eq!(
            Target::from(window).audio_app(),
            Some(&AudioApp::new(41, "Editor"))
        );
        assert_eq!(
            Target::from(app).audio_app(),
            Some(&AudioApp::new(41, "Editor"))
        );
    }

    #[test]
    fn a_screen_and_a_window_of_an_unknown_process_name_no_app() {
        let screen = Screen::new(ScreenId(1), "Main", Size::new(1920, 1080));
        let window = Window::new(WindowId(9), "Notes", "Editor", None, Size::new(800, 600));

        assert_eq!(Target::from(screen).audio_app(), None);
        assert_eq!(Target::from(window).audio_app(), None);
    }
}
