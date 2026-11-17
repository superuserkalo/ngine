use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use ngine_capture::Capturer;
use ngine_recorder::replay::remove_leftovers;
use rune::dirs::AppDirs;
use rune::{App, AppHandle, AppSpec, OverlayHandle, OverlayInput, ShortcutError, ShortcutId};

use crate::app::{self, Action, NAME, Ngine, SMALLEST_WINDOW, WINDOW_SIZE};
use crate::bar::{self, BarState};
use crate::cli::{Command, Reply};
use crate::engine::fake::{Clock, FakeEngine};
use crate::engine::real::RealEngine;
use crate::engine::{Engine, Target};
use crate::hotkeys;
use crate::ipc::{self, Address};
use crate::platform::{self, Autostart, Folders};
use crate::selection::Area;
use crate::settings::{Hotkeys, Settings, Store};
use crate::toast::{self, Toast};
use crate::tray::{self, Tray};
use crate::ui;
use crate::ui::selector::{self, Selector};

const SETTINGS_FILE: &str = "settings.json";
const REPLAY_BUFFER_FOLDER: &str = "replay";
const THUMBNAIL_FOLDER: &str = "thumbnails";
const RESTORE_TOKEN_FILE: &str = "picker-restore-token";
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);
const TOAST_FADE: Duration = Duration::from_millis(200);
const SAMPLE_CONFIRMATION: &str = "Replay saved · 31 s";
pub const OVERLAY_FLAG: &str = "--overlay";
/// Tooling draws the overlay that chooses a part of a screen.
pub const SELECTOR_FLAG: &str = "--selector";
/// Tooling draws the library of the folder that follows this flag.
pub const LIBRARY_FLAG: &str = "--library-of";
/// Tooling draws the clip editor on the video that follows this flag.
pub const EDITOR_FLAG: &str = "--editor-on";
/// The seconds into the video that the editor's playhead is put at.
pub const EDITOR_AT_FLAG: &str = "--editor-at";

/// The parts of the running app that live outside its window: the tray icon,
/// the saved-replay overlay, global hotkeys, the login entry and the command
/// socket.
pub struct Shell {
    handle: AppHandle<Ngine>,
    toast: OverlayHandle<Toast>,
    selector: OverlayHandle<Selector>,
    tray: Option<Tray>,
    shown_tray: Option<tray::Model>,
    shortcuts: Vec<ShortcutId>,
    autostart: Autostart,
    watchers: ipc::Watchers,
    _server: ipc::Server,
}

fn describe(error: &ShortcutError) -> String {
    match error {
        ShortcutError::Conflict => "Another app already uses this shortcut".into(),
        ShortcutError::Unsupported => hotkeys::NOT_OFFERED.into(),
        ShortcutError::PermissionDenied => "The desktop did not allow this shortcut".into(),
        ShortcutError::InvalidAccelerator(_) => hotkeys::UNUSABLE.into(),
        other => format!("This shortcut could not be set: {other}"),
    }
}

impl Shell {
    pub fn show_toast(&self, text: &str) {
        let text = text.to_owned();
        self.toast.update(move |toast, _| toast.text = text);
        self.toast.show();
    }

    pub fn publish(&self, state: BarState) {
        self.watchers.publish(state.encode());
    }

    pub fn hide_toast(&self) {
        self.toast.fade_out(TOAST_FADE);
    }

    fn place_selector(&self, screen: &Target) {
        if let Some(display) = screen.display {
            self.selector
                .set_display(platform::overlay_display(display.id));
        }
    }

    /// Shows the overlay that chooses a part of `screen`, starting from
    /// `area`, and gives it the keyboard.
    pub fn choose_selection(&self, screen: &Target, area: Option<Area>) {
        self.place_selector(screen);
        self.selector.set_input(OverlayInput::All);
        let screen = screen.clone();
        self.selector
            .update(move |selector, _| selector.choose_on(screen, area));
        self.selector.show();
        self.selector.request_keys();
    }

    /// Frames `area` of `screen` while it records, letting every click
    /// through.
    pub fn frame_selection(&self, screen: &Target, area: Area) {
        self.place_selector(screen);
        self.selector.set_input(OverlayInput::None);
        let screen = screen.clone();
        self.selector
            .update(move |selector, _| selector.frame(screen, area));
        self.selector.show();
    }

    pub fn hide_selector(&self) {
        self.selector.hide();
    }

