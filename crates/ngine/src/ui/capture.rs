use std::time::Duration;

use rune::tok::{radius, space, type_};
use rune::{
    Align, Color, El, Icon, ScrollSpec, Side, SurfaceFit, col, el, icon, row, spacer, text,
};

use super::widgets::{
    Button, Floating, ICON, Look, MenuItem, Option_, Paint, STRONG, Select, Stops, TIMER, caption,
    card, float, lone_switch, menu, menu_heading, menu_rule, meter, section_title, select, slider,
};
use super::{Ui, hotkey_caps, icons, page, page_title, sheet};
use crate::app::{Action, Ngine, Popup, Screen};
use crate::engine::{AudioSource, Target, TargetKind};
use crate::format;
use crate::hotkeys;
use crate::settings::{REPLAY_LENGTHS, SHORT_SAVE_SECONDS, Settings};

const SOURCE_TILE: f32 = 36.0;
const SOURCES_WIDTH: f32 = 340.0;
const SOURCES_HEIGHT: f32 = 380.0;
const AUDIO_LABEL: f32 = 112.0;
const VOLUME_WIDTH: f32 = 96.0;
const LENGTH_WIDTH: f32 = 112.0;
const RECENT_COUNT: usize = 3;
const VOLUME_STEP: u8 = 5;
const VOLUME_STOPS: u16 = 21;
const OFF: f32 = 0.45;

pub const NO_REPLAY: &str = "Turn instant replay on first";
pub const NOTHING_HELD: &str = "Nothing is held yet";
pub const STOP_TO_CHANGE: &str = "Stop the recording to change the source";

fn kind_icon(kind: TargetKind) -> &'static Icon {
    match kind {
        TargetKind::Screen => &icons::MONITOR,
        TargetKind::Window => &icons::APP_WINDOW,
        TargetKind::App => &icons::LAYOUT_GRID,
    }
}

fn kind_heading(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Screen => "Screens",
        TargetKind::Window => "Windows",
        TargetKind::App => "Apps",
    }
}

fn kind_word(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Screen => "Screen",
        TargetKind::Window => "Window",
        TargetKind::App => "App",
    }
}

fn seconds(seconds: u16) -> String {
    format::length(Duration::from_secs(u64::from(seconds)))
}

fn tile(p: &Paint, key: &str, glyph: &'static Icon) -> El {
    el().key(key)
        .size(SOURCE_TILE, SOURCE_TILE)
        .radius(radius::MD)
        .fill(p.t.control_track)
        .center()
        .child(icon(glyph, 18.0).color(p.t.text_secondary))
        .a11y_hidden()
}

fn source_words(app: &Ngine) -> (&'static Icon, String, String) {
    match (&app.target, app.picking) {
        (_, true) => (
            &icons::MOUSE_POINTER_CLICK,
            "Waiting for the system picker…".into(),
            "Choose a screen or a window in the dialog the system shows".into(),
        ),
        (Some(target), false) => (
            kind_icon(target.kind),
            target.name.clone(),
            format!("{} · {}", kind_word(target.kind), target.detail),
        ),
        (None, false) => (
            &icons::MOUSE_POINTER_CLICK,
            "Nothing chosen yet".into(),
            "Choose a screen, a window or an app to record".into(),
        ),
    }
}

fn source_blocked(app: &Ngine) -> Option<String> {
    if app.status.recording.is_some() {
        Some(STOP_TO_CHANGE.into())
    } else if app.picking {
        Some("The system picker is open".into())
    } else {
        None
    }
}

