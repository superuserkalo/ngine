use std::path::{Path, PathBuf};
use std::time::Duration;

use rune::{Key, KeyEvent, TextEdit, Tx};

use crate::editor::Editor;
use crate::engine::fake::FakeEngine;
use crate::engine::{
    Capabilities, Engine, Event, Problem, RecordingRequest, ReplayRequest, ReplayStorage, Status,
    Target, TargetKind,
};
use crate::format;
use crate::hotkeys;
use crate::library::{self, Library};
use crate::settings::{Settings, Store};
use crate::shell::Shell;
use crate::tray;

mod actions;
pub mod demo;
mod media;

pub use actions::Action;

pub const NAME: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const WINDOW_SIZE: (f32, f32) = (780.0, 560.0);

const TICK: Duration = Duration::from_millis(250);
const NOTICE_SECONDS: f64 = 4.0;
const TOAST_SECONDS: f64 = 3.0;
const SAVE_DELAY_SECONDS: f64 = 0.5;
const BYTES_PER_MB: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Record,
    Library,
    Editor,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    Video,
    Audio,
    Replay,
    Hotkeys,
    General,
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
pub enum Field {
    OutputFolder,
    BufferFolder,
    Hotkey(usize),
}

pub struct Fields {
    pub output_folder: TextEdit,
    pub buffer_folder: TextEdit,
    pub hotkeys: [TextEdit; hotkeys::COUNT],
}

impl Fields {
    fn of(settings: &Settings) -> Self {
        Self {
            output_folder: TextEdit::new(settings.output_folder.to_string_lossy()),
            buffer_folder: TextEdit::new(settings.replay.buffer_folder.to_string_lossy()),
            hotkeys: settings.hotkeys.all().map(TextEdit::new),
        }
    }

