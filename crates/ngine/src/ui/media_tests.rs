use std::path::{Path, PathBuf};
use std::time::Duration;

use ngine_recorder::Cut;
use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};

use super::view;
use crate::app::{self, Ngine, Screen, demo};
use crate::editor::{Editor, Exporting};
use crate::library::Library;

const FRAME: f64 = 1.0 / 60.0;
const TALL_WINDOW: (f32, f32) = (780.0, 900.0);

fn runtime(app: Ngine) -> Runtime<Ngine> {
    let mut runtime = Runtime::new(app, view, TALL_WINDOW, Env::new(true, 2.0));
    runtime.on_key = Some(app::on_key);
    runtime.frame(0.0);
    runtime.frame(FRAME);
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

fn settle(runtime: &mut Runtime<Ngine>) {
    runtime.frame(FRAME);
    runtime.settle_all();
}

fn press(runtime: &mut Runtime<Ngine>, key: &str) {
    let (x, y) = runtime
        .find(key)
        .unwrap_or_else(|| panic!("nothing to press with the key {key}"));
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    settle(runtime);
}

fn key(runtime: &mut Runtime<Ngine>, key: Key, shift: bool) {
    let mods = Mods {
        shift,
        ..Mods::default()
    };
    runtime.key(KeyEvent { key, mods });
    settle(runtime);
}

fn letter(runtime: &mut Runtime<Ngine>, text: &str) {
    key(runtime, Key::Text(text.into()), false);
}

fn editor(runtime: &Runtime<Ngine>) -> &Editor {
    runtime.state.editor.as_ref().expect("a recording is open")
}

fn seconds(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

fn place_on_timeline(runtime: &Runtime<Ngine>, key: &str) -> f32 {
    let timeline = element(runtime, "timeline").rect;
    let mark = element(runtime, key).rect;
    (mark[0] + mark[2] / 2.0 - timeline[0]) / timeline[2]
}

fn assert_near(found: f32, expected: f32, what: &str) {
    assert!(
        (found - expected).abs() < 0.004,
        "{what} is at {found} instead of {expected}"
    );
}

fn drag_on_timeline(runtime: &mut Runtime<Ngine>, from: f32, to: f32) {
    let [left, top, width, height] = element(runtime, "timeline").rect;
    let y = top + height / 2.0;
    runtime.pointer_down(left + from * width, y);
    runtime.pointer_move(left + f32::midpoint(from, to) * width, y);
    runtime.pointer_move(left + to * width, y);
    runtime.pointer_up(left + to * width, y);
    settle(runtime);
}

#[test]
fn an_empty_library_says_where_recordings_will_appear() {
    let runtime = scene("library-empty");
    assert_eq!(words(&runtime, "library-empty-title"), "No recordings yet");
    assert!(words(&runtime, "library-empty-words").contains(demo::OUTPUT_FOLDER));
    assert!(!shows(&runtime, "library-total"));
    assert!(!shows(&runtime, "library-list"));
}

#[test]
fn the_library_lists_every_video_with_its_length_size_date_and_actions() {
    let runtime = scene("library");
    assert_eq!(words(&runtime, "library-total"), "5 videos · 2.7 GB");
    assert_eq!(
        words(&runtime, "item-0-name"),
        "Recording 2026-11-16 14.03.22"
    );
    assert_eq!(
        words(&runtime, "item-0-details"),
        "16 Nov 2026, 14:03 · 310 MB"
    );
    assert_eq!(words(&runtime, "item-0-length"), "04:12");
    assert_eq!(words(&runtime, "item-0-markers"), "2 markers");
    for index in 0..5 {
        for part in ["thumbnail", "more"] {
            let key = format!("item-{index}-{part}");
            assert!(shows(&runtime, &key), "{key}");
        }
    }
    assert!(!shows(&runtime, "item-5"));
    assert!(!shows(&runtime, "library-empty"));
}

fn temp_folder(test: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("ngine-ui-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("a temp folder");
    folder
}

fn bin(file: &Path) -> Result<(), String> {
    std::fs::rename(file, file.with_extension("binned")).map_err(|error| error.to_string())
}

fn two_videos(folder: &Path) {
    let older = folder.join("Second.mp4");
    std::fs::write(folder.join("First.mp4"), "first").expect("a file");
    std::fs::write(&older, "second").expect("a file");
    let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(3_600);
    std::fs::File::options()
        .write(true)
        .open(older)
        .and_then(|file| file.set_modified(an_hour_ago))
        .expect("an older date");
}

fn library_on_disk(folder: &Path) -> Runtime<Ngine> {
    let mut app = demo::idle();
    app.edit(|settings| settings.output_folder = folder.to_owned());
    app.library = Library::on_disk(folder.join("thumbnails"));
    app.library.load_now(folder);
    app.trash = bin;
    app.screen = Screen::Library;
    runtime(app)
}

#[test]
fn a_video_is_renamed_in_place_and_a_taken_name_is_refused() {
    let folder = temp_folder("rename");
    two_videos(&folder);
    let mut runtime = library_on_disk(&folder);
    assert_eq!(words(&runtime, "item-0-name"), "First");
    assert!(shows(&runtime, "item-0-unreadable"));
    press(&mut runtime, "item-0-more");
    assert!(element(&runtime, "menu-open").opacity < 1.0);

    press(&mut runtime, "menu-rename");
    assert!(shows(&runtime, "rename"));
    assert!(!shows(&runtime, "item-0-name"));
    letter(&mut runtime, "Second");
    key(&mut runtime, Key::Enter, false);
    assert_eq!(
        words(&runtime, "notice"),
        "Another file already has this name"
    );
    assert!(shows(&runtime, "rename"));
    assert!(folder.join("First.mp4").exists());

    key(&mut runtime, Key::Escape, false);
    assert_eq!(words(&runtime, "item-0-name"), "First");

    runtime.update(|app, _| app.start_rename(0));
    settle(&mut runtime);
    letter(&mut runtime, "Boss fight");
    key(&mut runtime, Key::Enter, false);
    assert_eq!(words(&runtime, "item-0-name"), "Boss fight");
    assert!(!shows(&runtime, "rename"));
    assert!(!folder.join("First.mp4").exists());
    assert_eq!(
        std::fs::read_to_string(folder.join("Boss fight.mp4")).expect("the renamed file"),
        "first"
    );
    std::fs::remove_dir_all(&folder).expect("temp folder removed");
}

#[test]
fn moving_a_video_to_the_trash_takes_it_off_the_list() {
    let folder = temp_folder("trash");
    two_videos(&folder);
    let mut runtime = library_on_disk(&folder);

    press(&mut runtime, "item-0-more");
    press(&mut runtime, "menu-trash");
    press(&mut runtime, "trash-confirm");
    assert_eq!(words(&runtime, "item-0-name"), "Second");
    assert!(!shows(&runtime, "item-1"));
    assert!(words(&runtime, "notice").starts_with("1 video moved to the"));
    assert!(folder.join("First.binned").exists());
    assert!(folder.join("Second.mp4").exists());
    std::fs::remove_dir_all(&folder).expect("temp folder removed");
}

#[test]
fn the_editor_without_a_recording_points_to_the_library() {
    let mut runtime = scene("editor-empty");
    assert_eq!(words(&runtime, "editor-empty-title"), "No recording open");
    assert!(!shows(&runtime, "timeline"));
    press(&mut runtime, "editor-to-library");
    assert!(shows(&runtime, "library"));
}

#[test]
fn the_editor_shows_the_preview_the_clip_and_the_markers_at_their_times() {
    let runtime = scene("editor");
    assert_eq!(
        words(&runtime, "editor-name"),
        "Recording 2026-11-16 14.03.22"
    );
    assert_eq!(words(&runtime, "editor-details"), "2560 × 1440");
    assert_eq!(words(&runtime, "position"), "00:31.0 / 02:05.0");
    assert_eq!(words(&runtime, "clip-length"), "Clip 42 s");
    assert_eq!(words(&runtime, "clip-points"), "00:20.0 – 01:02.0");
    assert_eq!(words(&runtime, "play"), "Play");
    assert!(shows(&runtime, "preview"));
    for (index, at) in [12.0, 33.0, 80.0].into_iter().enumerate() {
        let key = format!("marker-{index}");
        assert_near(place_on_timeline(&runtime, &key), at / 125.0, &key);
    }
    assert!(!shows(&runtime, "marker-3"));
    assert_near(place_on_timeline(&runtime, "in-point"), 20.0 / 125.0, "in");
    assert_near(
        place_on_timeline(&runtime, "out-point"),
        62.0 / 125.0,
        "out",
    );
    assert_near(place_on_timeline(&runtime, "playhead"), 31.0 / 125.0, "now");
    for key in [
        "clip-system-volume",
        "clip-system-on",
        "clip-microphone-volume",
        "clip-microphone-on",
        "trim-cut",
        "export",
    ] {
        assert!(shows(&runtime, key), "{key}");
    }
    assert_eq!(editor(&runtime).cut, Cut::Fast);
}

#[test]
fn the_keyboard_plays_steps_and_sets_the_in_and_out_points() {
    let mut runtime = scene("editor");
    letter(&mut runtime, "i");
    assert_eq!(editor(&runtime).start, seconds(31));
    key(&mut runtime, Key::Right, true);
    key(&mut runtime, Key::Right, true);
    assert_eq!(editor(&runtime).position, seconds(33));
    key(&mut runtime, Key::Left, true);
    letter(&mut runtime, "o");
    assert_eq!(editor(&runtime).end, seconds(32));
    assert_eq!(words(&runtime, "clip-length"), "Clip 1 s");

    key(&mut runtime, Key::Right, false);
    let one_frame = editor(&runtime).position.saturating_sub(seconds(32));
    assert!(one_frame > Duration::ZERO && one_frame < Duration::from_millis(50));
    key(&mut runtime, Key::Home, false);
    assert_eq!(editor(&runtime).position, seconds(31));
    key(&mut runtime, Key::End, false);
    assert_eq!(editor(&runtime).position, seconds(32));

    letter(&mut runtime, " ");
    assert!(editor(&runtime).playing);
    assert_eq!(editor(&runtime).position, seconds(31));
    assert_eq!(words(&runtime, "play"), "Pause");
    letter(&mut runtime, " ");
    assert_eq!(words(&runtime, "play"), "Play");
}

#[test]
fn dragging_moves_the_in_point_the_out_point_and_the_playhead() {
    let mut runtime = scene("editor");
    drag_on_timeline(&mut runtime, 20.0 / 125.0, 0.08);
    assert_near(editor(&runtime).start.as_secs_f32(), 10.0, "the in point");
    assert_eq!(editor(&runtime).end, seconds(62));
    assert_eq!(editor(&runtime).position, editor(&runtime).start);

    drag_on_timeline(&mut runtime, 62.0 / 125.0, 0.8);
    assert_near(editor(&runtime).end.as_secs_f32(), 100.0, "the out point");
    assert_near(editor(&runtime).start.as_secs_f32(), 10.0, "the in point");

    drag_on_timeline(&mut runtime, 0.3, 0.4);
    assert_near(
        editor(&runtime).position.as_secs_f32(),
        50.0,
        "the playhead",
    );
    assert_near(editor(&runtime).end.as_secs_f32(), 100.0, "the out point");

    drag_on_timeline(&mut runtime, 0.8, 0.02);
    let editor = editor(&runtime);
    assert!(
        editor.end > editor.start,
        "the out point passed the in point"
    );
    assert!(editor.end < seconds(11));
}

#[test]
fn each_sound_source_can_be_muted_and_the_cut_chosen() {
    let mut runtime = scene("editor");
    press(&mut runtime, "clip-microphone-on");
    assert!(!editor(&runtime).microphone.on);
    let mix = editor(&runtime).mix();
    assert!(mix.microphone.abs() < f32::EPSILON);
    assert!((mix.system - 1.0).abs() < f32::EPSILON);

    press(&mut runtime, "trim-cut.exact");
    assert_eq!(editor(&runtime).cut, Cut::Exact);
    press(&mut runtime, "trim-cut.keyframe");
    assert_eq!(editor(&runtime).cut, Cut::Fast);

    let fixed = scene("editor-mix-only");
    assert!(!shows(&fixed, "clip-sound"));
    assert!(!shows(&fixed, "clip-system-volume"));
}

#[test]
fn an_export_shows_its_progress_its_file_or_its_failure() {
    let running = scene("editor-exporting");
    assert_eq!(words(&running, "export-percent"), "Exporting… 42 %");
    assert!(shows(&running, "export-progress"));
    assert!(shows(&running, "export-cancel"));
    assert!(!shows(&running, "export"));

    let done = scene("editor-exported");
    assert_eq!(words(&done, "export-done"), "Saved · 42 s");
    assert!(shows(&done, "export-show"));
    assert_eq!(words(&done, "export"), "Export again");

    let mut failed = scene("editor-export-failed");
    assert_eq!(
        words(&failed, "export-failed"),
        "The clip could not be exported. The disk is full."
    );
    assert_eq!(words(&failed, "export"), "Try again");
    assert_eq!(words(&failed, "export-folder"), "Change folder…");
    press(&mut failed, "editor-back");
    assert!(shows(&failed, "library"));
    assert!(matches!(
        Editor::sample(Path::new("x.mp4")).exporting,
        Exporting::Idle
    ));
}

#[test]
fn the_full_screen_control_shows_on_hover_and_f_and_escape_enter_and_leave() {
    let mut runtime = scene("editor");
    assert!(!shows(&runtime, "full-screen"));
    runtime.update(|app, _| {
        if let Some(editor) = &mut app.editor {
            editor.hovered = true;
        }
    });
    settle(&mut runtime);
    assert!(shows(&runtime, "full-screen"));
    press(&mut runtime, "full-screen");
    assert!(runtime.state.watching());
    assert!(shows(&runtime, "watching"));
    assert!(!shows(&runtime, "sidebar"));
    assert!(shows(&runtime, "watching-controls"));
    letter(&mut runtime, "i");
    assert_eq!(editor(&runtime).start, seconds(31));
    key(&mut runtime, Key::Escape, false);
    assert!(!runtime.state.watching());
    assert!(shows(&runtime, "sidebar"));
    letter(&mut runtime, "f");
    assert!(runtime.state.watching());
    letter(&mut runtime, "f");
    assert!(!runtime.state.watching());
}

#[test]
fn the_controls_over_a_playing_video_hide_when_the_pointer_rests_and_return_when_it_moves() {
    let mut runtime = scene("editor");
    letter(&mut runtime, "f");
    runtime.update(|app, _| {
        if let Some(editor) = &mut app.editor {
            editor.playing = true;
        }
    });
    settle(&mut runtime);
    assert!(shows(&runtime, "watching-controls"));
    runtime.update(|app, _| {
        app.sync(app.clock() + crate::editor::CONTROLS_LINGER_SECONDS + 0.1);
    });
    settle(&mut runtime);
    assert!(!shows(&runtime, "watching-controls"));
    runtime.update(|app, _| app.pointer_moved(false));
    settle(&mut runtime);
    assert!(shows(&runtime, "watching-controls"));
    runtime.update(|app, _| {
        if let Some(editor) = &mut app.editor {
            editor.playing = false;
        }
        app.sync(app.clock() + 10.0);
    });
    settle(&mut runtime);
    assert!(shows(&runtime, "watching-controls"));
}