fn source(ui: &mut Ui<'_>) -> El {
    let p = ui.p;
    let app = ui.app;
    let open = app.popup == Some(Popup::Sources);
    let (glyph, name, detail) = source_words(app);
    let action = if app.capabilities.lists_targets {
        "Change"
    } else {
        "Choose…"
    };
    let mut line = row()
        .key("source")
        .role("button")
        .label(format!("Source: {name}. {action}"))
        .expanded(open)
        .min_h(SOURCE_TILE + 2.0 * space::S3)
        .px(space::S3)
        .gap(space::S3)
        .center_y()
        .radius(radius::LG)
        .fill(p.t.card)
        .border(p.hair, if open { p.accent } else { p.t.card_border })
        .child(tile(&p, "source-icon", glyph))
        .child(
            col()
                .key("source-words")
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(name)
                        .key("source-name")
                        .font(STRONG)
                        .color(p.t.text)
                        .truncate()
                        .a11y_hidden(),
                )
                .child(caption(&p, "source-detail", detail).a11y_hidden()),
        )
        .child(
            row()
                .key("source-action")
                .gap(space::S1)
                .center_y()
                .child(
                    text(action)
                        .font(type_::LABEL)
                        .color(p.t.text_secondary)
                        .a11y_hidden(),
                )
                .child(
                    icon(&icons::CHEVRON_DOWN, ICON)
                        .color(p.t.text_secondary)
                        .a11y_hidden(),
                ),
        );
    line = match source_blocked(app) {
        Some(why) => line
            .disabled(true)
            .focusable(false)
            .description(why.clone())
            .help(why),
        None => line
            .hover(p.t.hover)
            .press(p.t.press)
            .focusable(true)
            .on_press(|app: &mut Ngine| {
                if app.popup == Some(Popup::Sources) {
                    app.close_popup();
                } else {
                    app.open_sources();
                }
            }),
    };
    if open {
        sources_menu(ui);
    }
    col()
        .key("source-group")
        .gap(space::S2)
        .child(section_title(&p, "source-title", "Source").ml(space::S1))
        .child(line)
}

fn sources_menu(ui: &mut Ui<'_>) {
    let p = ui.p;
    let app = ui.app;
    let mut items = Vec::new();
    for kind in TargetKind::ALL {
        let of_kind: Vec<(usize, &Target)> = app
            .sources
            .iter()
            .enumerate()
            .filter(|(_, target)| target.kind == kind)
            .collect();
        if of_kind.is_empty() {
            continue;
        }
        items.push(menu_heading(
            &p,
            &format!("sources-{}", kind_heading(kind).to_lowercase()),
            kind_heading(kind),
        ));
        for (index, target) in of_kind {
            let key = format!("source-{}", target.id);
            items.push(
                MenuItem::new(&key, &target.name)
                    .icon(kind_icon(kind))
                    .detail(target.detail.clone())
                    .chosen(app.target.as_ref() == Some(target))
                    .build(&p, "menuitemradio", move |app, _| app.choose(index)),
            );
        }
    }
    if !items.is_empty() {
        items.push(menu_rule(&p, "sources-rule"));
    }
    items.push(
        MenuItem::new("source-picker", "Choose with the system picker…")
            .icon(&icons::MOUSE_POINTER_CLICK)
            .build(&p, "menuitem", |app, _| app.pick()),
    );
    let list = menu(&p, "sources", "Sources", items)
        .w(SOURCES_WIDTH.min(ui.content_width()))
        .max_h(SOURCES_HEIGHT)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let floating = Floating {
        key: "sources",
        anchor: "source",
        side: Side::Bottom,
        align: Align::Start,
    };
    float(&mut ui.layer, &floating, list);
}

/// The volume of one source as a slider in steps of five percent.
pub fn volume(
    p: &Paint,
    key: &str,
    label: &str,
    percent: u8,
    width: f32,
    set: fn(&mut Settings, u8),
) -> El {
    let stops = Stops {
        key,
        label,
        index: u16::from(percent / VOLUME_STEP),
        count: VOLUME_STOPS,
        value: format!("{percent} %"),
        width,
    };
    slider(p, stops, move |app, index| {
        let percent = u8::try_from(index)
            .unwrap_or(u8::MAX)
            .saturating_mul(VOLUME_STEP);
        app.edit(|settings| set(settings, percent.min(100)));
    })
}

#[derive(Clone, Copy)]
struct AudioRow {
    key: &'static str,
    label: &'static str,
    glyph: &'static Icon,
    source: AudioSource,
    level: Option<u8>,
    flip: fn(&mut Settings),
    set_volume: fn(&mut Settings, u8),
}

