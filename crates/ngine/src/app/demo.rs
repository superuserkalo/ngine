use std::path::PathBuf;
use std::time::Duration;

use ngine_recorder::Cut;

use super::{Action, Ngine, Screen, SettingsTab};
use crate::editor::{Editor, Exporting, Source};
use crate::engine::fake::{Clock, FakeEngine};
use crate::engine::{Permission, Problem, ReplayStorage, TargetKind};
use crate::library::Library;
use crate::platform;
use crate::settings::{Settings, Store};

pub const OUTPUT_FOLDER: &str = platform::SAMPLE_OUTPUT_FOLDER;
const BUFFER_FOLDER: &str = platform::SAMPLE_BUFFER_FOLDER;

pub fn settings() -> Settings {
    Settings::defaults(PathBuf::from(OUTPUT_FOLDER), PathBuf::from(BUFFER_FOLDER))
}

pub fn with(lists_targets: bool, store: Store) -> Ngine {
    let engine = FakeEngine::new(Clock::manual(), lists_targets);
    let mut store = store;
    let settings = store.load(settings());
    Ngine::new(Box::new(engine.clone()), Some(engine), settings, store)
}

pub fn idle() -> Ngine {
    with(true, Store::in_memory())
}

fn pass(app: &mut Ngine, time: Duration) {
    if let Some(simulator) = &app.simulator {
        simulator.advance(time);
    }
    app.sync(app.clock);
}

fn recording(app: &mut Ngine) {
    app.show_kind(TargetKind::Window);
    app.perform(Action::ToggleRecording);
    pass(app, Duration::from_secs(12));
    app.perform(Action::AddMarker);
    pass(app, Duration::from_secs(21));
    app.perform(Action::AddMarker);
    pass(app, Duration::from_secs(9));
    app.notice = None;
}

fn replay(app: &mut Ngine) {
    app.perform(Action::ToggleReplay);
    pass(app, Duration::from_secs(43));
    app.notice = None;
}

fn replay_saved(app: &mut Ngine) {
    app.perform(Action::ToggleReplay);
    pass(app, Duration::from_millis(100_800));
    app.perform(Action::SaveShort);
    pass(app, Duration::from_secs(1));
}

fn picker_only(app: &mut Ngine) {
    *app = with(false, Store::in_memory());
}

fn picker_waiting(app: &mut Ngine) {
    picker_only(app);
    app.pick();
}

fn picker_chosen(app: &mut Ngine) {
    picker_waiting(app);
    pass(app, Duration::from_secs(1));
}

fn picker_cancelled(app: &mut Ngine) {
    picker_waiting(app);
    if let Some(simulator) = &app.simulator {
        simulator.cancel_pick();
    }
    pass(app, Duration::from_secs(1));
}

fn capture_lost(app: &mut Ngine) {
    recording(app);
    app.perform(Action::ToggleRecording);
    if let Some(simulator) = &app.simulator {
        simulator.raise(Problem::CaptureLost);
    }
    pass(app, Duration::from_secs(1));
    app.notice = None;
}

fn problems(app: &mut Ngine) {
    recording(app);
    if let Some(simulator) = &app.simulator {
        simulator.raise(Problem::CaptureRestarted);
        simulator.raise(Problem::PermissionMissing(Permission::Microphone));
    }
    pass(app, Duration::from_secs(1));
}

fn permission_missing(app: &mut Ngine) {
    if let Some(simulator) = &app.simulator {
        simulator.block_starts_with(Some(Problem::PermissionMissing(
            Permission::ScreenRecording,
        )));
    }
    app.perform(Action::ToggleRecording);
}

fn disk_full(app: &mut Ngine) {
    recording(app);
    app.perform(Action::ToggleRecording);
    if let Some(simulator) = &app.simulator {
        simulator.raise(Problem::DiskFull {
            folder: PathBuf::from(OUTPUT_FOLDER),
        });
    }
    pass(app, Duration::from_secs(1));
    app.notice = None;
}

fn settings_tab(app: &mut Ngine, tab: SettingsTab) {
    app.screen = Screen::Settings;
    app.settings_tab = tab;
}

