use std::path::{Path, PathBuf};
use std::time::Duration;

use rune::{Key, KeyEvent, TextEdit, Tx};

use browse::Browse;

use crate::bar::BarState;
use crate::editor::Editor;
use crate::engine::fake::FakeEngine;
use crate::engine::{
    Capabilities, Device, Engine, Event, Inputs, Inventory, Permission, Problem, RecordingRequest,
    ReplayRequest, ReplayStorage, Status, SystemSound, Target, TargetKind,
};
use crate::format;
use crate::hotkeys;
use crate::library::{self, Library};
use crate::notification::Notification;
use crate::platform;
use crate::settings::{Settings, Store};
use crate::shell::Shell;
use crate::tray;

mod actions;
mod amounts;
pub mod browse;
mod capture;
pub mod demo;
mod media;
pub mod meters;

pub use actions::Action;
pub use amounts::{Amount, Draft};
pub use capture::HotkeyCapture;
pub use meters::Meters;

pub const NAME: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const WINDOW_SIZE: (f32, f32) = (1040.0, 680.0);
pub const SMALLEST_WINDOW: (f32, f32) = (720.0, 480.0);

const TICK: Duration = Duration::from_millis(250);
const METER_TICK: Duration = Duration::from_millis(50);
const READING_TICK: Duration = Duration::from_millis(50);
const NOTICE_SECONDS: f64 = 4.0;
const TOAST_SECONDS: f64 = 3.0;
const SAVE_DELAY_SECONDS: f64 = 0.5;
const BYTES_PER_MB: u64 = 1_000_000;
pub const RENAME_BOX: &str = "rename-box";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Capture,
    Library,
    Editor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Video,
    Audio,
    Replay,
    Hotkeys,
    Storage,
    General,
    About,
}