fn audio_row(p: &Paint, narrow: bool, audio: AudioRow) -> El {
    let AudioRow {
        key,
        label,
        glyph,
        source,
        level,
        flip,
        set_volume,
    } = audio;
    let on = source.enabled;
    let shown = if on { 1.0 } else { OFF };
    let mut line = row()
        .key(key)
        .min_h(44.0)
        .px(space::S3)
        .gap(space::S3)
        .center_y()
        .child(
            icon(glyph, 16.0)
                .color(p.t.text_secondary)
                .opacity(shown)
                .a11y_hidden(),
        )
        .child(
            text(label)
                .key(format!("{key}-label"))
                .font(type_::BODY)
                .color(p.t.text)
                .w(AUDIO_LABEL)
                .a11y_hidden(),
        )
        .child(
            el().key(format!("{key}-meter-box"))
                .grow(1.0)
                .min_w(40.0)
                .opacity(shown)
                .child(meter(
                    p,
                    &format!("{key}-meter"),
                    &format!("{label} level"),
                    level.filter(|_| on),
                )),
        );
    if !narrow {
        line = line.child(
            volume(
                p,
                &format!("{key}-volume"),
                &format!("{label} volume"),
                source.volume_percent,
                VOLUME_WIDTH,
                set_volume,
            )
            .opacity(shown),
        );
    }
    line.child(lone_switch(
        p,
        &format!("{key}-toggle"),
        label,
        on,
        move |app| app.edit(flip),
    ))
}

fn audio(ui: &Ui<'_>) -> El {
    let p = ui.p;
    let app = ui.app;
    let audio = app.settings.audio;
    let levels = app.status.levels;
    let narrow = ui.content_width() < 560.0;
    let rows = vec![
        audio_row(
            &p,
            narrow,
            AudioRow {
                key: "audio-system",
                label: "System sound",
                glyph: &icons::VOLUME_2,
                source: audio.system,
                level: levels.map(|levels| levels.system),
                flip: |settings| settings.audio.system.enabled ^= true,
                set_volume: |settings, percent| settings.audio.system.volume_percent = percent,
            },
        ),
        audio_row(
            &p,
            narrow,
            AudioRow {
                key: "audio-microphone",
                label: "Microphone",
                glyph: &icons::MIC,
                source: audio.microphone,
                level: levels.map(|levels| levels.microphone),
                flip: |settings| settings.audio.microphone.enabled ^= true,
                set_volume: |settings, percent| settings.audio.microphone.volume_percent = percent,
            },
        ),
    ];
    col()
        .key("audio")
        .gap(space::S2)
        .child(section_title(&p, "audio-title", "Sound").ml(space::S1))
        .child(card(&p, "audio-card", rows))
}

fn recording_words(app: &Ngine) -> String {
    match (&app.status.recording, &app.target) {
        (Some(recording), _) => {
            let markers = match recording.markers {
                0 => String::new(),
                1 => " · 1 marker".into(),
                many => format!(" · {many} markers"),
            };
            format!("Recording {}{markers}", recording.target.name)
        }
        (None, Some(target)) => format!("Ready to record {}", target.name),
        (None, None) => "Press Record to choose what to record".into(),
    }
}

fn record(ui: &Ui<'_>) -> El {
    let p = ui.p;
    let app = ui.app;
    let running = app.status.recording.as_ref();
    let elapsed = running.map_or(Duration::ZERO, |recording| recording.elapsed);
    let mut buttons = row().key("record-buttons").gap(space::S2).center_y();
    buttons = if running.is_some() {
        buttons
            .child(
                Button::new("marker", "Add marker")
                    .icon(&icons::FLAG)
                    .large()
                    .keys(hotkey_caps(app, hotkeys::MARKER))
                    .build(&p, |app| app.perform(Action::AddMarker)),
            )
            .child(
                Button::new("record-toggle", "Stop")
                    .icon(&icons::SQUARE)
                    .look(Look::Danger)
                    .large()
                    .keys(hotkey_caps(app, hotkeys::RECORD))
                    .build(&p, |app| app.perform(Action::ToggleRecording)),
            )
    } else {
        let blocked = app
            .picking
            .then(|| "Waiting for the system picker".to_owned());
        buttons.child(
            Button::new("record-toggle", "Record")
                .icon(&icons::CIRCLE)
                .look(Look::Primary)
                .large()
                .blocked(blocked)
                .keys(hotkey_caps(app, hotkeys::RECORD))
                .build(&p, |app| app.perform(Action::ToggleRecording)),
        )
    };
    let timer_colour = if running.is_some() { p.t.text } else { p.faint };
    let mut timer = row().key("elapsed-line").gap(space::S2).center_y();
    if running.is_some() {
        timer = timer.child(
            el().key("recording-dot")
                .size(10.0, 10.0)
                .radius(5.0)
                .fill(p.danger)
                .a11y_hidden(),
        );
    }
    timer = timer.child(
        text(format::timer(elapsed))
            .key("elapsed")
            .role("timer")
            .font(TIMER)
            .color(timer_colour),
    );
    let words = col()
        .key("record-words")
        .grow(1.0)
        .min_w(160.0)
        .gap(2.0)
        .child(timer)
        .child(caption(&p, "recording-state", recording_words(app)));
    let body = row()
        .key("record-body")
        .padding(space::S3 + space::S1)
        .gap(space::S3)
        .center_y()
        .flex_wrap()
        .child(words)
        .child(buttons);
    card(&p, "record", vec![body])
}