fn settings_replay_long(app: &mut Ngine) {
    settings_tab(app, SettingsTab::Replay);
    app.edit(|settings| {
        settings.replay.length_seconds = 1200;
        settings.replay.save_seconds = 120;
        settings.replay.storage = ReplayStorage::Auto;
    });
}

fn settings_hotkey_problem(app: &mut Ngine) {
    settings_tab(app, SettingsTab::Hotkeys);
    app.edit(|settings| {
        settings.hotkeys.marker = "m".into();
        settings.hotkeys.save_custom = settings.hotkeys.save_short.clone();
    });
    app.fields = super::Fields::of(&app.settings);
}

fn settings_reset(app: &mut Ngine) {
    app.settings_were_reset = true;
}

fn library(app: &mut Ngine) {
    app.library = Library::sample(&app.settings.output_folder);
    app.screen = Screen::Library;
}

fn editor(app: &mut Ngine) {
    library(app);
    let file = app.library.path(0).unwrap_or_default();
    app.editor = Some(Editor::sample(&file));
    app.screen = Screen::Editor;
}

fn in_editor(app: &mut Ngine, change: impl FnOnce(&mut Editor)) {
    if let Some(editor) = &mut app.editor {
        change(editor);
    }
}

pub type Scene = (&'static str, fn(&mut Ngine));

pub const SCENES: &[Scene] = &[
    ("idle", |_| {}),
    ("recording", recording),
    ("replay", replay),
    ("recording-and-replay", |app| {
        replay(app);
        recording(app);
    }),
    ("replay-saved", replay_saved),
    ("picker-only", picker_only),
    ("picker-waiting", picker_waiting),
    ("picker-chosen", picker_chosen),
    ("picker-cancelled", picker_cancelled),
    ("capture-lost", capture_lost),
    ("problems", problems),
    ("permission-missing", permission_missing),
    ("disk-full", disk_full),
    ("settings-reset", settings_reset),
    ("settings-video", |app| {
        settings_tab(app, SettingsTab::Video);
    }),
    ("settings-audio", |app| {
        settings_tab(app, SettingsTab::Audio);
    }),
    ("settings-replay", |app| {
        settings_tab(app, SettingsTab::Replay);
    }),
    ("settings-replay-long", settings_replay_long),
    ("settings-hotkeys", |app| {
        settings_tab(app, SettingsTab::Hotkeys);
    }),
    ("settings-hotkey-problem", settings_hotkey_problem),
    ("settings-general", |app| {
        settings_tab(app, SettingsTab::General);
    }),
    ("library", library),
    ("library-empty", |app| app.screen = Screen::Library),
    ("library-renaming", |app| {
        library(app);
        app.start_rename(3);
    }),
    ("editor", editor),
    ("editor-empty", |app| app.screen = Screen::Editor),
    ("editor-playing", |app| {
        editor(app);
        in_editor(app, |editor| editor.playing = true);
    }),
    ("editor-exact-muted", |app| {
        editor(app);
        in_editor(app, |editor| {
            editor.cut = Cut::Exact;
            editor.set_level(Source::Microphone, |level| level.on = false);
            editor.set_level(Source::System, |level| level.percent = 60);
        });
    }),
    ("editor-mix-only", |app| {
        editor(app);
        in_editor(app, |editor| editor.info.audio_tracks = 1);
    }),
    ("editor-exporting", |app| {
        editor(app);
        in_editor(app, |editor| {
            editor.exporting = Exporting::Running {
                done: 0.42,
                live: None,
            };
        });
    }),
    ("editor-exported", |app| {
        editor(app);
        let file = PathBuf::from(OUTPUT_FOLDER).join("Clip 2026-11-16 14.09.51.mp4");
        in_editor(app, |editor| {
            editor.exporting = Exporting::Done {
                file,
                length: Duration::from_secs(42),
            };
        });
    }),
    ("editor-export-failed", |app| {
        editor(app);
        in_editor(app, |editor| {
            editor.exporting = Exporting::Failed("The disk is full".into());
        });
    }),
];