impl Section {
    pub const ALL: [Self; 7] = [
        Self::Video,
        Self::Audio,
        Self::Replay,
        Self::Hotkeys,
        Self::Storage,
        Self::General,
        Self::About,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    Output,
    Buffer,
}

/// The one thing that floats above the window: a menu, a list of choices or a
/// question. Opening one closes the one before it.
#[derive(Clone, Debug, PartialEq)]
pub enum Popup {
    Sources,
    Choices(String),
    ItemMenu {
        file: PathBuf,
        at: Option<(f32, f32)>,
    },
    EditorKeys,
    ConfirmTrash(Vec<PathBuf>),
    ConfirmReset(Section),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    pub is_error: bool,
    until: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Confirmation {
    pub text: String,
    until: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterPick {
    Record,
    Replay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayCost {
    pub megabytes: u64,
    pub on_disk: bool,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one separate fact about the window, not a state machine"
)]
pub struct Ngine {
    engine: Box<dyn Engine>,
    pub simulator: Option<FakeEngine>,
    pub capabilities: Capabilities,
    pub settings: Settings,
    store: Store,
    pub settings_were_reset: bool,
    pub screen: Screen,
    pub section: Section,
    pub settings_open: bool,
    pub popup: Option<Popup>,
    pub sources: Vec<Target>,
    pub target: Option<Target>,
    pub picking: bool,
    pub reading: bool,
    listed: bool,
    after_pick: Option<AfterPick>,
    pub status: Status,
    pub meters: Meters,
    pub microphones: Vec<Device>,
    pub system_sounds: Vec<SystemSound>,
    pub problems: Vec<Problem>,
    pub notice: Option<Notice>,
    pub confirmation: Option<Confirmation>,
    pub hotkey_problems: [Option<String>; hotkeys::COUNT],
    pub capture: Option<HotkeyCapture>,
    pub draft: Option<Draft>,
    pub free_space: Option<u64>,
    pub library: Library,
    pub browse: Browse,
    pub editor: Option<Editor>,
    pub clip_name: TextEdit,
    pub rename: TextEdit,
    pub renaming: Option<PathBuf>,
    pub trash: fn(&Path) -> Result<(), String>,
    shell: Option<Shell>,
    clock: f64,
    settings_dirty: bool,
    save_due: Option<f64>,
}

impl Ngine {
    pub fn new(
        engine: Box<dyn Engine>,
        simulator: Option<FakeEngine>,
        mut settings: Settings,
        store: Store,
    ) -> Self {
        let capabilities = engine.capabilities();
        if !capabilities.codecs.contains(&settings.video.codec)
            && let Some(codec) = capabilities.codecs.first()
        {
            settings.video.codec = *codec;
        }
        let mut app = Self {
            engine,
            simulator,
            settings_were_reset: store.found_damaged(),
            store,
            screen: Screen::Capture,
            section: Section::Video,
            settings_open: false,
            popup: None,
            target: None,
            sources: Vec::new(),
            picking: false,
            reading: false,
            listed: false,
            after_pick: None,
            status: Status::default(),
            meters: Meters::default(),
            microphones: Vec::new(),
            system_sounds: Vec::new(),
            problems: Vec::new(),
            notice: None,
            confirmation: None,
            hotkey_problems: hotkeys::problems(&settings.hotkeys),
            capture: None,
            draft: None,
            free_space: None,
            library: Library::offline(),
            browse: Browse::default(),
            editor: None,
            clip_name: TextEdit::new(""),
            rename: TextEdit::new(""),
            renaming: None,
            trash: library::move_to_trash,
            capabilities,
            settings,
            shell: None,
            clock: 0.0,
            settings_dirty: false,
            save_due: None,
        };
        app.read_inventory();
        app.refresh();
        app
    }

    pub fn attach(&mut self, mut shell: Shell) {
        self.hotkey_problems = shell.bind_hotkeys(&self.settings.hotkeys);
        self.shell = Some(shell);
        self.show_tray();
    }

    /// Seconds on the app's clock, as the last poll saw it.
    pub fn clock(&self) -> f64 {
        self.clock
    }

    pub fn memory_budget_mb(&self) -> u32 {
        self.settings
            .replay
            .memory_budget_mb
            .unwrap_or(self.capabilities.default_memory_budget_mb)
    }

    fn bytes_per_second(&self) -> u64 {
        self.engine
            .bit_rate(self.settings.video, self.settings.audio)
            / 8
    }

    pub fn megabytes_per_minute(&self) -> u64 {
        format::approximate_megabytes(self.bytes_per_second() * 60)
    }

    /// What a buffer of `length_seconds` would hold and where, with the
    /// current video and audio settings.
    pub fn cost_of(&self, length_seconds: u16) -> ReplayCost {
        let bytes = self.bytes_per_second() * u64::from(length_seconds);
        ReplayCost {
            megabytes: format::approximate_megabytes(bytes),
            on_disk: match self.settings.replay.storage {
                ReplayStorage::Memory => false,
                ReplayStorage::Disk => true,
                ReplayStorage::Auto => bytes > u64::from(self.memory_budget_mb()) * BYTES_PER_MB,
            },
        }
    }

    pub fn read_inventory(&mut self) {
        self.reading = true;
        self.engine.read_inventory(&self.settings.output_folder);
        for event in self.engine.take_events() {
            self.absorb(event);
        }
    }

    fn take_inventory(&mut self, inventory: Inventory) {
        self.reading = false;
        self.sources = inventory.sources;
        self.microphones = inventory.microphones;
        self.system_sounds = inventory.system_sounds;
        if inventory.free_space.is_some() {
            self.free_space = inventory.free_space;
        }
        if std::mem::replace(&mut self.listed, true) {
            return;
        }
        if self.target.is_none() && !self.picking {
            self.target = self.sources.first().cloned();
        }
        if self.settings.replay.start_with_app
            && self.target.is_some()
            && self.status.replay.is_none()
        {
            let started = self.set_replay(true);
            self.note(&started);
        }
    }

    /// The chosen microphone when the system no longer offers it. ngine
    /// records the default microphone instead until it is back.
    pub fn missing_microphone(&self) -> Option<&str> {
        let wanted = self.settings.inputs.microphone.as_deref()?;
        let listed = self.microphones.iter().any(|device| device.id == wanted);
        (!listed && !self.microphones.is_empty()).then_some(wanted)
    }

    pub fn missing_microphone_name(&self) -> Option<String> {
        let id = self.missing_microphone()?;
        Some(
            self.settings
                .inputs
                .microphone_name
                .clone()
                .unwrap_or_else(|| id.to_owned()),
        )
    }

    /// The chosen system sound when it can no longer be recorded, such as an
    /// app that has quit. ngine records all system sound instead.
    pub fn missing_system_sound(&self) -> Option<&SystemSound> {
        let wanted = &self.settings.inputs.system;
        let available = matches!(wanted, SystemSound::Everything)
            || self
                .system_sounds
                .iter()
                .any(|offered| same_sound(offered, wanted));
        (!available).then_some(wanted)
    }

    /// The inputs a capture starts with: the chosen ones, or the system's
    /// defaults where a chosen one is missing.
    pub fn inputs(&self) -> Inputs {
        let microphone = match self.missing_microphone() {
            Some(_) => None,
            None => self.settings.inputs.microphone.clone(),
        };
        let system = match self.missing_system_sound() {
            Some(_) => SystemSound::Everything,
            None => self
                .system_sounds
                .iter()
                .find(|offered| same_sound(offered, &self.settings.inputs.system))
                .cloned()
                .unwrap_or_default(),
        };
        Inputs {
            microphone,
            microphone_name: None,
            system,
        }
    }

    fn recording_request(&self, target: Target) -> RecordingRequest {
        RecordingRequest {
            target,
            video: self.settings.video,
            audio: self.settings.audio,
            inputs: self.inputs(),
            folder: self.settings.output_folder.clone(),
        }
    }

    fn replay_request(&self, target: Target) -> ReplayRequest {
        let replay = &self.settings.replay;
        ReplayRequest {
            target,
            video: self.settings.video,
            audio: self.settings.audio,
            inputs: self.inputs(),
            length: Duration::from_secs(u64::from(replay.length_seconds)),
            storage: replay.storage,
            memory_budget_mb: self.memory_budget_mb(),
            buffer_folder: replay.buffer_folder.clone(),
            clear_after_save: replay.clear_after_save,
            folder: self.settings.output_folder.clone(),
        }
    }

    pub fn open_sources(&mut self) {
        if self.capabilities.lists_targets {
            self.popup = Some(Popup::Sources);
            self.read_inventory();
        } else {
            self.pick();
        }
    }

    pub fn choose_source(&mut self, id: &str) {
        if let Some(index) = self.sources.iter().position(|source| source.id == id) {
            self.choose(index);
        } else {
            self.popup = None;
        }
    }

    pub fn choose(&mut self, index: usize) {
        if let Some(target) = self.sources.get(index) {
            self.target = Some(target.clone());
        }
        self.popup = None;
    }

    pub fn choose_first(&mut self, kind: TargetKind) {
        if let Some(index) = self.sources.iter().position(|source| source.kind == kind) {
            self.choose(index);
        }
    }

    pub fn pick(&mut self) {
        self.popup = None;
        self.engine.pick_target();
        self.picking = true;
    }

    pub fn open_popup(&mut self, popup: Popup) {
        self.popup = Some(popup);
    }

    pub fn close_popup(&mut self) {
        self.popup = None;
    }

    pub fn show_section(&mut self, section: Section) {
        self.capture = None;
        self.popup = None;
        self.section = section;
        self.open_settings();
    }

    pub fn open_settings(&mut self) {
        if !self.settings_open {
            self.read_inventory();
        }
        self.settings_open = true;
        self.renaming = None;
        self.popup = None;
        if let Some(editor) = &mut self.editor {
            editor.pause();
        }
    }

    pub fn close_settings(&mut self) {
        self.settings_open = false;
        self.capture = None;
        self.popup = None;
    }

    fn measure_free_space(&mut self) {
        self.free_space = self.engine.free_space(&self.settings.output_folder);
    }

    /// The settings a fresh install starts with.
    pub fn default_settings(&self) -> Settings {
        self.store
            .defaults()
            .cloned()
            .unwrap_or_else(|| self.settings.clone())
    }

    /// Puts one section of the settings back to what a fresh install has.
    pub fn reset_section(&mut self, section: Section) {
        let fresh = self.default_settings();
        self.popup = None;
        self.capture = None;
        let codecs = self.capabilities.codecs.clone();
        self.edit(|settings| match section {
            Section::Video => {
                settings.video = fresh.video;
                if !codecs.contains(&settings.video.codec)
                    && let Some(codec) = codecs.first()
                {
                    settings.video.codec = *codec;
                }
            }
            Section::Audio => settings.audio = fresh.audio,
            Section::Replay => {
                let buffer_folder = settings.replay.buffer_folder.clone();
                settings.replay = fresh.replay;
                settings.replay.buffer_folder = buffer_folder;
            }
            Section::Hotkeys => settings.hotkeys = fresh.hotkeys,
            Section::Storage => {
                settings.output_folder = fresh.output_folder;
                settings.replay.buffer_folder = fresh.replay.buffer_folder;
            }
            Section::General => settings.start_at_login = fresh.start_at_login,
            Section::About => {}
        });
    }

    /// Takes the answer of a folder dialog.
    pub fn folder_chosen(&mut self, folder: Folder, answer: Result<Option<PathBuf>, String>) {
        match answer {
            Ok(Some(chosen)) => {
                self.edit(|settings| match folder {
                    Folder::Output => settings.output_folder = chosen,
                    Folder::Buffer => settings.replay.buffer_folder = chosen,
                });
                if folder == Folder::Output {
                    self.problems.retain(|problem| {
                        !matches!(problem, Problem::DiskFull { .. } | Problem::WriteFailed)
                    });
                    self.refresh_library();
                }
            }
            Ok(None) => {}
            Err(error) => self.tell(format!("The folder dialog could not open: {error}"), true),
        }
    }

    /// Opens the system's privacy settings where `permission` is granted.
    pub fn open_privacy_settings(&mut self, permission: Permission) {
        if let Err(error) = crate::platform::open_privacy_settings(permission) {
            self.tell(
                format!(
                    "{} could not be opened: {error}",
                    crate::platform::SYSTEM_SETTINGS
                ),
                true,
            );
        }
    }

    /// Starts the recording again after a problem ended it.
    pub fn retry_capture(&mut self) {
        self.problems
            .retain(|problem| !matches!(problem, Problem::CaptureLost));
        if self.status.recording.is_none() {
            self.perform(Action::ToggleRecording);
        }
    }

    pub fn dismiss_problem(&mut self, index: usize) {
        if index < self.problems.len() {
            self.problems.remove(index);
        }
    }

    pub fn edit(&mut self, change: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        change(&mut self.settings);
        let replay = &mut self.settings.replay;
        replay.save_seconds = replay.save_seconds.min(replay.length_seconds);
        if before != self.settings {
            self.settings_changed(&before);
        }
    }

    fn settings_changed(&mut self, before: &Settings) {
        self.settings_dirty = true;
        self.forget_untyped_draft();
        if before.hotkeys != self.settings.hotkeys {
            self.hotkey_problems = match &mut self.shell {
                Some(shell) => shell.bind_hotkeys(&self.settings.hotkeys),
                None => hotkeys::problems(&self.settings.hotkeys),
            };
        }
        if before.start_at_login != self.settings.start_at_login
            && let Some(shell) = &self.shell
            && let Err(error) = shell.set_autostart(self.settings.start_at_login)
        {
            self.settings.start_at_login = before.start_at_login;
            self.tell(
                format!("Start at login could not be changed: {error}"),
                true,
            );
        }
        if before.output_folder != self.settings.output_folder {
            self.measure_free_space();
        }
        let capture_changed = before.video != self.settings.video
            || before.audio != self.settings.audio
            || before.inputs != self.settings.inputs
            || before.replay != self.settings.replay
            || before.output_folder != self.settings.output_folder;
        if capture_changed && let Some(replay) = self.status.replay.clone() {
            let request = self.replay_request(replay.target);
            if let Err(refusal) = self.engine.start_replay(&request) {
                self.tell(refusal.to_string(), true);
            }
        }
    }

    pub fn tell(&mut self, text: String, is_error: bool) {
        self.notice = Some(Notice {
            text,
            is_error,
            until: self.clock + NOTICE_SECONDS,
        });
    }

    fn announce(&self, notification: Notification) {
        if self.settings.announcements.desktop && self.shell.is_some() {
            platform::notify(notification);
        }
    }

    fn confirm(&mut self, text: String) {
        if self.settings.announcements.overlay
            && let Some(shell) = &self.shell
        {
            shell.show_toast(&text);
        }
        self.confirmation = Some(Confirmation {
            text,
            until: self.clock + TOAST_SECONDS,
        });
    }

    fn absorb(&mut self, event: Event) {
        match event {
            Event::TargetPicked(target) => {
                self.picking = false;
                self.target = Some(target);
                match self.after_pick.take() {
                    Some(AfterPick::Record) => {
                        let started = self.start_recording();
                        self.note(&started);
                    }
                    Some(AfterPick::Replay) => {
                        let started = self.set_replay(true);
                        self.note(&started);
                    }
                    None => {}
                }
            }
            Event::PickCancelled => {
                self.picking = false;
                self.after_pick = None;
            }
            Event::RecordingStarted => self
                .problems
                .retain(|problem| !matches!(problem, Problem::PermissionMissing(_))),
            Event::RecordingStopped { file, length } => {
                self.confirm(format!("Recording saved · {}", format::length(length)));
                self.announce(Notification::recording_saved(&file, length));
                self.refresh_library();
                self.measure_free_space();
            }
            Event::ReplaySaved { file, length } => {
                self.confirm(format!("Replay saved · {}", format::length(length)));
                self.announce(Notification::replay_saved(&file, length));
                self.refresh_library();
            }
            Event::Problem(problem) => self.raise(problem),
            Event::Inventory(inventory) => self.take_inventory(inventory),
            Event::ReplayStarted | Event::ReplayStopped | Event::MarkerAdded { .. } => {}
        }
    }

    fn raise(&mut self, problem: Problem) {
        if !self.problems.contains(&problem) {
            self.announce(Notification::problem(&problem));
            self.problems.push(problem);
        }
    }

    fn refresh(&mut self) -> bool {
        let events = self.engine.take_events();
        let mut changed = !events.is_empty();
        for event in events {
            self.absorb(event);
        }
        let status = self.engine.status();
        changed |= status != self.status;
        self.status = status;
        changed |= self.meters.follow(self.status.levels, self.clock);
        self.show_tray();
        changed
    }

    fn show_tray(&mut self) {
        if let Some(shell) = &mut self.shell {
            shell.show_tray(tray::Model::of(&self.status, &self.settings));
            shell.publish(BarState::of(&self.status));
        }
    }

    fn expire(&mut self) -> bool {
        let clock = self.clock;
        let notice_over = self.notice.as_ref().is_some_and(|n| clock >= n.until);
        if notice_over {
            self.notice = None;
        }
        let confirmation_over = self.confirmation.as_ref().is_some_and(|c| clock >= c.until);
        if confirmation_over {
            self.confirmation = None;
            if let Some(shell) = &self.shell {
                shell.hide_toast();
            }
        }
        notice_over || confirmation_over
    }

    pub fn save_settings(&mut self) {
        self.settings_dirty = false;
        self.save_due = None;
        if let Err(error) = self.store.save(&self.settings) {
            self.tell(format!("The settings could not be saved: {error}"), true);
        }
    }

    pub fn sync(&mut self, clock: f64) -> bool {
        self.clock = clock;
        let mut changed = self.refresh();
        changed |= self.follow_media();
        changed |= self.expire();
        changed |= self.settle_draft();
        if self.settings_dirty {
            self.settings_dirty = false;
            self.save_due = Some(clock + SAVE_DELAY_SECONDS);
        }
        if self.save_due.is_some_and(|due| clock >= due) {
            self.save_settings();
            changed = true;
        }
        changed
    }

    fn meters_move(&self) -> bool {
        self.screen == Screen::Capture && (self.status.levels.is_some() || self.meters.moving())
    }

    fn wants_ticks(&self) -> bool {
        self.status.recording.is_some()
            || self.status.replay.is_some()
            || self.picking
            || self.notice.is_some()
            || self.confirmation.is_some()
            || self.save_due.is_some()
            || self.draft_waits()
    }
}

pub fn poll(app: &mut Ngine, clock: f64, tx: &mut Tx) -> bool {
    let changed = app.sync(clock);
    if let Some(soon) = app.media_wants_ticks() {
        tx.wake_after(soon);
    } else if app.meters_move() {
        tx.wake_after(METER_TICK);
    } else if app.reading {
        tx.wake_after(READING_TICK);
    } else if app.wants_ticks() {
        tx.wake_after(TICK);
    }
    changed
}

pub fn on_key(app: &mut Ngine, event: &KeyEvent, tx: &mut Tx) -> bool {
    match event.key {
        Key::Command('w') => tx.close_window(),
        Key::Command('q') => {
            app.save_settings();
            tx.quit();
        }
        Key::Command(',') if app.settings_open => app.close_settings(),
        Key::Command(',') => app.open_settings(),
        Key::Command('1') => app.show(Screen::Capture),
        Key::Command('2') => app.show(Screen::Library),
        Key::Escape if app.capture.is_some() => app.cancel_hotkey(),
        Key::Escape if app.draft.is_some() => app.discard_draft(),
        Key::Escape if app.settings_open => app.close_settings(),
        _ if app.settings_open => return false,
        Key::Escape if app.renaming.is_some() => app.cancel_rename(),
        Key::Escape if app.watching() => toggle_watching(app, tx),
        Key::Text(ref key)
            if key.eq_ignore_ascii_case("f")
                && app.editor.is_some()
                && app.screen == Screen::Editor
                && tx.focused().is_none_or(|key| key != "clip-name") =>
        {
            toggle_watching(app, tx);
        }
        Key::Escape if app.screen == Screen::Editor => app.close_editor(),
        _ if app.screen == Screen::Editor && tx.focused().is_none_or(|key| key != "clip-name") => {
            return app.editor_key(event);
        }
        _ if app.screen == Screen::Library && tx.focused().is_none_or(|key| key != "rename") => {
            let focused = tx.focused().map(str::to_owned);
            let renaming = app.renaming.is_some();
            let handled = app.library_key(event, focused.as_deref());
            if !renaming && app.renaming.is_some() {
                tx.scroll_into_view(RENAME_BOX);
            }
            return handled;
        }
        _ => return false,
    }
    true
}

fn same_sound(offered: &SystemSound, wanted: &SystemSound) -> bool {
    match (offered, wanted) {
        (SystemSound::Everything, SystemSound::Everything) => true,
        (SystemSound::App { name: a, .. }, SystemSound::App { name: b, .. }) => a == b,
        (SystemSound::Output(a), SystemSound::Output(b)) => a.id == b.id,
        _ => false,
    }
}

/// Starts or ends watching the open video full screen, taking the window
/// full screen with it and giving it back after.
pub fn toggle_watching(app: &mut Ngine, tx: &mut Tx) {
    if app.watching() {
        if app.stop_watching() {
            tx.toggle_full_screen();
        }
    } else {
        app.watch_full_screen(true);
        tx.toggle_full_screen();
    }
}

pub fn on_close(app: &mut Ngine, _: &mut Tx) {
    app.save_settings();
}

#[cfg(test)]
mod tests;