/// What a buffer of `length` seconds costs, as the length menus show it.
pub fn cost_words(app: &Ngine, length: u16) -> String {
    let cost = app.cost_of(length);
    format!(
        "≈ {} {}",
        format::megabytes(cost.megabytes),
        if cost.on_disk { "on disk" } else { "in memory" }
    )
}

fn replay_words(app: &Ngine) -> String {
    match &app.status.replay {
        Some(replay) => {
            let place = if replay.on_disk {
                "on disk"
            } else {
                "in memory"
            };
            format!(
                "Holding {} of {} · {} {place}",
                format::length(replay.held),
                format::length(replay.length),
                format::size(replay.bytes)
            )
        }
        None => format!(
            "Off. When on, keeps the last {} ready to save, {}.",
            seconds(app.settings.replay.length_seconds),
            cost_words(app, app.settings.replay.length_seconds)
        ),
    }
}

/// The menu of buffer lengths, each with what it costs.
pub fn length_select(ui: &mut Ui<'_>, key: &str, width: f32) -> El {
    let p = ui.p;
    let app = ui.app;
    let options = REPLAY_LENGTHS
        .iter()
        .map(|length| Option_::new(seconds(*length)).detail(cost_words(app, *length)))
        .collect();
    let chosen = Select {
        key,
        label: "Replay length",
        options,
        selected: REPLAY_LENGTHS
            .iter()
            .position(|length| *length == app.settings.replay.length_seconds),
        width,
    };
    select(&p, app, &mut ui.layer, chosen, |app, index| {
        if let Some(length) = REPLAY_LENGTHS.get(index) {
            app.edit(|settings| settings.replay.length_seconds = *length);
        }
    })
}

/// Why the replay cannot be saved now, if it cannot.
pub fn save_blocked(app: &Ngine) -> Option<String> {
    match &app.status.replay {
        None => Some(NO_REPLAY.into()),
        Some(replay) if replay.held.is_zero() => Some(NOTHING_HELD.into()),
        Some(_) => None,
    }
}

fn save_buttons(p: &Paint, app: &Ngine) -> El {
    let blocked = save_blocked(app);
    let save = |key: &str, label: &str, slot: usize, action: Action| {
        Button::new(key, label)
            .icon(&icons::HISTORY)
            .blocked(blocked.clone())
            .keys(hotkey_caps(app, slot))
            .build(p, move |app| app.perform(action))
    };
    let buttons = row()
        .key("save-buttons")
        .gap(space::S2)
        .flex_wrap()
        .child(save(
            "save-short",
            &format!("Save last {}", seconds(SHORT_SAVE_SECONDS)),
            hotkeys::SAVE_SHORT,
            Action::SaveShort,
        ))
        .child(save(
            "save-custom",
            &format!("Save last {}", seconds(app.settings.replay.save_seconds)),
            hotkeys::SAVE_CUSTOM,
            Action::SaveCustom,
        ))
        .child(save(
            "save-everything",
            "Save whole replay",
            hotkeys::SAVE_EVERYTHING,
            Action::SaveEverything,
        ));
    match blocked {
        Some(why) => col()
            .key("save-group")
            .gap(space::S2)
            .child(buttons)
            .child(caption(p, "save-blocked", format!("{why}."))),
        None => buttons,
    }
}