    pub fn show_tray(&mut self, model: tray::Model) {
        if self.shown_tray.as_ref() == Some(&model) {
            return;
        }
        if self.tray.is_none() {
            let handle = self.handle.clone();
            self.tray = Tray::new(&model, move |action| perform_later(&handle, action));
        } else if let Some(tray) = &mut self.tray {
            tray.show(&model);
        }
        self.shown_tray = Some(model);
    }

    pub fn bind_hotkeys(&mut self, hotkeys: &Hotkeys) -> [Option<String>; hotkeys::COUNT] {
        for id in self.shortcuts.drain(..) {
            if let Err(error) = self.handle.unregister_shortcut(id) {
                eprintln!("ngine: a hotkey could not be released: {error}");
            }
        }
        let mut problems = hotkeys::problems(hotkeys);
        for (slot, accelerator) in hotkeys.all().into_iter().enumerate() {
            let Some(shortcut) = hotkeys::parse(accelerator).filter(|_| problems[slot].is_none())
            else {
                continue;
            };
            let action = hotkeys::ACTIONS[slot];
            let bound = self
                .handle
                .register_shortcut(shortcut, move |app: &mut Ngine, _| app.perform(action));
            match bound {
                Ok(id) => self.shortcuts.push(id),
                Err(error) => problems[slot] = Some(describe(&error)),
            }
        }
        problems
    }

    pub fn set_autostart(&self, enabled: bool) -> io::Result<()> {
        self.autostart.set(enabled)
    }

    pub fn open_window(&self) {
        self.handle.open();
    }

    pub fn quit(&self) {
        self.handle.quit();
    }
}

fn perform_later(handle: &AppHandle<Ngine>, action: Action) {
    handle.update(move |app, tx| {
        app.perform(action);
        tx.wake_after(Duration::ZERO);
    });
}

fn ask(handle: &AppHandle<Ngine>, command: Command) -> Reply {
    let (sender, receiver) = mpsc::channel();
    handle.update(move |app, tx| {
        let reply = app.execute(command);
        tx.wake_after(Duration::ZERO);
        if sender.send(reply).is_err() {
            eprintln!("ngine: the sender of a command stopped waiting for its answer");
        }
    });
    receiver
        .recv_timeout(ANSWER_TIMEOUT)
        .unwrap_or_else(|_| Err("ngine did not answer in time".into()))
}

fn window_app() -> App<Ngine> {
    platform::window_frame(App::new(spec()))
}

fn spec() -> AppSpec<Ngine> {
    AppSpec {
        scenes: app::demo::SCENES,
        poll: Some(app::poll),
        on_key: Some(app::on_key),
        min_size: Some(SMALLEST_WINDOW),
        ..AppSpec::new(NAME, WINDOW_SIZE, app::demo::idle, ui::view)
    }
}

struct Places {
    dirs: AppDirs,
    home: PathBuf,
}

impl Places {
    fn find() -> Result<Self, String> {
        let dirs = AppDirs::new(NAME).ok_or("ngine could not find your home folder")?;
        let home = dirs
            .home()
            .map(PathBuf::from)
            .ok_or("ngine could not find your home folder")?;
        Ok(Self { dirs, home })
    }

    fn address(&self) -> Address {
        Address::in_folder(self.dirs.state())
    }

    fn default_settings(&self) -> Settings {
        Settings::defaults(
            self.home.join(platform::VIDEOS_FOLDER).join(NAME),
            self.dirs.cache().join(REPLAY_BUFFER_FOLDER),
        )
    }

    fn autostart(&self) -> Result<Autostart, String> {
        let program = std::env::current_exe()
            .map_err(|error| format!("ngine could not find its own program file: {error}"))?;
        let folders = Folders {
            home: &self.home,
            config: self.dirs.config(),
        };
        Ok(Autostart::of(&folders, &program))
    }

    fn start(&self) -> Ngine {
        let mut store = Store::open(&self.dirs.config().join(SETTINGS_FILE));
        let settings = store.load(self.default_settings());
        if let Err(error) = remove_leftovers(&settings.replay.buffer_folder) {
            eprintln!("ngine: old replay buffer files could not be removed: {error}");
        }
        let engine: Box<dyn Engine> = match Capturer::system() {
            Ok(capturer) => Box::new(
                RealEngine::new(capturer)
                    .keeping_picks_in(self.dirs.state().join(RESTORE_TOKEN_FILE)),
            ),
            Err(error) => {
                eprintln!("ngine: capture is simulated on this system: {error}");
                Box::new(FakeEngine::new(
                    Clock::system(),
                    platform::LISTS_CAPTURE_TARGETS,
                ))
            }
        };
        let mut app = Ngine::new(engine, None, settings, store);
        app.use_library_on_disk(self.dirs.cache().join(THUMBNAIL_FOLDER));
        app
    }
}

