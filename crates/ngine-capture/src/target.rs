#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use std::any::Any;
use std::fmt;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use std::sync::Arc;

use crate::frame::Size;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScreenId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

/// A rectangle of a target's picture, in its pixels from the top-left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn size(self) -> Size {
        Size::new(self.width, self.height)
    }

    /// The part of this region that lies inside a picture of `size`. `None`
    /// when nothing of it does.
    pub fn within(self, size: Size) -> Option<Self> {
        let right = self.x.saturating_add(self.width).min(size.width);
        let bottom = self.y.saturating_add(self.height).min(size.height);
        (self.x < right && self.y < bottom).then(|| Self {
            x: self.x,
            y: self.y,
            width: right - self.x,
            height: bottom - self.y,
        })
    }

    /// The largest region inside this one whose corners all sit on even
    /// pixels, as a picture with half-size chroma needs. `None` when that
    /// leaves nothing.
    pub fn on_even_pixels(self) -> Option<Self> {
        let x = self.x.next_multiple_of(2);
        let y = self.y.next_multiple_of(2);
        let right = self.x.saturating_add(self.width) & !1;
        let bottom = self.y.saturating_add(self.height) & !1;
        (x < right && y < bottom).then(|| Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }
}

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
    /// False for a window that is minimised or on another desktop or Space.
    pub on_screen: bool,
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
            on_screen: true,
        }
    }

    #[must_use]
    pub fn with_on_screen(mut self, on_screen: bool) -> Self {
        self.on_screen = on_screen;
        self
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

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
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
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
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
    region: Option<Region>,
}

impl Target {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    pub(crate) fn picked(handle: PickedHandle) -> Self {
        Self {
            audio_app: handle.audio_app.clone(),
            source: TargetSource::Picked(handle),
            region: None,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    pub(crate) fn source(&self) -> &TargetSource {
        &self.source
    }

    pub fn kind(&self) -> TargetKind {
        match &self.source {
            TargetSource::Screen(_) => TargetKind::Screen,
            TargetSource::Window(_) => TargetKind::Window,
            TargetSource::App(_) => TargetKind::App,
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            TargetSource::Picked(handle) => handle.kind,
        }
    }

    pub fn label(&self) -> &str {
        match &self.source {
            TargetSource::Screen(screen) => &screen.name,
            TargetSource::Window(window) => &window.title,
            TargetSource::App(app) => &app.name,
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            TargetSource::Picked(handle) => &handle.label,
        }
    }

    /// The app this target belongs to, when the system says which one it is.
    /// A screen has none.
    pub fn audio_app(&self) -> Option<&AudioApp> {
        self.audio_app.as_ref()
    }

    /// Captures only `region` of this target's picture, in the pixels its
    /// listing gives as its size. Frames then have the region's size. A part
    /// of the region that falls outside the picture is left out, and a frame
    /// that holds none of it is not delivered.
    #[must_use]
    pub fn with_region(mut self, region: Region) -> Self {
        self.region = Some(region);
        self
    }

    pub fn region(&self) -> Option<Region> {
        self.region
    }
}

impl From<Screen> for Target {
    fn from(screen: Screen) -> Self {
        Self {
            source: TargetSource::Screen(screen),
            audio_app: None,
            region: None,
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
            region: None,
        }
    }
}

impl From<App> for Target {
    fn from(app: App) -> Self {
        Self {
            audio_app: Some(AudioApp::new(u64::from(app.process_id), app.name.clone())),
            source: TargetSource::App(app),
            region: None,
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
            .field("region", &self.region)
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
    fn a_region_keeps_only_what_lies_on_the_picture() {
        let picture = Size::new(1920, 1080);

        assert_eq!(
            Region::new(1800, 1000, 400, 400).within(picture),
            Some(Region::new(1800, 1000, 120, 80))
        );
        assert_eq!(Region::new(1920, 0, 10, 10).within(picture), None);
        assert_eq!(
            Region::new(10, 20, 30, 40).within(picture),
            Some(Region::new(10, 20, 30, 40))
        );
    }

    #[test]
    fn a_region_on_even_pixels_shrinks_inward() {
        assert_eq!(
            Region::new(3, 5, 101, 51).on_even_pixels(),
            Some(Region::new(4, 6, 100, 50))
        );
        assert_eq!(
            Region::new(4, 6, 100, 50).on_even_pixels(),
            Some(Region::new(4, 6, 100, 50))
        );
        assert_eq!(Region::new(3, 3, 1, 1).on_even_pixels(), None);
    }

    #[test]
    fn a_target_carries_the_region_it_was_given() {
        let screen = Screen::new(ScreenId(1), "Main", Size::new(1920, 1080));
        let region = Region::new(0, 0, 640, 360);

        assert_eq!(Target::from(screen.clone()).region(), None);
        assert_eq!(
            Target::from(screen).with_region(region).region(),
            Some(region)
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