    pub fn get_mut(&mut self, field: Field) -> &mut TextEdit {
        match field {
            Field::OutputFolder => &mut self.output_folder,
            Field::BufferFolder => &mut self.buffer_folder,
            Field::Hotkey(slot) => &mut self.hotkeys[slot % hotkeys::COUNT],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayCost {
    pub megabytes: u64,
    pub on_disk: bool,
}

pub struct Ngine {
    engine: Box<dyn Engine>,
    pub simulator: Option<FakeEngine>,
    pub capabilities: Capabilities,
    pub settings: Settings,
    store: Store,
    pub settings_were_reset: bool,
    pub screen: Screen,
    pub settings_tab: SettingsTab,
    pub target_kind: TargetKind,
    pub targets: Vec<Target>,
    pub target: Option<Target>,
    pub picking: bool,
    after_pick: Option<AfterPick>,
    pub status: Status,
    pub problems: Vec<Problem>,
    pub notice: Option<Notice>,
    pub confirmation: Option<Confirmation>,
    pub fields: Fields,
    pub hotkey_problems: [Option<String>; hotkeys::COUNT],
    pub library: Library,
    pub editor: Option<Editor>,
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
        mut engine: Box<dyn Engine>,
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
        let targets = engine.targets(TargetKind::Screen);
        let mut app = Self {
            engine,
            simulator,
            settings_were_reset: store.found_damaged(),
            store,
            screen: Screen::Record,
            settings_tab: SettingsTab::Video,
            target_kind: TargetKind::Screen,
            target: targets.first().cloned(),
            targets,
            picking: false,
            after_pick: None,
            status: Status::default(),
            problems: Vec::new(),
            notice: None,
            confirmation: None,
            fields: Fields::of(&settings),
            hotkey_problems: hotkeys::problems(&settings.hotkeys),
            library: Library::offline(),
            editor: None,
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
        if app.settings.replay.start_with_app && app.target.is_some() {
            let started = app.set_replay(true);
            app.note(&started);
        }
        app.refresh();
        app
    }

    pub fn attach(&mut self, mut shell: Shell) {
        self.hotkey_problems = shell.bind_hotkeys(&self.settings.hotkeys);
        self.shell = Some(shell);
        self.show_tray();
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

    pub fn replay_cost(&self) -> ReplayCost {
        let bytes = self.bytes_per_second() * u64::from(self.settings.replay.length_seconds);
        ReplayCost {
            megabytes: format::approximate_megabytes(bytes),
            on_disk: match self.settings.replay.storage {
                ReplayStorage::Memory => false,
                ReplayStorage::Disk => true,
                ReplayStorage::Auto => bytes > u64::from(self.memory_budget_mb()) * BYTES_PER_MB,
            },
        }
    }

    fn recording_request(&self, target: Target) -> RecordingRequest {
        RecordingRequest {
            target,
            video: self.settings.video,
            audio: self.settings.audio,
            folder: self.settings.output_folder.clone(),
        }
    }

    fn replay_request(&self, target: Target) -> ReplayRequest {
        let replay = &self.settings.replay;
        ReplayRequest {
            target,
            video: self.settings.video,
            audio: self.settings.audio,
            length: Duration::from_secs(u64::from(replay.length_seconds)),
            storage: replay.storage,
            memory_budget_mb: self.memory_budget_mb(),
            buffer_folder: replay.buffer_folder.clone(),
            clear_after_save: replay.clear_after_save,
            folder: self.settings.output_folder.clone(),
        }
    }

    pub fn show_kind(&mut self, kind: TargetKind) {
        self.target_kind = kind;
        self.targets = self.engine.targets(kind);
        if self
            .target
            .as_ref()
            .is_none_or(|target| target.kind != kind)
        {
            self.target = self.targets.first().cloned();
        }
    }

    pub fn choose(&mut self, index: usize) {
        if let Some(target) = self.targets.get(index) {
            self.target = Some(target.clone());
        }
    }

    pub fn pick(&mut self) {
        self.engine.pick_target();
        self.picking = true;
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
        let capture_changed = before.video != self.settings.video
            || before.audio != self.settings.audio
            || before.replay != self.settings.replay
            || before.output_folder != self.settings.output_folder;
        if capture_changed && let Some(replay) = self.status.replay.clone() {
            let request = self.replay_request(replay.target);
            if let Err(refusal) = self.engine.start_replay(&request) {
                self.tell(refusal.to_string(), true);
            }
        }
    }

    pub fn field_edited(&mut self, field: Field) {
        let text = self.fields.get_mut(field).text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.edit(|settings| match field {
            Field::OutputFolder => settings.output_folder = text.into(),
            Field::BufferFolder => settings.replay.buffer_folder = text.into(),
            Field::Hotkey(slot) => settings.hotkeys.set(slot, text),
        });
    }

    pub fn tell(&mut self, text: String, is_error: bool) {
        self.notice = Some(Notice {
            text,
            is_error,
            until: self.clock + NOTICE_SECONDS,
        });
    }

    fn confirm(&mut self, text: String) {
        if let Some(shell) = &self.shell {
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
            Event::RecordingStopped { length, .. } => {
                self.confirm(format!("Recording saved · {}", format::length(length)));
                self.refresh_library();
            }
            Event::ReplaySaved { length, .. } => {
                self.confirm(format!("Replay saved · {}", format::length(length)));
                self.refresh_library();
            }
            Event::Problem(problem) => self.raise(problem),
            Event::ReplayStarted | Event::ReplayStopped | Event::MarkerAdded { .. } => {}
        }
    }

    fn raise(&mut self, problem: Problem) {
        if !self.problems.contains(&problem) {
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
        self.show_tray();
        changed
    }

    fn show_tray(&mut self) {
        if let Some(shell) = &mut self.shell {
            shell.show_tray(tray::Model::of(&self.status, &self.settings));
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

    fn wants_ticks(&self) -> bool {
        self.status.recording.is_some()
            || self.status.replay.is_some()
            || self.picking
            || self.notice.is_some()
            || self.confirmation.is_some()
            || self.save_due.is_some()
    }
}

pub fn poll(app: &mut Ngine, clock: f64, tx: &mut Tx) -> bool {
    let changed = app.sync(clock);
    if let Some(soon) = app.media_wants_ticks() {
        tx.wake_after(soon);
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
        Key::Command(',') => app.show(Screen::Settings),
        Key::Escape if app.renaming.is_some() => app.cancel_rename(),
        _ if app.screen == Screen::Editor && tx.focused().is_none_or(|key| key != "rename") => {
            return app.editor_key(event);
        }
        _ => return false,
    }
    true
}

pub fn on_close(app: &mut Ngine, _: &mut Tx) {
    app.save_settings();
}

#[cfg(test)]
mod tests;
