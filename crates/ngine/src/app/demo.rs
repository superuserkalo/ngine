use std::path::PathBuf;
use std::time::Duration;

use ngine_recorder::Cut;
use rune::TextEdit;

use super::browse::Filter;
use super::{Action, Ngine, Popup, Screen, Section};
use crate::editor::{Editor, Exporting, Source};
use crate::engine::fake::{Clock, FakeEngine};
use crate::engine::{Permission, Problem, ReplayStorage, TargetKind};
use crate::hotkeys;
use crate::library::Library;
use crate::platform;
use crate::settings::{Settings, Store};

pub const OUTPUT_FOLDER: &str = platform::SAMPLE_OUTPUT_FOLDER;
const BUFFER_FOLDER: &str = platform::SAMPLE_BUFFER_FOLDER;
const LONG_NAME: &str =
    "Ranked match on the new map with the whole squad, overtime and the comeback nobody expected";

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
    app.choose_first(TargetKind::Window);
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

fn section(app: &mut Ngine, section: Section) {
    app.show_section(section);
}

fn settings_replay_long(app: &mut Ngine) {
    section(app, Section::Replay);
    app.edit(|settings| {
        settings.replay.length_seconds = 1200;
        settings.replay.save_seconds = 120;
        settings.replay.storage = ReplayStorage::Auto;
    });
}

fn settings_hotkey_problem(app: &mut Ngine) {
    section(app, Section::Hotkeys);
    app.edit(|settings| {
        settings.hotkeys.marker = "m".into();
        settings.hotkeys.save_custom = settings.hotkeys.save_short.clone();
    });
}

fn settings_hotkey_conflict(app: &mut Ngine) {
    section(app, Section::Hotkeys);
    app.begin_hotkey(hotkeys::MARKER);
    if let Some(taken) = hotkeys::parse(&app.settings.hotkeys.record) {
        app.propose_hotkey(hotkeys::MARKER, &taken);
    }
}

fn settings_reset(app: &mut Ngine) {
    app.settings_were_reset = true;
}

fn library(app: &mut Ngine) {
    app.library = Library::sample(&app.settings.output_folder);
    app.screen = Screen::Library;
}

fn library_long_names(app: &mut Ngine) {
    library(app);
    if let Some(entry) = app.library.entries.get_mut(1) {
        LONG_NAME.clone_into(&mut entry.item.name);
        entry.item.info = None;
    }
    if let Some(entry) = app.library.entries.get_mut(3) {
        entry.item.name = format!("Clip {LONG_NAME}");
        entry.item.unfinished = true;
    }
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
    ("idle-with-recent", |app| {
        app.library = Library::sample(&app.settings.output_folder);
    }),
    ("recording", recording),
    ("replay", replay),
    ("recording-and-replay", |app| {
        replay(app);
        recording(app);
    }),
    ("replay-saved", replay_saved),
    ("sources-open", Ngine::open_sources),
    ("capture-microphone-open", |app| {
        app.open_popup(Popup::Choices("capture-microphone".into()));
    }),
    ("replay-length-open", |app| {
        app.open_popup(Popup::Choices("replay-length".into()));
    }),
    ("picker-only", picker_only),
    ("picker-waiting", picker_waiting),
    ("picker-chosen", picker_chosen),
    ("picker-cancelled", picker_cancelled),
    ("capture-lost", capture_lost),
    ("problems", problems),
    ("permission-missing", permission_missing),
    ("disk-full", disk_full),
    ("settings-reset", settings_reset),
    ("settings-video", |app| section(app, Section::Video)),
    ("settings-frame-rate-open", |app| {
        section(app, Section::Video);
        app.open_popup(Popup::Choices("settings-frame-rate-presets".into()));
    }),
    ("settings-audio", |app| section(app, Section::Audio)),
    ("settings-replay", |app| section(app, Section::Replay)),
    ("settings-replay-long", settings_replay_long),
    ("settings-hotkeys", |app| section(app, Section::Hotkeys)),
    ("settings-hotkey-recording", |app| {
        section(app, Section::Hotkeys);
        app.begin_hotkey(hotkeys::SAVE_SHORT);
    }),
    ("settings-hotkey-held", |app| {
        section(app, Section::Hotkeys);
        app.begin_hotkey(hotkeys::SAVE_SHORT);
        if let Some(capture) = &mut app.capture {
            capture.held = rune::Mods {
                ctrl: true,
                shift: true,
                ..rune::Mods::default()
            };
        }
    }),
    ("settings-hotkey-conflict", settings_hotkey_conflict),
    ("settings-microphone-open", |app| {
        section(app, Section::Audio);
        app.open_popup(Popup::Choices("settings-microphone".into()));
    }),
    ("settings-microphone-missing", |app| {
        app.edit(|settings| {
            settings.inputs.microphone = Some("unplugged".into());
            settings.inputs.microphone_name = Some("Studio microphone".into());
        });
        section(app, Section::Audio);
    }),
    ("settings-frame-rate-typed-wrong", |app| {
        section(app, Section::Video);
        app.draft_of(crate::app::Amount::FrameRate).text = "500".into();
        app.drafted();
    }),
    ("settings-budget-presets", |app| {
        section(app, Section::Replay);
        app.open_popup(Popup::Choices("settings-memory-budget-presets".into()));
    }),
    ("settings-hotkey-problem", settings_hotkey_problem),
    ("settings-storage", |app| section(app, Section::Storage)),
    ("settings-general", |app| section(app, Section::General)),
    ("settings-about", |app| section(app, Section::About)),
    ("settings-reset-ask", |app| {
        section(app, Section::Video);
        app.open_popup(Popup::ConfirmReset(Section::Video));
    }),
    ("library", library),
    ("library-empty", |app| app.screen = Screen::Library),
    ("library-replays", |app| {
        library(app);
        app.set_filter(Filter::Replays);
    }),
    ("library-no-results", |app| {
        library(app);
        app.browse.search = TextEdit::new("speedrun");
    }),
    ("library-selected", |app| {
        library(app);
        app.toggle_selected(1);
        app.toggle_selected(3);
    }),
    ("library-menu", |app| {
        library(app);
        app.open_item_menu(1, None);
    }),
    ("library-trash-ask", |app| {
        library(app);
        app.toggle_selected(1);
        app.toggle_selected(3);
        app.ask_to_trash_selection();
    }),
    ("library-renaming", |app| {
        library(app);
        app.start_rename(1);
    }),
    ("library-long-names", library_long_names),
    ("editor", editor),
    ("editor-empty", |app| app.screen = Screen::Editor),
    ("editor-playing", |app| {
        editor(app);
        in_editor(app, |editor| editor.playing = true);
    }),
    ("editor-hover", |app| {
        editor(app);
        in_editor(app, |editor| editor.hovered = true);
    }),
    ("editor-full-screen", |app| {
        editor(app);
        app.watch_full_screen(false);
    }),
    ("editor-full-screen-resting", |app| {
        editor(app);
        app.watch_full_screen(false);
        in_editor(app, |editor| {
            editor.playing = true;
            if let Some(watching) = &mut editor.watching {
                watching.moved_at = -crate::editor::CONTROLS_LINGER_SECONDS * 4.0;
            }
        });
    }),
    ("editor-keys", |app| {
        editor(app);
        app.open_popup(Popup::EditorKeys);
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
