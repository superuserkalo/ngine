use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use rune::dirs::AppDirs;
use rune::{App, AppHandle, AppSpec, OverlayHandle, ShortcutError, ShortcutId};

use crate::app::{self, Action, NAME, Ngine, WINDOW_SIZE};
use crate::cli::{Command, Reply};
use crate::engine::fake::{Clock, FakeEngine};
use crate::hotkeys;
use crate::ipc::{self, Address};
use crate::platform::{self, Autostart, Folders};
use crate::settings::{Hotkeys, Settings, Store};
use crate::toast::{self, Toast};
use crate::tray::{self, Tray};
use crate::ui;

const SETTINGS_FILE: &str = "settings.json";
const REPLAY_BUFFER_FOLDER: &str = "replay";
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);
const TOAST_FADE: Duration = Duration::from_millis(200);
const SAMPLE_CONFIRMATION: &str = "Replay saved · 31 s";
pub const OVERLAY_FLAG: &str = "--overlay";

/// The parts of the running app that live outside its window: the tray icon,
/// the saved-replay overlay, global hotkeys, the login entry and the command
/// socket.
pub struct Shell {
    handle: AppHandle<Ngine>,
    toast: OverlayHandle<Toast>,
    tray: Option<Tray>,
    shown_tray: Option<tray::Model>,
    shortcuts: Vec<ShortcutId>,
    autostart: Autostart,
    _server: ipc::Server,
}

fn describe(error: &ShortcutError) -> String {
    match error {
        ShortcutError::Conflict => "Another app already uses this shortcut".into(),
        ShortcutError::Unsupported => "This desktop does not offer global shortcuts".into(),
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

    pub fn hide_toast(&self) {
        self.toast.fade_out(TOAST_FADE);
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

fn spec() -> AppSpec<Ngine> {
    AppSpec {
        scenes: app::demo::SCENES,
        poll: Some(app::poll),
        on_key: Some(app::on_key),
        min_size: Some(WINDOW_SIZE),
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
        let engine = FakeEngine::new(Clock::system(), platform::LISTS_CAPTURE_TARGETS);
        Ngine::new(Box::new(engine), None, settings, store)
    }
}

pub fn send(command: Command) -> Result<Reply, String> {
    let places = Places::find()?;
    ipc::send(&places.address(), command).map_err(|error| error.to_string())
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
    let mut app = App::new(spec())
        .args(Vec::new())
        .init(move || places.start())
        .keep_running()
        .on_close(app::on_close);
    if background {
        app = app.windowless();
    }
    let toast = app.overlay(toast::spec(), Toast::default());
    let handle = app.handle();
    let commands = handle.clone();
    let server = ipc::serve(&address, move |command| ask(&commands, command))
        .map_err(|error| format!("ngine could not listen for commands: {error}"))?;
    let attached = handle.clone();
    handle.update(move |app, tx| {
        app.attach(Shell {
            handle: attached,
            toast,
            tray: None,
            shown_tray: None,
            shortcuts: Vec::new(),
            autostart,
            _server: server,
        });
        tx.wake_after(Duration::ZERO);
    });
    app.try_run()
}

/// Runs Rune's headless tooling (`--snap`, `--tree`, `--list-scenes` and the
/// rest) on a simulated app that touches no settings file, socket or tray.
///
/// With [`OVERLAY_FLAG`] among the arguments, the tooling draws the
/// saved-replay overlay instead of the window.
pub fn run_tooling(mut arguments: Vec<String>) -> Result<(), String> {
    let wanted = arguments.len();
    arguments.retain(|argument| argument != OVERLAY_FLAG);
    if arguments.len() == wanted {
        return App::new(spec()).args(arguments).try_run();
    }
    let overlay = AppSpec::new("ngine-saved", toast::SIZE, sample_toast, toast::view);
    App::new(overlay).args(arguments).try_run()
}

fn sample_toast() -> Toast {
    Toast {
        text: SAMPLE_CONFIRMATION.into(),
    }
}
