use std::time::Duration;

use rune::tok::{radius, space, type_};
use rune::{Color, El, col, el, row, spacer, text};

use super::widgets::{
    Choice, Look, Paint, STRONG, Segments, TIMER, button, caption, card, segmented, switch,
};
use crate::app::{Action, Ngine};
use crate::engine::{Target, TargetKind};
use crate::format;
use crate::hotkeys;
use crate::settings::SHORT_SAVE_SECONDS;

const KIND_WIDTH: f32 = 72.0;
const TARGET_HEIGHT: f32 = 36.0;
const RADIO: f32 = 14.0;
const RADIO_DOT: f32 = 6.0;

fn kind_id(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Screen => "screen",
        TargetKind::Window => "window",
        TargetKind::App => "app",
    }
}

fn kind_label(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Screen => "Screen",
        TargetKind::Window => "Window",
        TargetKind::App => "App",
    }
}

fn kinds(p: &Paint, app: &Ngine) -> El {
    let choices = TargetKind::ALL
        .map(|kind| Choice::new(kind_id(kind), kind_label(kind)))
        .into();
    let segments = Segments {
        key: "target-kind",
        label: "What to record",
        choices,
        selected: TargetKind::ALL
            .iter()
            .position(|kind| *kind == app.target_kind),
        segment_width: KIND_WIDTH,
    };
    segmented(p, segments, |app, index| {
        if let Some(kind) = TargetKind::ALL.get(index) {
            app.show_kind(*kind);
        }
    })
}

fn radio(p: &Paint, key: &str, chosen: bool) -> El {
    let mut ring = el()
        .key(format!("{key}-radio"))
        .size(RADIO, RADIO)
        .radius(RADIO / 2.0)
        .border(
            1.5,
            if chosen {
                p.accent
            } else {
                p.t.text_secondary.alpha(0.5)
            },
        )
        .center()
        .a11y_hidden();
    if chosen {
        ring = ring.child(
            el().key(format!("{key}-radio-dot"))
                .size(RADIO_DOT, RADIO_DOT)
                .radius(RADIO_DOT / 2.0)
                .fill(p.accent),
        );
    }
    ring
}

fn target_words(p: &Paint, key: &str, target: &Target) -> [El; 3] {
    [
        text(target.name.clone())
            .key(format!("{key}-name"))
            .font(type_::BODY)
            .color(p.t.text)
            .a11y_hidden(),
        spacer(),
        text(target.detail.clone())
            .key(format!("{key}-detail"))
            .font(type_::LABEL)
            .color(p.t.text_secondary)
            .a11y_hidden(),
    ]
}

fn target_row(p: &Paint, app: &Ngine, index: usize, target: &Target) -> El {
    let key = format!("target-{}", target.id);
    let chosen = app.target.as_ref() == Some(target);
    row()
        .key(key.clone())
        .role("radio")
        .label(target.name.clone())
        .checked(chosen)
        .h(TARGET_HEIGHT)
        .px(space::S3)
        .gap(space::S3)
        .center_y()
        .radius(radius::MD)
        .hover(p.t.hover)
        .press(p.t.press)
        .focusable(true)
        .on_press(move |app: &mut Ngine| app.choose(index))
        .child(radio(p, &key, chosen))
        .children(target_words(p, &key, target))
}

fn picked_row(p: &Paint, app: &Ngine) -> El {
    let picker_label = if app.target.is_some() {
        "Pick something else…"
    } else {
        "Pick…"
    };
    let mut line = row()
        .key("picked")
        .min_h(TARGET_HEIGHT + space::S2)
        .px(space::S3)
        .gap(space::S3)
        .center_y();
    line = match (&app.target, app.picking) {
        (_, true) => line
            .child(
                text("Waiting for the system picker…")
                    .key("picked-waiting")
                    .font(type_::BODY)
                    .color(p.t.text_secondary),
            )
            .child(spacer()),
        (Some(target), false) => line
            .child(radio(p, "picked", true))
            .children(target_words(p, "picked", target)),
        (None, false) => line
            .child(
                text("Nothing chosen yet")
                    .key("picked-nothing")
                    .font(type_::BODY)
                    .color(p.t.text_secondary),
            )
            .child(spacer()),
    };
    line.child(button(
        p,
        "pick",
        picker_label,
        Look::Plain,
        !app.picking,
        Ngine::pick,
    ))
}