pub fn send(command: Command) -> Result<Reply, String> {
    let places = Places::find()?;
    ipc::send(&places.address(), command).map_err(|error| error.to_string())
}

/// Prints the running ngine's state for a status bar such as Waybar.
pub fn show_on_the_bar(options: &[String]) -> Result<(), String> {
    let mode = bar::parse(options).map_err(|error| format!("{error}\n\n{}", bar::USAGE))?;
    let address = Places::find()?.address();
    bar::run(mode, || {
        ipc::watch(&address)
            .ok()
            .map(|lines| lines.map_while(|line| BarState::decode(&line)))
    })
}

/// Opens the app, or brings the window of the ngine that already runs
/// forward and returns.
pub fn run(background: bool) -> Result<(), String> {
    let places = Places::find()?;
    let address = places.address();
    if ipc::send(&address, Command::Show).is_ok() {
        println!("ngine is already running. Its window is now in front.");
        return Ok(());
    }
    let autostart = places.autostart()?;
    rune::set_app_id(NAME);
    let mut app = window_app()
        .args(Vec::new())
        .init(move || places.start())
        .keep_running()
        .on_close(app::on_close);
    if background {
        app = app.windowless();
    }
    let toast = app.overlay(toast::spec(), Toast::default());
    let handle = app.handle();
    let answers = handle.clone();
    let selector = app.overlay(
        selector::spec(),
        Selector::answering(move |answer| {
            answers.update(move |app, tx| {
                app.selection_answered(answer);
                tx.wake_after(Duration::ZERO);
            });
        }),
    );
    let commands = handle.clone();
    let server = ipc::serve(&address, move |command| ask(&commands, command))
        .map_err(|error| format!("ngine could not listen for commands: {error}"))?;
    let attached = handle.clone();
    handle.update(move |app, tx| {
        app.attach(Shell {
            handle: attached,
            toast,
            selector,
            tray: None,
            shown_tray: None,
            shortcuts: Vec::new(),
            autostart,
            watchers: server.watchers(),
            _server: server,
        });
        tx.wake_after(Duration::ZERO);
    });
    app.try_run()
}

fn taken_value(arguments: &mut Vec<String>, flag: &str) -> Option<String> {
    let at = arguments.iter().position(|argument| argument == flag)?;
    arguments.remove(at);
    (at < arguments.len()).then(|| arguments.remove(at))
}

/// Runs Rune's headless tooling (`--snap`, `--tree`, `--list-scenes` and the
/// rest) on a simulated app that touches no settings file, socket or tray.
///
/// With [`OVERLAY_FLAG`] among the arguments, the tooling draws the
/// saved-replay overlay instead of the window.
pub fn run_tooling(mut arguments: Vec<String>) -> Result<(), String> {
    if let Some(folder) = taken_value(&mut arguments, LIBRARY_FLAG) {
        return window_app()
            .args(arguments)
            .init(move || {
                let mut app = app::demo::idle();
                app.show_library_of(Path::new(&folder));
                app
            })
            .try_run();
    }
    if let Some(file) = taken_value(&mut arguments, EDITOR_FLAG) {
        let at = taken_value(&mut arguments, EDITOR_AT_FLAG)
            .and_then(|seconds| seconds.parse().ok())
            .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
            .unwrap_or_default();
        return window_app()
            .args(arguments)
            .init(move || {
                let mut app = app::demo::idle();
                app.show_editor_on(Path::new(&file), at);
                app
            })
            .try_run();
    }
    let wanted = arguments.len();
    arguments.retain(|argument| argument != SELECTOR_FLAG);
    if arguments.len() < wanted {
        let spec = AppSpec {
            scenes: selector::SCENES,
            ..AppSpec::new(
                selector::NAME,
                selector::SAMPLE_SIZE,
                selector::sample,
                selector::view,
            )
        };
        return App::new(spec).args(arguments).try_run();
    }
    arguments.retain(|argument| argument != OVERLAY_FLAG);
    if arguments.len() == wanted {
        return window_app().args(arguments).try_run();
    }
    let overlay = AppSpec::new("ngine-saved", toast::SIZE, sample_toast, toast::view);
    App::new(overlay).args(arguments).try_run()
}

fn sample_toast() -> Toast {
    Toast {
        text: SAMPLE_CONFIRMATION.into(),
    }
}
