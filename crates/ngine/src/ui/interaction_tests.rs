use rune::{Cursor, ElementInfo, Env, Key, KeyEvent, Mods, Runtime};

use super::view;
use crate::app::{Ngine, WINDOW_SIZE, demo, on_key};

const FRAME: f64 = 1.0 / 60.0;

type Way = (&'static str, fn(&mut Runtime<Ngine>));
const ROUNDS: usize = 20;
const PRESSED: [&str; 8] = [
    "button",
    "menuitem",
    "menuitemradio",
    "option",
    "switch",
    "combobox",
    "tab",
    "slider",
];

fn scene(name: &str) -> Runtime<Ngine> {
    let mut app = demo::idle();
    let (_, apply) = demo::SCENES
        .iter()
        .find(|(scene, _)| *scene == name)
        .unwrap_or_else(|| panic!("no scene called {name}"));
    apply(&mut app);
    let mut runtime = Runtime::new(app, view, WINDOW_SIZE, Env::new(true, 2.0));
    runtime.on_key = Some(on_key);
    runtime.frame(0.0);
    runtime.settle_all();
    runtime
}

fn live(runtime: &Runtime<Ngine>) -> Vec<ElementInfo> {
    runtime
        .elements()
        .into_iter()
        .filter(|element| !element.exiting && !element.inert)
        .collect()
}

fn element(runtime: &Runtime<Ngine>, key: &str) -> Option<ElementInfo> {
    live(runtime)
        .into_iter()
        .find(|element| element.key.as_deref() == Some(key))
}

fn centre(element: &ElementInfo) -> Option<(f32, f32)> {
    let [x, y, width, height] = element.visible;
    (width >= 2.0 && height >= 2.0).then(|| (x + width / 2.0, y + height / 2.0))
}

fn click_at(runtime: &mut Runtime<Ngine>, (x, y): (f32, f32)) {
    runtime.pointer_move(x, y);
    runtime.frame(FRAME);
    runtime.pointer_down(x, y);
    runtime.frame(FRAME);
    runtime.pointer_up(x, y);
    runtime.frame(FRAME);
}

fn click(runtime: &mut Runtime<Ngine>, key: &str) {
    let at = runtime
        .elements()
        .into_iter()
        .find(|element| !element.exiting && element.key.as_deref() == Some(key))
        .as_ref()
        .and_then(centre)
        .unwrap_or_else(|| panic!("nothing to click with the key {key}"));
    click_at(runtime, at);
}

fn key(runtime: &mut Runtime<Ngine>, key: Key) {
    runtime.key(KeyEvent {
        key,
        mods: Mods::default(),
    });
    runtime.frame(FRAME);
}

fn shows(runtime: &Runtime<Ngine>, key: &str) -> bool {
    element(runtime, key).is_some()
}

fn hovered_cursor(runtime: &mut Runtime<Ngine>, at: (f32, f32)) -> Cursor {
    runtime.pointer_move(at.0, at.1);
    runtime.frame(FRAME);
    runtime.cursor()
}

fn outside(runtime: &Runtime<Ngine>) -> (f32, f32) {
    let (width, height) = WINDOW_SIZE;
    let _ = runtime;
    (width - 6.0, height - 6.0)
}

const SCENES: [&str; 15] = [
    "idle",
    "idle-with-recent",
    "replay",
    "sources-open",
    "settings-video",
    "settings-audio",
    "settings-replay",
    "settings-hotkeys",
    "settings-storage",
    "settings-general",
    "library",
    "library-menu",
    "library-renaming",
    "editor",
    "editor-keys",
];

#[test]
fn every_control_shows_the_hand_and_every_text_field_the_text_cursor() {
    let mut hovered = 0;
    for name in SCENES {
        let mut runtime = scene(name);
        let controls: Vec<ElementInfo> = live(&runtime)
            .into_iter()
            .filter(|element| PRESSED.contains(&element.role) || element.role == "segment")
            .filter(|element| element.opacity >= 1.0)
            .collect();
        for control in controls {
            let Some(at) = centre(&control) else {
                continue;
            };
            let Some(on_top) = runtime.element_at(at.0, at.1) else {
                continue;
            };
            let here = control.key.clone().unwrap_or_default();
            if on_top.key() != Some(here.as_str())
                && !on_top.key().is_some_and(|key| key.starts_with(&here))
            {
                continue;
            }
            let wanted = Cursor::Pointer;
            let shown = hovered_cursor(&mut runtime, at);
            hovered += 1;
            assert!(
                shown == wanted
                    || (control.key.as_deref() == Some("timeline")
                        && shown == Cursor::ResizeColumn),
                "{name}: {here} ({}) shows {shown:?}",
                control.role
            );
        }
        let fields: Vec<ElementInfo> = live(&runtime)
            .into_iter()
            .filter(|element| element.role == "textbox")
            .collect();
        for field in fields {
            if let Some(at) = centre(&field) {
                assert_eq!(
                    hovered_cursor(&mut runtime, at),
                    Cursor::Text,
                    "{name}: {:?}",
                    field.key
                );
            }
        }
    }
    assert!(hovered > 100, "only {hovered} controls were hovered");
}

#[test]
fn the_clip_ends_show_the_resize_cursor_and_the_rest_of_the_timeline_the_hand() {
    let mut runtime = scene("editor");
    let timeline = element(&runtime, "timeline").expect("a timeline");
    let in_point = element(&runtime, "in-point").expect("the start of the clip");
    let y = timeline.rect[1] + timeline.rect[3] / 2.0;
    let x = in_point.rect[0] + in_point.rect[2] / 2.0;
    assert_eq!(hovered_cursor(&mut runtime, (x, y)), Cursor::ResizeColumn);
    assert_eq!(
        hovered_cursor(&mut runtime, (timeline.rect[0] + timeline.rect[2] - 4.0, y)),
        Cursor::Pointer
    );
}

#[test]
fn every_menu_opens_on_the_first_click_and_closes_every_way() {
    let mut checked = Vec::new();
    for name in [
        "idle",
        "library",
        "editor",
        "settings-video",
        "settings-audio",
        "settings-replay",
    ] {
        let runtime = scene(name);
        let triggers: Vec<String> = live(&runtime)
            .into_iter()
            .filter(|element| element.expanded == Some(false) && element.opacity >= 1.0)
            .filter_map(|element| element.key)
            .filter(|key| key != "nav-settings")
            .collect();
        for trigger in triggers {
            let mut runtime = scene(name);
            for round in 0..3 {
                click(&mut runtime, &trigger);
                let opened = runtime
                    .elements()
                    .into_iter()
                    .find(|e| !e.exiting && e.key.as_deref() == Some(trigger.as_str()))
                    .and_then(|e| e.expanded);
                assert_eq!(
                    opened,
                    Some(true),
                    "{name}: {trigger} round {round}: first click"
                );
                assert!(runtime.state.popup.is_some(), "{name}: {trigger}");
                key(&mut runtime, Key::Escape);
                assert!(runtime.state.popup.is_none(), "{name}: {trigger}: Escape");

                click(&mut runtime, &trigger);
                assert!(runtime.state.popup.is_some(), "{name}: {trigger}: reopen");
                click(&mut runtime, &trigger);
                assert!(
                    runtime.state.popup.is_none(),
                    "{name}: {trigger}: second click"
                );

                click(&mut runtime, &trigger);
                assert!(
                    runtime.state.popup.is_some(),
                    "{name}: {trigger}: third open"
                );
                let at = outside(&runtime);
                click_at(&mut runtime, at);
                assert!(
                    runtime.state.popup.is_none(),
                    "{name}: {trigger}: click outside"
                );
            }
            checked.push(format!("{name}/{trigger}"));
        }
    }
    for wanted in [
        "idle/source",
        "idle/capture-microphone",
        "idle/capture-system-sound",
        "idle/replay-length",
        "library/library-sort",
        "editor/editor-keys-button",
        "library/item-0-more",
        "settings-audio/settings-microphone",
        "settings-replay/settings-memory-budget-presets-button",
    ] {
        assert!(
            checked.iter().any(|one| one == wanted),
            "{wanted} was not checked: {checked:?}"
        );
    }
}

#[test]
fn a_cards_more_button_opens_its_menu_on_the_first_click() {
    let mut runtime = scene("library");
    for _ in 0..ROUNDS {
        click(&mut runtime, "item-0-more");
        assert!(shows(&runtime, "item-menu"));
        key(&mut runtime, Key::Escape);
        assert!(!shows(&runtime, "item-menu"));
    }
}

fn settings_shown(runtime: &Runtime<Ngine>) -> bool {
    runtime.state.settings_open && shows(runtime, "settings")
}

fn command_comma(runtime: &mut Runtime<Ngine>) {
    key(runtime, Key::Command(','));
}

#[test]
fn the_settings_open_and_close_every_way_many_times() {
    let ways_in: [Way; 2] = [
        ("the sidebar", |runtime| click(runtime, "nav-settings")),
        ("the shortcut", command_comma),
    ];
    let ways_out: [Way; 4] = [
        ("the close button", |runtime| {
            click(runtime, "settings-close");
        }),
        ("Escape", |runtime| key(runtime, Key::Escape)),
        ("the shortcut", command_comma),
        ("a click outside", |runtime| {
            let at = outside(runtime);
            click_at(runtime, at);
        }),
    ];
    for name in ["idle", "library", "editor"] {
        let mut runtime = scene(name);
        for round in 0..ROUNDS {
            for (into, open) in ways_in {
                for (out, close) in ways_out {
                    open(&mut runtime);
                    assert!(
                        settings_shown(&runtime),
                        "{name} round {round}: {into} did not open them"
                    );
                    close(&mut runtime);
                    assert!(
                        !runtime.state.settings_open,
                        "{name} round {round}: {out} did not close them"
                    );
                    runtime.settle_all();
                    assert!(
                        !shows(&runtime, "settings"),
                        "{name} round {round}: {out} left the sheet"
                    );
                }
            }
        }
    }
}

#[test]
fn renaming_a_card_moves_nothing_in_the_grid() {
    let tops = |name: &str| -> Vec<(String, f32)> {
        live(&scene(name))
            .into_iter()
            .filter_map(|element| {
                let key = element.key?;
                (key.starts_with("item-") && key.ends_with("-details"))
                    .then_some((key, element.rect[1]))
            })
            .collect()
    };
    let before = tops("library");
    assert_ne!(before.len(), 0);
    assert_eq!(before, tops("library-renaming"));
}

#[test]
fn renaming_a_card_below_the_fold_scrolls_its_field_into_view() {
    let mut app = demo::idle();
    let (_, library) = demo::SCENES
        .iter()
        .find(|(scene, _)| *scene == "library")
        .expect("a library scene");
    library(&mut app);
    let small = crate::app::SMALLEST_WINDOW;
    let mut runtime = Runtime::new(app, view, small, Env::new(true, 2.0));
    runtime.on_key = Some(on_key);
    runtime.frame(0.0);
    runtime.settle_all();
    runtime.update(|app, _| app.open_item_menu(3, Some((200.0, 200.0))));
    runtime.frame(FRAME);
    click(&mut runtime, "menu-rename");
    runtime.settle_all();
    let field = element(&runtime, crate::app::RENAME_BOX).expect("the rename field");
    let [_, top, _, height] = field.visible;
    assert!(
        height >= field.rect[3] - 0.5,
        "the field is cut: {:?}",
        field.visible
    );
    assert!(top + height <= small.1, "the field is below the window");
}