fn replay(ui: &mut Ui<'_>) -> El {
    let p = ui.p;
    let on = ui.app.status.replay.is_some();
    let length = length_select(ui, "replay-length", LENGTH_WIDTH);
    let app = ui.app;
    let head = row()
        .key("replay-head")
        .gap(space::S3)
        .center_y()
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
                        .color(p.t.text),
                )
                .child(caption(&p, "replay-state", replay_words(app))),
        )
        .child(length)
        .child(lone_switch(
            &p,
            "replay-toggle",
            "Instant replay",
            on,
            |app| app.perform(Action::ToggleReplay),
        ));
    let body = col()
        .key("replay-body")
        .padding(space::S3 + space::S1)
        .gap(space::S3)
        .child(head)
        .child(save_buttons(&p, app));
    card(&p, "replay", vec![body])
}

fn recent(ui: &Ui<'_>) -> Option<El> {
    let p = ui.p;
    let app = ui.app;
    let mut newest: Vec<usize> = (0..app.library.entries.len()).collect();
    newest.sort_by_key(|index| std::cmp::Reverse(app.library.entries[*index].item.modified));
    newest.truncate(RECENT_COUNT);
    if newest.is_empty() {
        return None;
    }
    let gap = space::S3;
    #[expect(
        clippy::cast_precision_loss,
        reason = "the strip holds three cards at most"
    )]
    let count = RECENT_COUNT as f32;
    let width = (ui.content_width() - gap * (count - 1.0)) / count;
    let mut strip = row().key("recent-strip").gap(gap);
    for (place, index) in newest.into_iter().enumerate() {
        let entry = &app.library.entries[index];
        let key = format!("recent-{place}");
        let length = entry.item.info.as_ref().map_or_else(
            || "Unreadable".to_owned(),
            |info| format::length(info.length),
        );
        strip = strip.child(
            col()
                .key(key.clone())
                .role("button")
                .label(format!("Open {}", entry.item.name))
                .w(width)
                .gap(space::S1 + 2.0)
                .padding(space::S1)
                .radius(radius::LG)
                .hover(p.t.hover)
                .press(p.t.press)
                .focusable(true)
                .on_press(move |app: &mut Ngine| app.open_in_editor(index))
                .child(
                    el().key(format!("{key}-thumbnail"))
                        .w_full()
                        .h((width - 2.0 * space::S1) * 9.0 / 16.0)
                        .radius(radius::MD)
                        .fill(Color::BLACK)
                        .pixels_fit(&entry.thumbnail, SurfaceFit::Fill)
                        .a11y_hidden(),
                )
                .child(
                    text(entry.item.name.clone())
                        .key(format!("{key}-name"))
                        .font(type_::LABEL)
                        .color(p.t.text)
                        .truncate()
                        .px(2.0)
                        .a11y_hidden(),
                )
                .child(
                    caption(
                        &p,
                        &format!("{key}-details"),
                        format!("{} · {}", length, entry.date),
                    )
                    .px(2.0)
                    .a11y_hidden(),
                ),
        );
    }
    Some(
        col()
            .key("recent")
            .gap(space::S2)
            .child(
                row()
                    .key("recent-head")
                    .center_y()
                    .child(section_title(&p, "recent-title", "Recent").ml(space::S1))
                    .child(spacer())
                    .child(
                        Button::new("recent-all", "Show all")
                            .look(Look::Quiet)
                            .build(&p, |app| app.show(Screen::Library)),
                    ),
            )
            .child(strip),
    )
}

pub fn view(ui: &mut Ui<'_>) -> El {
    let p = ui.p;
    let mut content = sheet(ui, "capture")
        .child(page_title(&p, "capture-title", "Capture"))
        .child(source(ui))
        .child(record(ui))
        .child(replay(ui))
        .child(audio(ui));
    if let Some(recent) = recent(ui) {
        content = content.child(recent);
    }
    page("capture", content)
}
