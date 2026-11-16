use std::time::Duration;

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode, Runtime};

use super::view;
use crate::app::{Ngine, Popup, Section, WINDOW_SIZE, demo};
use crate::engine::{Codec, ReplayStorage, Tracks};
use crate::hotkeys;
use crate::settings::Store;

const FRAME: f64 = 1.0 / 60.0;

fn runtime(app: Ngine) -> Runtime<Ngine> {
    let mut runtime = Runtime::new(app, view, WINDOW_SIZE, Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime
}

fn scene(name: &str) -> Runtime<Ngine> {
    let mut app = demo::idle();
    let (_, apply) = demo::SCENES
        .iter()
        .find(|(scene, _)| *scene == name)
        .unwrap_or_else(|| panic!("no scene called {name}"));
    apply(&mut app);
    runtime(app)
}

fn settings(section: Section) -> Runtime<Ngine> {
    let mut app = demo::idle();
    app.show_section(section);
    runtime(app)
}

fn element(runtime: &Runtime<Ngine>, key: &str) -> ElementInfo {
    runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no element with the key {key}"))
}

fn words(runtime: &Runtime<Ngine>, key: &str) -> String {
    let element = element(runtime, key);
    element.text.or(element.label).unwrap_or_default()
}

fn shows(runtime: &Runtime<Ngine>, key: &str) -> bool {
    runtime.find(key).is_some()
}

fn press(runtime: &mut Runtime<Ngine>, key: &str) {
    let (x, y) = runtime
        .find(key)
        .unwrap_or_else(|| panic!("nothing to press with the key {key}"));
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(FRAME);
    runtime.settle_all();
}

fn pass(runtime: &mut Runtime<Ngine>, time: Duration) {
    runtime.update(|app, _| {
        if let Some(simulator) = &app.simulator {
            simulator.advance(time);
        }
        app.sync(0.0);
    });
    runtime.frame(FRAME);
    runtime.settle_all();
}

fn type_text(runtime: &mut Runtime<Ngine>, text: &str) {
    runtime.key(KeyEvent {
        key: Key::Text(text.into()),
        mods: Mods::default(),
    });
    runtime.frame(FRAME);
    runtime.settle_all();
}

fn dimmed(runtime: &Runtime<Ngine>, key: &str) -> bool {
    element(runtime, key).opacity < 1.0
}

fn help(runtime: &Runtime<Ngine>, key: &str) -> String {
    element(runtime, key).help.unwrap_or_default()
}

fn physical(runtime: &mut Runtime<Ngine>, key: PhysicalKeyCode, mods: Mods) {
    runtime.physical_input(PhysicalInput::Key {
        key,
        mods,
        pressed: true,
        repeat: false,
    });
    runtime.frame(FRAME);
    runtime.settle_all();
}

fn ctrl_alt() -> Mods {
    Mods {
        ctrl: true,
        alt: true,
        ..Mods::default()
    }
}

#[test]
fn every_scene_draws_a_window() {
    for (name, _) in demo::SCENES {
        assert!(shows(&scene(name), "window"), "{name}");
    }
}

#[test]
fn the_sidebar_has_capture_library_and_settings_and_a_live_status() {
    let mut runtime = scene("idle");
    for key in ["nav-capture", "nav-library", "nav-settings", "status-idle"] {
        assert!(shows(&runtime, key), "{key}");
    }
    assert!(!shows(&runtime, "nav-editor"));
    assert!(!shows(&runtime, "quit"));
    press(&mut runtime, "nav-library");
    assert_eq!(words(&runtime, "library-title"), "Library");
    press(&mut runtime, "nav-settings");
    assert!(shows(&runtime, "sections"));
    press(&mut runtime, "status");
    assert!(shows(&runtime, "capture"));
}

#[test]
fn idle_capture_shows_the_source_a_record_button_and_why_saving_waits() {
    let runtime = scene("idle");
    assert_eq!(words(&runtime, "source-name"), "Built-in display");
    assert_eq!(words(&runtime, "record-toggle"), "Record");
    assert_eq!(words(&runtime, "elapsed"), "00:00");
    assert_eq!(
        words(&runtime, "recording-state"),
        "Ready to record Built-in display"
    );
    assert_eq!(
        words(&runtime, "replay-state"),
        "Off. When on, keeps the last 1 min ready to save, ≈ 90 MB in memory."
    );
    assert!(!shows(&runtime, "marker"));
    assert!(dimmed(&runtime, "save-short"));
    assert_eq!(help(&runtime, "save-short"), super::capture::NO_REPLAY);
    assert_eq!(
        words(&runtime, "save-blocked"),
        "Turn instant replay on first."
    );
    assert_eq!(words(&runtime, "record-toggle-keys"), caps("ctrl+alt+r"));
    assert!(!shows(&runtime, "banners"));
}

#[test]
fn the_source_menu_groups_screens_windows_and_apps_and_a_press_chooses() {
    let mut runtime = scene("idle");
    press(&mut runtime, "source");
    for key in [
        "sources-screens",
        "sources-windows",
        "sources-apps",
        "source-picker",
    ] {
        assert!(shows(&runtime, key), "{key}");
    }
    press(&mut runtime, "source-window-2");
    assert!(!shows(&runtime, "sources"));
    assert_eq!(words(&runtime, "source-name"), "Discord");
    assert_eq!(
        words(&runtime, "recording-state"),
        "Ready to record Discord"
    );
}

#[test]
fn recording_turns_record_into_stop_with_a_marker_button_and_locks_the_source() {
    let mut runtime = scene("idle");
    press(&mut runtime, "record-toggle");
    pass(&mut runtime, Duration::from_secs(42));
    assert_eq!(words(&runtime, "record-toggle"), "Stop");
    assert_eq!(words(&runtime, "elapsed"), "00:42");
    assert_eq!(words(&runtime, "status-recording-text"), "Recording 00:42");
    assert_eq!(help(&runtime, "source"), super::capture::STOP_TO_CHANGE);

    press(&mut runtime, "marker");
    assert_eq!(
        words(&runtime, "recording-state"),
        "Recording Built-in display · 1 marker"
    );
    press(&mut runtime, "record-toggle");
    assert_eq!(words(&runtime, "record-toggle"), "Record");
    assert_eq!(words(&runtime, "notice"), "Recording saved · 42 s");
    assert!(shows(&runtime, "status-idle"));
}

#[test]
fn replay_shows_what_it_holds_and_a_save_confirms_the_real_length() {
    let mut runtime = scene("idle");
    press(&mut runtime, "replay-toggle");
    pass(&mut runtime, Duration::from_millis(43_400));
    assert_eq!(words(&runtime, "status-replay-text"), "Replay on · 1 min");
    assert!(words(&runtime, "replay-state").starts_with("Holding 43 s of 1 min · "));
    assert!(words(&runtime, "replay-state").ends_with(" in memory"));
    assert!(!dimmed(&runtime, "save-short"));
    assert!(!shows(&runtime, "save-blocked"));

    press(&mut runtime, "save-short");
    pass(&mut runtime, Duration::from_secs(1));
    assert_eq!(words(&runtime, "notice"), "Replay saved · 30 s");
    press(&mut runtime, "replay-toggle");
    assert!(dimmed(&runtime, "save-everything"));
}

#[test]
fn the_replay_length_menu_puts_the_cost_beside_each_length() {
    let mut runtime = scene("replay-length-open");
    assert_eq!(words(&runtime, "replay-length.3-label"), "1 min");
    assert_eq!(
        words(&runtime, "replay-length.3-detail"),
        "≈ 90 MB in memory"
    );
    assert_eq!(
        words(&runtime, "replay-length.10-detail"),
        "≈ 1.8 GB on disk"
    );
    press(&mut runtime, "replay-length.5");
    assert_eq!(runtime.state.settings.replay.length_seconds, 120);
    assert!(!shows(&runtime, "replay-length-choices"));
}

#[test]
fn where_sources_cannot_be_listed_the_system_picker_chooses() {
    let mut runtime = scene("picker-only");
    assert_eq!(words(&runtime, "source-name"), "Nothing chosen yet");
    press(&mut runtime, "source");
    assert!(!shows(&runtime, "sources"));
    assert_eq!(
        words(&runtime, "source-name"),
        "Waiting for the system picker…"
    );
    pass(&mut runtime, Duration::from_secs(1));
    assert_eq!(words(&runtime, "source-name"), "Counter-Strike 2");
    let cancelled = scene("picker-cancelled");
    assert_eq!(words(&cancelled, "source-name"), "Nothing chosen yet");
}

#[test]
fn problems_show_one_banner_at_a_time_with_the_count_of_the_rest() {
    let mut runtime = scene("problems");
    assert_eq!(
        words(&runtime, "problem-capture-restarted-title"),
        "Capture stopped for a moment and was restarted"
    );
    assert!(
        words(&runtime, "problem-capture-restarted-detail").ends_with("1 more after this one.")
    );
    assert!(!shows(&runtime, "problem-permission-microphone"));
    press(&mut runtime, "problem-capture-restarted-dismiss");
    assert!(shows(&runtime, "problem-permission-microphone"));
}

#[test]
fn each_banner_offers_the_action_that_fixes_it() {
    let full = scene("disk-full");
    assert_eq!(words(&full, "problem-disk-full-title"), "The disk is full");
    assert_eq!(
        words(&full, "problem-disk-full-action"),
        "Choose another folder"
    );
    let missing = scene("permission-missing");
    assert!(words(&missing, "problem-permission-screen-detail").contains("Screen Recording"));
    assert_eq!(words(&missing, "record-toggle"), "Record");

    let mut lost = scene("capture-lost");
    assert_eq!(
        words(&lost, "problem-capture-lost-detail"),
        "What was recorded up to then is saved."
    );
    press(&mut lost, "problem-capture-lost-action");
    assert!(!shows(&lost, "problem-capture-lost"));
    assert_eq!(words(&lost, "record-toggle"), "Stop");
}

#[test]
fn the_settings_sections_switch_what_is_shown() {
    let mut runtime = settings(Section::Video);
    for (id, shown) in [
        ("audio", "settings-audio"),
        ("replay", "settings-replay"),
        ("hotkeys", "settings-hotkeys"),
        ("storage", "settings-storage"),
        ("general", "settings-general"),
        ("about", "settings-about"),
        ("video", "settings-video"),
    ] {
        press(&mut runtime, &format!("section-{id}"));
        assert!(shows(&runtime, shown), "{shown}");
    }
}

#[test]
fn the_codec_menu_lists_only_what_this_computer_encodes() {
    let mut runtime = settings(Section::Video);
    press(&mut runtime, "codec");
    assert!(shows(&runtime, "codec.0"));
    assert!(shows(&runtime, "codec.1"));
    assert!(!shows(&runtime, "codec.2"));
    press(&mut runtime, "codec.1");
    assert_eq!(runtime.state.settings.video.codec, Codec::Hevc);
    press(&mut runtime, "frame-rate");
    press(&mut runtime, "frame-rate.2");
    assert_eq!(runtime.state.settings.video.frame_rate, 120);
    assert_eq!(
        words(&runtime, "quality-row-hint"),
        "About 110 MB for each minute"
    );
}

#[test]
fn the_audio_settings_switch_sources_and_tracks() {
    let mut runtime = settings(Section::Audio);
    assert_eq!(words(&runtime, "system-volume-value"), "100 %");
    press(&mut runtime, "microphone");
    assert!(!runtime.state.settings.audio.microphone.enabled);
    press(&mut runtime, "tracks");
    press(&mut runtime, "tracks.1");
    assert_eq!(runtime.state.settings.audio.tracks, Tracks::MixOnly);
    press(&mut runtime, "app-only");
    assert!(!runtime.state.settings.audio.app_only);
}

#[test]
fn a_slider_moves_to_the_stop_under_the_pointer() {
    let mut runtime = settings(Section::Audio);
    let track = element(&runtime, "system-volume").rect;
    let (x, y) = (track[0] + track[2] / 2.0, track[1] + track[3] / 2.0);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(FRAME);
    runtime.settle_all();
    assert_eq!(runtime.state.settings.audio.system.volume_percent, 50);
    assert_eq!(words(&runtime, "system-volume-value"), "50 %");
}

#[test]
fn the_replay_settings_say_what_the_buffer_costs_and_where_it_is_kept() {
    let mut runtime = settings(Section::Replay);
    assert_eq!(
        words(&runtime, "replay-length-row-hint"),
        "≈ 90 MB in memory"
    );
    press(&mut runtime, "storage");
    press(&mut runtime, "storage.2");
    assert_eq!(runtime.state.settings.replay.storage, ReplayStorage::Disk);
    assert_eq!(words(&runtime, "replay-length-row-hint"), "≈ 90 MB on disk");
    let long = scene("settings-replay-long");
    assert_eq!(words(&long, "replay-length-row-hint"), "≈ 1.8 GB on disk");
    assert_eq!(words(&long, "save-length-current"), "2 min");
}

#[test]
fn a_shorter_buffer_shortens_the_custom_save_length() {
    let mut runtime = settings(Section::Replay);
    runtime.update(|app, _| app.edit(|settings| settings.replay.length_seconds = 30));
    runtime.frame(FRAME);
    runtime.settle_all();
    assert_eq!(runtime.state.settings.replay.save_seconds, 30);
    assert_eq!(words(&runtime, "save-length-current"), "30 s");
}

#[test]
fn the_hotkey_recorder_takes_the_pressed_keys() {
    let mut runtime = settings(Section::Hotkeys);
    assert_eq!(words(&runtime, "hotkey-4-caps"), caps("ctrl+alt+m"));
    press(&mut runtime, "hotkey-4");
    assert!(runtime.state.is_capturing(hotkeys::MARKER));
    assert_eq!(
        words(&runtime, "hotkey-4-waiting"),
        super::settings::WAITING_FOR_KEYS
    );
    assert_eq!(
        words(&runtime, "hotkey-4-row-hint"),
        super::settings::RECORDER_HINT
    );
    let ctrl_shift = Mods {
        ctrl: true,
        shift: true,
        ..Mods::default()
    };
    physical(&mut runtime, PhysicalKeyCode::F9, ctrl_shift);
    assert!(!runtime.state.is_capturing(hotkeys::MARKER));
    assert_eq!(runtime.state.settings.hotkeys.marker, "ctrl+shift+f9");
}

#[test]
fn escape_cancels_the_hotkey_recorder_and_keeps_the_hotkey() {
    let mut runtime = settings(Section::Hotkeys);
    press(&mut runtime, "hotkey-4");
    physical(&mut runtime, PhysicalKeyCode::Escape, Mods::default());
    assert!(runtime.state.capture.is_none());
    assert_eq!(runtime.state.settings.hotkeys.marker, "ctrl+alt+m");
    assert!(shows(&runtime, "hotkey-4-caps"));
}

#[test]
fn a_hotkey_another_action_has_is_refused_with_its_name() {
    let mut runtime = settings(Section::Hotkeys);
    press(&mut runtime, "hotkey-4");
    physical(&mut runtime, PhysicalKeyCode::KeyR, ctrl_alt());
    assert_eq!(runtime.state.settings.hotkeys.marker, "ctrl+alt+m");
    assert_eq!(
        words(&runtime, "hotkey-4-row-hint"),
        format!(
            "{} is already used for “Start or stop recording”",
            caps("ctrl+alt+r")
        )
    );
    physical(&mut runtime, PhysicalKeyCode::KeyM, Mods::default());
    assert_eq!(
        words(&runtime, "hotkey-4-row-hint"),
        hotkeys::NEEDS_MODIFIER
    );
}

#[test]
fn a_hotkey_can_be_cleared_and_then_says_it_is_not_set() {
    let mut runtime = settings(Section::Hotkeys);
    press(&mut runtime, "hotkey-4-clear");
    assert_eq!(runtime.state.settings.hotkeys.marker, "");
    assert_eq!(words(&runtime, "hotkey-4-unset"), super::settings::NOT_SET);
    assert!(dimmed(&runtime, "hotkey-4-clear"));
    assert_eq!(help(&runtime, "hotkey-4-clear"), "No hotkey is set");
    let problem = scene("settings-hotkey-problem");
    assert_eq!(words(&problem, "hotkey-1-row-hint"), hotkeys::USED_TWICE);
    assert_eq!(words(&problem, "hotkey-4-row-hint"), hotkeys::UNUSABLE);
}

#[test]
fn storage_shows_both_folders_and_the_free_space() {
    let runtime = settings(Section::Storage);
    assert_eq!(
        words(&runtime, "output-folder-hint"),
        crate::app::demo::OUTPUT_FOLDER
    );
    for key in [
        "output-folder-change",
        "output-folder-open",
        "buffer-folder-change",
        "free-space-value",
    ] {
        assert!(shows(&runtime, key), "{key}");
    }
}

#[test]
fn a_section_is_reset_after_asking() {
    let mut runtime = settings(Section::Video);
    runtime.update(|app, _| app.edit(|settings| settings.video.frame_rate = 120));
    press(&mut runtime, "section-reset");
    assert_eq!(
        runtime.state.popup,
        Some(Popup::ConfirmReset(Section::Video))
    );
    press(&mut runtime, "reset-confirm");
    assert_eq!(runtime.state.settings.video, demo::settings().video);
    assert!(runtime.state.popup.is_none());
}

#[test]
fn the_general_and_about_sections_hold_start_at_login_and_the_build() {
    let mut runtime = settings(Section::General);
    press(&mut runtime, "start-at-login");
    assert!(runtime.state.settings.start_at_login);
    let about = settings(Section::About);
    assert_eq!(words(&about, "about-note"), super::EARLY_NOTE);
    assert!(!shows(&about, "section-reset"));
}

#[test]
fn settings_from_a_damaged_file_are_the_defaults_with_a_banner_saying_so() {
    let folder = std::env::temp_dir().join(format!("ngine-ui-damaged-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("a temp folder");
    let file = folder.join("settings.json");
    std::fs::write(&file, b"{ \"replay.length_seconds\": 3").expect("a damaged file");

    let mut runtime = runtime(demo::with(true, Store::open(&file)));
    assert_eq!(runtime.state.settings, demo::settings());
    assert_eq!(
        words(&runtime, "banner-settings-reset-title"),
        "The settings file was damaged"
    );
    press(&mut runtime, "banner-settings-reset-action");
    assert!(shows(&runtime, "sections"));
    assert!(!shows(&runtime, "banner-settings-reset"));
    std::fs::remove_dir_all(&folder).expect("temp folder removed");
}

#[test]
fn the_library_filter_search_and_order_change_the_cards_shown() {
    let mut runtime = scene("library");
    assert_eq!(words(&runtime, "library-total"), "5 videos · 2.7 GB");
    press(&mut runtime, "library-filter.replays");
    assert!(shows(&runtime, "item-1"));
    assert!(!shows(&runtime, "item-0"));
    press(&mut runtime, "library-filter.all");
    press(&mut runtime, "library-search");
    type_text(&mut runtime, "boss");
    assert!(shows(&runtime, "item-3"));
    assert!(!shows(&runtime, "item-0"));
    type_text(&mut runtime, "zzz");
    assert_eq!(
        words(&runtime, "library-none-found-title"),
        "Nothing matches"
    );
    press(&mut runtime, "library-show-all");
    assert!(shows(&runtime, "item-0"));
    press(&mut runtime, "library-sort");
    press(&mut runtime, "library-sort.2");
    assert_eq!(runtime.state.browse.sort, crate::app::browse::Sort::Largest);
}

#[test]
fn a_card_has_its_actions_in_a_menu_and_delete_asks_before_trashing() {
    let mut runtime = scene("library");
    runtime.update(|app, _| app.trash = |_| Ok(()));
    press(&mut runtime, "item-1-more");
    for key in ["menu-open", "menu-rename", "menu-show", "menu-trash"] {
        assert!(shows(&runtime, key), "{key}");
    }
    press(&mut runtime, "menu-trash");
    assert!(shows(&runtime, "trash-ask"));
    press(&mut runtime, "trash-confirm");
    assert_eq!(runtime.state.library.entries.len(), 4);
    assert!(words(&runtime, "notice").starts_with("1 video moved to the"));
}

#[test]
fn selected_cards_get_a_bar_that_trashes_them_together() {
    let mut runtime = scene("library-selected");
    assert_eq!(words(&runtime, "selection-count"), "2 selected");
    press(&mut runtime, "selection-trash");
    assert!(words(&runtime, "trash-ask-title").starts_with("Move 2 videos to the"));
    press(&mut runtime, "trash-cancel");
    assert!(!shows(&runtime, "trash-ask"));
    press(&mut runtime, "selection-clear");
    assert!(!shows(&runtime, "selection-bar"));
}

#[test]
fn the_editor_goes_back_to_the_library_and_shows_its_keys() {
    let mut runtime = scene("editor");
    press(&mut runtime, "editor-keys-button");
    assert!(shows(&runtime, "editor-keys"));
    assert_eq!(words(&runtime, "shortcut-0-words"), "Play or pause");
    runtime.update(|app, _| app.close_popup());
    runtime.frame(FRAME);
    press(&mut runtime, "editor-back");
    assert!(shows(&runtime, "library"));
    assert!(runtime.state.editor.is_none());
}

#[test]
fn the_capture_screen_shows_the_newest_recordings() {
    let mut runtime = scene("idle-with-recent");
    assert_eq!(
        words(&runtime, "recent-0-name"),
        "Recording 2026-11-16 14.03.22"
    );
    assert!(shows(&runtime, "recent-2"));
    assert!(!shows(&runtime, "recent-3"));
    press(&mut runtime, "recent-all");
    assert!(shows(&runtime, "library"));
}

#[test]
fn every_screen_fits_a_quarter_tile_and_a_narrow_one() {
    for size in [(960.0, 540.0), (720.0, 480.0), (600.0, 900.0)] {
        for name in ["idle", "library", "editor", "settings-hotkeys"] {
            let mut app = demo::idle();
            let (_, apply) = demo::SCENES
                .iter()
                .find(|(scene, _)| *scene == name)
                .expect("the scene");
            apply(&mut app);
            let mut runtime = Runtime::new(app, view, size, Env::new(true, 2.0));
            runtime.frame(0.0);
            let window = element(&runtime, "window").rect;
            let main = element(&runtime, "main").rect;
            assert!(
                main[0] + main[2] <= window[2] + 0.5,
                "{name} at {size:?} runs past the window"
            );
        }
    }
}

#[test]
fn escape_cancels_the_hotkey_recorder_even_when_it_arrives_as_a_plain_key() {
    let mut runtime = runtime({
        let mut app = demo::idle();
        app.show_section(Section::Hotkeys);
        app.begin_hotkey(hotkeys::MARKER);
        app
    });
    let escape = KeyEvent {
        key: Key::Escape,
        mods: Mods::default(),
    };
    runtime.update(|app, tx| {
        crate::app::on_key(app, &escape, tx);
    });
    runtime.frame(FRAME);
    runtime.settle_all();
    assert!(runtime.state.capture.is_none());
    assert!(shows(&runtime, "hotkey-4-caps"));
}

fn caps(accelerator: &str) -> String {
    hotkeys::keycaps(accelerator)
        .expect("the test accelerator parses")
        .join(" ")
}