fn targets(p: &Paint, app: &Ngine) -> El {
    let mut header = row().key("targets-header").center_y().gap(space::S3).child(
        text("What to record")
            .key("targets-title")
            .font(type_::SECTION)
            .color(p.t.text_secondary)
            .ml(space::S3),
    );
    header = header.child(spacer());
    let mut rows = Vec::new();
    if app.capabilities.lists_targets {
        header = header.child(kinds(p, app));
        rows.extend(
            app.targets
                .iter()
                .enumerate()
                .map(|(index, target)| target_row(p, app, index, target)),
        );
        let picked_elsewhere = app
            .target
            .as_ref()
            .is_some_and(|target| !app.targets.contains(target));
        if picked_elsewhere || app.picking || rows.is_empty() {
            rows.push(picked_row(p, app));
        } else {
            header = header.child(button(p, "pick", "Pick…", Look::Plain, true, Ngine::pick));
        }
    } else {
        rows.push(picked_row(p, app));
    }
    col()
        .key("targets")
        .gap(space::S2)
        .child(header)
        .child(card(p, "targets-card", rows))
}

fn hotkey_hint(app: &Ngine, hints: &[(usize, &str)]) -> Option<String> {
    let hotkeys = app.settings.hotkeys.all();
    let parts: Vec<String> = hints
        .iter()
        .filter(|(slot, _)| app.hotkey_problems[*slot].is_none())
        .filter_map(|(slot, does)| {
            hotkeys::keycaps(hotkeys[*slot]).map(|keys| format!("{keys} {does}"))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("  ·  "))
}

fn recording_words(app: &Ngine) -> String {
    match (&app.status.recording, &app.target) {
        (Some(recording), _) => {
            let markers = match recording.markers {
                0 => String::new(),
                1 => "  ·  1 marker".into(),
                many => format!("  ·  {many} markers"),
            };
            format!("Recording {}{markers}", recording.target.name)
        }
        (None, Some(target)) => format!("Ready to record {}", target.name),
        (None, None) => "Choose what to record".into(),
    }
}

fn recording(p: &Paint, app: &Ngine) -> El {
    let running = app.status.recording.as_ref();
    let elapsed = running.map_or(Duration::ZERO, |recording| recording.elapsed);
    let toggle = if running.is_some() {
        button(
            p,
            "record-toggle",
            "Stop recording",
            Look::Danger,
            true,
            |app| {
                app.perform(Action::ToggleRecording);
            },
        )
    } else {
        button(
            p,
            "record-toggle",
            "Start recording",
            Look::Primary,
            true,
            |app| {
                app.perform(Action::ToggleRecording);
            },
        )
    };
    let mut body = col()
        .key("recording-body")
        .padding(space::S3)
        .gap(space::S3)
        .child(
            row()
                .key("recording-row")
                .gap(space::S3)
                .center_y()
                .child(
                    col()
                        .key("recording-words")
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(
                            text(format::timer(elapsed))
                                .key("elapsed")
                                .font(TIMER)
                                .color(if running.is_some() {
                                    p.t.text
                                } else {
                                    p.t.text_secondary.alpha(0.6)
                                }),
                        )
                        .child(caption(p, "recording-state", recording_words(app))),
                )
                .child(button(
                    p,
                    "marker",
                    "Add marker",
                    Look::Plain,
                    running.is_some(),
                    |app| app.perform(Action::AddMarker),
                ))
                .child(toggle),
        );
    let hints = [(0, "starts and stops from anywhere"), (4, "adds a marker")];
    if let Some(hint) = hotkey_hint(app, &hints) {
        body = body.child(caption(p, "recording-hotkeys", hint));
    }
    col()
        .key("recording")
        .gap(space::S2)
        .child(
            text("Recording")
                .key("recording-title")
                .font(type_::SECTION)
                .color(p.t.text_secondary)
                .ml(space::S3),
        )
        .child(card(p, "recording-card", vec![body]))
}

pub fn cost_words(app: &Ngine) -> String {
    let cost = app.replay_cost();
    let length = Duration::from_secs(u64::from(app.settings.replay.length_seconds));
    format!(
        "{} ≈ {} {}",
        format::length(length),
        format::megabytes(cost.megabytes),
        if cost.on_disk { "on disk" } else { "in memory" }
    )
}

fn replay_words(app: &Ngine) -> String {
    match &app.status.replay {
        Some(replay) => format!(
            "Holding {} of {} {}",
            format::length(replay.held),
            format::length(replay.length),
            if replay.on_disk {
                "on disk"
            } else {
                "in memory"
            }
        ),
        None => format!("Off. When on, {}.", cost_words(app)),
    }
}

fn save_buttons(p: &Paint, app: &Ngine) -> El {
    let can_save = app
        .status
        .replay
        .as_ref()
        .is_some_and(|replay| !replay.held.is_zero());
    let seconds = |seconds: u16| format::length(Duration::from_secs(u64::from(seconds)));
    row()
        .key("save-buttons")
        .gap(space::S2)
        .child(button(
            p,
            "save-short",
            &format!("Save last {}", seconds(SHORT_SAVE_SECONDS)),
            Look::Plain,
            can_save,
            |app| app.perform(Action::SaveShort),
        ))
        .child(button(
            p,
            "save-custom",
            &format!("Save last {}", seconds(app.settings.replay.save_seconds)),
            Look::Plain,
            can_save,
            |app| app.perform(Action::SaveCustom),
        ))
        .child(button(
            p,
            "save-everything",
            "Save everything",
            Look::Plain,
            can_save,
            |app| app.perform(Action::SaveEverything),
        ))
}

fn replay(p: &Paint, app: &Ngine) -> El {
    let on = app.status.replay.is_some();
    let mut body = col()
        .key("replay-body")
        .padding(space::S3)
        .gap(space::S3)
        .child(
            row()
                .key("replay-toggle")
                .role("switch")
                .label("Instant replay")
                .checked(on)
                .gap(space::S3)
                .center_y()
                .focusable(true)
                .on_press(|app: &mut Ngine| app.perform(Action::ToggleReplay))
                .child(
                    col()
                        .key("replay-words")
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(
                            text("Instant replay")
                                .key("replay-name")
                                .font(STRONG)
                                .color(p.t.text)
                                .a11y_hidden(),
                        )
                        .child(caption(p, "replay-state", replay_words(app))),
                )
                .child(switch(p, "replay-switch", on)),
        );
    body = body.child(save_buttons(p, app));
    let hints = [(1, "saves the last 30 s"), (3, "saves everything")];
    if let Some(hint) = hotkey_hint(app, &hints) {
        body = body.child(caption(p, "replay-hotkeys", hint));
    }
    col()
        .key("replay")
        .gap(space::S2)
        .child(
            text("Instant replay")
                .key("replay-title")
                .font(type_::SECTION)
                .color(p.t.text_secondary)
                .ml(space::S3),
        )
        .child(card(p, "replay-card", vec![body]))
}

pub fn view(p: &Paint, app: &Ngine) -> El {
    col()
        .key("record")
        .gap(space::S3 + space::S1)
        .fill(Color::TRANSPARENT)
        .child(targets(p, app))
        .child(recording(p, app))
        .child(replay(p, app))
}
