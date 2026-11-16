use std::path::PathBuf;
use std::time::Duration;

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};

use super::view;
use crate::app::{Ngine, Screen, SettingsTab, WINDOW_SIZE, demo};
use crate::engine::{Codec, ReplayStorage};
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

fn settings(tab: SettingsTab) -> Runtime<Ngine> {
    let mut app = demo::idle();
    app.screen = Screen::Settings;
    app.settings_tab = tab;
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

#[test]
fn every_scene_draws_a_window() {
    for (name, _) in demo::SCENES {
        assert!(shows(&scene(name), "window"), "{name}");
    }
}

#[test]
fn idle_shows_the_targets_a_start_button_and_nothing_running() {
    let runtime = scene("idle");
    for key in [
        "nav-record",
        "nav-library",
        "nav-editor",
        "nav-settings",
        "target-kind",
        "target-screen-1",
        "target-screen-2",
        "pick",
        "status-idle",
        "preview-note",
    ] {
        assert!(shows(&runtime, key), "{key}");
    }
    assert_eq!(words(&runtime, "record-toggle"), "Start recording");
    assert_eq!(words(&runtime, "elapsed"), "00:00");
    assert_eq!(
        words(&runtime, "recording-state"),
        "Ready to record Built-in display"
    );
    assert_eq!(
        words(&runtime, "replay-state"),
        "Off. When on, 1 min ≈ 90 MB in memory."
    );
    assert!(dimmed(&runtime, "marker"));
    assert!(dimmed(&runtime, "save-short"));
    assert!(!shows(&runtime, "status-recording"));
    assert!(!shows(&runtime, "banners"));
}

#[test]
fn choosing_a_kind_lists_its_targets_and_a_press_chooses_one() {
    let mut runtime = scene("idle");
    press(&mut runtime, "target-kind.window");
    assert!(shows(&runtime, "target-window-1"));
    assert!(!shows(&runtime, "target-screen-1"));
    press(&mut runtime, "target-window-2");
    assert_eq!(
        words(&runtime, "recording-state"),
        "Ready to record Discord"
    );
}

#[test]
fn starting_shows_the_elapsed_time_and_stopping_confirms_the_length() {
    let mut runtime = scene("idle");
    press(&mut runtime, "record-toggle");
    pass(&mut runtime, Duration::from_secs(42));
    assert_eq!(words(&runtime, "record-toggle"), "Stop recording");
    assert_eq!(words(&runtime, "elapsed"), "00:42");
    assert_eq!(words(&runtime, "status-recording"), "Recording 00:42");
    assert!(!shows(&runtime, "status-idle"));
    assert!(!dimmed(&runtime, "marker"));

    press(&mut runtime, "marker");
    assert_eq!(
        words(&runtime, "recording-state"),
        "Recording Built-in display  ·  1 marker"
    );
    assert_eq!(words(&runtime, "notice"), "Marker added at 00:42");

    press(&mut runtime, "record-toggle");
    assert_eq!(words(&runtime, "record-toggle"), "Start recording");
    assert_eq!(words(&runtime, "notice"), "Recording saved · 42 s");
    assert!(shows(&runtime, "status-idle"));
}

#[test]
fn replay_shows_what_it_holds_and_a_save_confirms_the_real_length() {
    let mut runtime = scene("idle");
    press(&mut runtime, "replay-toggle");
    pass(&mut runtime, Duration::from_millis(43_400));
    assert!(shows(&runtime, "status-replay"));
    assert_eq!(
        words(&runtime, "replay-state"),
        "Holding 43 s of 1 min in memory"
    );
    assert!(!dimmed(&runtime, "save-short"));

    press(&mut runtime, "save-short");
    pass(&mut runtime, Duration::from_secs(1));
    assert_eq!(words(&runtime, "notice"), "Replay saved · 30 s");
    let confirmation = runtime.state.confirmation.clone().expect("a confirmation");
    assert_eq!(confirmation.text, "Replay saved · 30 s");

    press(&mut runtime, "replay-toggle");
    assert!(shows(&runtime, "status-idle"));
    assert!(dimmed(&runtime, "save-everything"));
}

#[test]
fn the_saved_replay_scene_reports_a_clip_a_little_longer_than_asked() {
    let runtime = scene("replay-saved");
    assert_eq!(words(&runtime, "notice"), "Replay saved · 31 s");
}

#[test]
fn where_targets_cannot_be_listed_the_picker_is_the_only_way() {
    let mut runtime = scene("picker-only");
    assert!(!shows(&runtime, "target-kind"));
    assert!(shows(&runtime, "picked-nothing"));
    assert_eq!(words(&runtime, "recording-state"), "Choose what to record");

    press(&mut runtime, "pick");
    assert!(shows(&runtime, "picked-waiting"));
    assert!(dimmed(&runtime, "pick"));

    pass(&mut runtime, Duration::from_secs(1));
    assert_eq!(words(&runtime, "picked-name"), "Counter-Strike 2");
    assert_eq!(words(&runtime, "pick"), "Pick something else…");
    assert_eq!(
        words(&runtime, "recording-state"),
        "Ready to record Counter-Strike 2"
    );
}

#[test]
fn starting_with_nothing_picked_opens_the_picker_and_then_records() {
    let mut runtime = scene("picker-only");
    press(&mut runtime, "record-toggle");
    assert!(shows(&runtime, "picked-waiting"));
    assert_eq!(
        words(&runtime, "notice"),
        "Choose what to record in the system picker"
    );
    pass(&mut runtime, Duration::from_secs(1));
    assert_eq!(words(&runtime, "record-toggle"), "Stop recording");
}

#[test]
fn a_cancelled_picker_goes_back_to_nothing_chosen() {
    let runtime = scene("picker-cancelled");
    assert!(shows(&runtime, "picked-nothing"));
    assert!(!dimmed(&runtime, "pick"));
}

#[test]
fn problems_have_a_banner_each_that_can_be_dismissed() {
    let mut runtime = scene("problems");
    assert_eq!(
        words(&runtime, "problem-capture-restarted-title"),
        "Capture stopped for a moment and was restarted"
    );
    assert_eq!(
        words(&runtime, "problem-permission-microphone-title"),
        "ngine is not allowed to use the microphone"
    );
    assert!(shows(&runtime, "status-recording"));

    press(&mut runtime, "problem-capture-restarted-dismiss");
    assert!(!shows(&runtime, "problem-capture-restarted"));
    assert!(shows(&runtime, "problem-permission-microphone"));
}

#[test]
fn a_missing_permission_blocks_the_start_and_says_what_to_do() {
    let runtime = scene("permission-missing");
    assert_eq!(
        words(&runtime, "problem-permission-screen-detail"),
        "Allow it in the system's privacy settings, then start again."
    );
    assert_eq!(words(&runtime, "record-toggle"), "Start recording");
    assert!(shows(&runtime, "status-idle"));
    assert!(!shows(&runtime, "notice"));
}

#[test]
fn a_full_disk_and_a_lost_capture_say_the_recording_was_kept() {
    let full = scene("disk-full");
    assert_eq!(words(&full, "problem-disk-full-title"), "The disk is full");
    assert!(words(&full, "problem-disk-full-detail").contains(crate::app::demo::OUTPUT_FOLDER));
    let lost = scene("capture-lost");
    assert_eq!(
        words(&lost, "problem-capture-lost-detail"),
        "What was recorded up to then is saved."
    );
}

#[test]
fn the_video_settings_offer_only_the_codecs_the_engine_has() {
    let mut runtime = settings(SettingsTab::Video);
    for key in ["codec", "quality", "frame-rate", "output-folder"] {
        assert!(shows(&runtime, key), "{key}");
    }
    assert_eq!(
        words(&runtime, "codec-row-hint"),
        "AV1 cannot be encoded on this machine"
    );
    assert!(dimmed(&runtime, "codec.av1"));
    press(&mut runtime, "codec.av1");
    assert_eq!(runtime.state.settings.video.codec, Codec::H264);
    press(&mut runtime, "codec.hevc");
    assert_eq!(runtime.state.settings.video.codec, Codec::Hevc);
    press(&mut runtime, "frame-rate.120");
    assert_eq!(runtime.state.settings.video.frame_rate, 120);
    assert_eq!(
        words(&runtime, "quality-row-hint"),
        "About 110 MB for each minute"
    );
}

#[test]
fn the_audio_settings_switch_sources_and_tracks() {
    let mut runtime = settings(SettingsTab::Audio);
    for key in ["system-volume", "microphone-volume", "tracks", "app-only"] {
        assert!(shows(&runtime, key), "{key}");
    }
    assert_eq!(words(&runtime, "system-volume-value"), "100 %");
    press(&mut runtime, "microphone");
    assert!(!runtime.state.settings.audio.microphone.enabled);
    press(&mut runtime, "tracks.mix-only");
    assert_eq!(
        runtime.state.settings.audio.tracks,
        crate::engine::Tracks::MixOnly
    );
    press(&mut runtime, "app-only");
    assert!(!runtime.state.settings.audio.app_only);
}

#[test]
fn a_slider_moves_to_the_stop_under_the_pointer() {
    let mut runtime = settings(SettingsTab::Audio);
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
fn the_replay_settings_say_what_the_buffer_costs_and_where_it_lives() {
    let mut runtime = settings(SettingsTab::Replay);
    for key in [
        "replay-length",
        "storage",
        "memory-budget",
        "buffer-folder",
        "save-length",
        "clear-after-save",
        "replay-at-start",
    ] {
        assert!(shows(&runtime, key), "{key}");
    }
    assert_eq!(
        words(&runtime, "replay-length-row-hint"),
        "1 min ≈ 90 MB in memory"
    );
    assert_eq!(words(&runtime, "memory-budget-value"), "Automatic, 512 MB");

    press(&mut runtime, "storage.disk");
    assert_eq!(runtime.state.settings.replay.storage, ReplayStorage::Disk);
    assert_eq!(
        words(&runtime, "replay-length-row-hint"),
        "1 min ≈ 90 MB on disk"
    );

    let long = scene("settings-replay-long");
    assert_eq!(
        words(&long, "replay-length-row-hint"),
        "20 min ≈ 1.8 GB on disk"
    );
    assert_eq!(words(&long, "save-length-value"), "2 min");
}

#[test]
fn a_shorter_buffer_shortens_the_custom_save_length() {
    let mut runtime = settings(SettingsTab::Replay);
    runtime.update(|app, _| app.edit(|settings| settings.replay.length_seconds = 30));
    runtime.frame(FRAME);
    runtime.settle_all();
    assert_eq!(runtime.state.settings.replay.save_seconds, 30);
    assert_eq!(words(&runtime, "save-length-value"), "30 s");
}

#[test]
fn the_hotkey_settings_name_what_is_wrong_with_a_shortcut() {
    let fine = settings(SettingsTab::Hotkeys);
    for slot in 0..5 {
        assert!(shows(&fine, &format!("hotkey-{slot}")), "hotkey {slot}");
    }
    assert!(shows(&fine, "hotkeys-commands"));

    let troubled = scene("settings-hotkey-problem");
    assert_eq!(
        words(&troubled, "hotkey-1-row-hint"),
        "This shortcut is used twice"
    );
    assert_eq!(
        words(&troubled, "hotkey-4-row-hint"),
        "ngine cannot use this shortcut. Write it like ctrl+alt+r"
    );
}

#[test]
fn a_typed_hotkey_is_used_once_enter_is_pressed() {
    let mut runtime = settings(SettingsTab::Hotkeys);
    press(&mut runtime, "hotkey-4");
    runtime.update(|app, _| app.fields.hotkeys[4].replace_text("ctrl+shift+f9"));
    runtime.frame(FRAME);
    runtime.settle_all();
    assert_eq!(
        words(&runtime, "hotkey-4-row-hint"),
        "Press Enter to use it"
    );
    assert_eq!(runtime.state.settings.hotkeys.marker, "ctrl+alt+m");
    runtime.key(KeyEvent {
        key: Key::Enter,
        mods: Mods::default(),
    });
    runtime.frame(FRAME);
    runtime.settle_all();
    assert_eq!(runtime.state.settings.hotkeys.marker, "ctrl+shift+f9");
}

#[test]
fn typing_in_the_output_folder_changes_the_setting() {
    let mut runtime = settings(SettingsTab::Video);
    press(&mut runtime, "output-folder");
    type_text(&mut runtime, "x");
    let folder = runtime.state.settings.output_folder.clone();
    assert!(
        folder.to_string_lossy().contains('x'),
        "{}",
        folder.display()
    );
    assert_ne!(folder, PathBuf::from(crate::app::demo::OUTPUT_FOLDER));
}

#[test]
fn the_general_settings_hold_start_at_login_and_say_what_this_build_is() {
    let mut runtime = settings(SettingsTab::General);
    assert_eq!(words(&runtime, "about-note"), super::PREVIEW_NOTE);
    press(&mut runtime, "start-at-login");
    assert!(runtime.state.settings.start_at_login);
}

#[test]
fn the_tabs_switch_the_settings_shown() {
    let mut runtime = settings(SettingsTab::Video);
    for (tab, shown) in [
        ("audio", "settings-audio"),
        ("replay", "settings-replay"),
        ("hotkeys", "settings-hotkeys"),
        ("general", "settings-general"),
        ("video", "settings-video"),
    ] {
        press(&mut runtime, &format!("settings-tab.{tab}"));
        assert!(shows(&runtime, shown), "{shown}");
    }
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
    press(&mut runtime, "banner-settings-reset-dismiss");
    assert!(!shows(&runtime, "banner-settings-reset"));
    std::fs::remove_dir_all(&folder).expect("temp folder removed");
}

#[test]
fn the_navigation_reaches_the_library_and_the_clip_editor() {
    let mut runtime = scene("idle");
    press(&mut runtime, "nav-library");
    assert_eq!(words(&runtime, "library-title"), "Library");
    assert_eq!(words(&runtime, "library-state"), "Not built yet");
    press(&mut runtime, "nav-editor");
    assert_eq!(words(&runtime, "editor-title"), "Clip editor");
    press(&mut runtime, "nav-settings");
    assert!(shows(&runtime, "settings-tab"));
    press(&mut runtime, "nav-record");
    assert!(shows(&runtime, "record"));
}
