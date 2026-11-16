use std::time::Duration;

use rune::tok::{radius, space, type_};
use rune::{
    Align, Color, Cursor, El, Icon, ScrollSpec, Side, SurfaceFit, col, el, icon, row, spacer, text,
};

use super::widgets::{
    Button, CAPTION, Floating, ICON, Look, MenuItem, Option_, Paint, STRONG, Select, Stops, TIMER,
    caption, card, float, lone_switch, menu, menu_heading, menu_rule, meter, section_title, select,
    slider,
};
use super::{Ui, hotkey_caps, icons, page, sheet};
use crate::app::{Action, Ngine, Popup, Screen};
use crate::engine::{AudioSource, SystemSound, Target, TargetKind};
use crate::format;
use crate::hotkeys;
use crate::settings::{REPLAY_LENGTHS, SHORT_SAVE_SECONDS, Settings};

const SOURCE_TILE: f32 = 36.0;
const SOURCES_WIDTH: f32 = 380.0;
const SOURCES_HEIGHT: f32 = 420.0;
const AUDIO_LABEL: f32 = 104.0;
const VOLUME_WIDTH: f32 = 96.0;
const METER_WIDTH: f32 = 120.0;
const SHORT_METER_WIDTH: f32 = 64.0;
const SHORT_METERS_BELOW: f32 = 700.0;
const DEVICE_WIDTH: f32 = 220.0;
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
            target_detail(app, target),
        ),
        (None, false) => (
            &icons::MOUSE_POINTER_CLICK,
            "Nothing chosen yet".into(),
            "Choose a screen, a window or an app to record".into(),
        ),
    }
}

/// How many listed windows belong to the app `target`.
pub fn window_count(app: &Ngine, target: &Target) -> usize {
    app.sources
        .iter()
        .filter(|window| window.owner.as_deref() == Some(target.id.as_str()))
        .count()
}

fn windows_words(count: usize) -> String {
    match count {
        1 => "1 window".to_owned(),
        many => format!("{many} windows"),
    }
}

/// What the source line says under the name: what kind of thing it is and
/// what it holds.
fn target_detail(app: &Ngine, target: &Target) -> String {
    match target.kind {
        TargetKind::App => format!("App · all {}", windows_words(window_count(app, target))),
        TargetKind::Window | TargetKind::Screen
            if target.detail.is_empty() || target.detail == target.name =>
        {
            kind_word(target.kind).to_owned()
        }
        TargetKind::Window => format!("Window of {}", target.detail),
        TargetKind::Screen => format!("Screen · {}", target.detail),
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
    let action = if app.capabilities.lists_targets || app.target.is_some() {
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
        .px(space::S6 - 4.0)
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
            .cursor(Cursor::Pointer)
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
    line
}

fn elsewhere_tag(p: &Paint, key: &str) -> El {
    text("Hidden")
        .key(format!("{key}-elsewhere"))
        .font(CAPTION)
        .color(p.faint)
        .help("Minimised or on another desktop. It records all the same.")
}

fn window_item(app: &Ngine, p: &Paint, window: &Target, nested: bool) -> El {
    let key = format!("source-{}", window.id);
    let mut item = MenuItem::new(&key, &window.name).chosen(app.target.as_ref() == Some(window));
    if !nested {
        item = item
            .icon(kind_icon(TargetKind::Window))
            .detail(window.detail.clone());
    }
    let id = window.id.clone();
    let mut built = item.build(p, "menuitemradio", move |app, _| app.choose_source(&id));
    if nested {
        built = built.pl(space::S6 + space::S3);
    }
    if window.elsewhere {
        built = built
            .description("Minimised or on another desktop")
            .child(elsewhere_tag(p, &key));
    }
    built
}

/// The menu of what can be recorded: screens first, then each app with its
/// windows under it. Choosing the app records every window it has; choosing
/// one window records only that window. Where the system cannot record a
/// whole app, its name heads its windows instead.
fn sources_menu(ui: &mut Ui<'_>) {
    let p = ui.p;
    let mut items = screen_items(&p, ui.app);
    items.extend(app_items(&p, ui.app));

    let listed = col()
        .key("sources-list")
        .items_stretch()
        .max_h(SOURCES_HEIGHT.min(ui.window.1 - 172.0).max(120.0))
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .children(items);
    let list = menu(
        &p,
        "sources",
        "Sources",
        vec![
            listed,
            menu_rule(&p, "sources-rule"),
            MenuItem::new("source-picker", "Choose with the system picker…")
                .icon(&icons::MOUSE_POINTER_CLICK)
                .build(&p, "menuitem", |app, _| app.pick()),
        ],
    )
    .w(SOURCES_WIDTH.min(ui.content_width()));
    let floating = Floating {
        key: "sources",
        anchor: "source",
        side: Side::Bottom,
        align: Align::Start,
    };
    float(&mut ui.layer, &floating, list);
}

fn screen_items(p: &Paint, app: &Ngine) -> Vec<El> {
    let p = *p;
    let mut items = Vec::new();
    let screens: Vec<(usize, &Target)> = app
        .sources
        .iter()
        .enumerate()
        .filter(|(_, target)| target.kind == TargetKind::Screen)
        .collect();
    if !screens.is_empty() {
        items.push(menu_heading(&p, "sources-screens", "Screens"));
        for (_, target) in screens {
            let key = format!("source-{}", target.id);
            let id = target.id.clone();
            items.push(
                MenuItem::new(&key, &target.name)
                    .icon(kind_icon(TargetKind::Screen))
                    .detail(target.detail.clone())
                    .chosen(app.target.as_ref() == Some(target))
                    .build(&p, "menuitemradio", move |app, _| app.choose_source(&id)),
            );
        }
    }
    items
}

fn app_items(p: &Paint, app: &Ngine) -> Vec<El> {
    let p = *p;
    let mut items = Vec::new();
    let windows: Vec<(usize, &Target)> = app
        .sources
        .iter()
        .enumerate()
        .filter(|(_, target)| target.kind == TargetKind::Window)
        .collect();
    let apps: Vec<(usize, &Target)> = app
        .sources
        .iter()
        .enumerate()
        .filter(|(_, target)| target.kind == TargetKind::App)
        .collect();
    if !apps.is_empty() || !windows.is_empty() {
        items.push(menu_heading(&p, "sources-apps", "Apps and windows"));
    }
    for (_, owner) in &apps {
        let key = format!("source-{}", owner.id);
        let id = owner.id.clone();
        let count = window_count(app, owner);
        items.push(
            MenuItem::new(&key, &owner.name)
                .icon(kind_icon(TargetKind::App))
                .detail(windows_words(count))
                .chosen(app.target.as_ref() == Some(*owner))
                .build(&p, "menuitemradio", move |app, _| app.choose_source(&id)),
        );
        let own = windows
            .iter()
            .filter(|(_, window)| window.owner.as_deref() == Some(owner.id.as_str()));
        for (_, window) in own.filter(|_| count > 1) {
            items.push(window_item(app, &p, window, true));
        }
    }
    let unowned = |window: &&(usize, &Target)| {
        window
            .1
            .owner
            .as_deref()
            .is_none_or(|owner| apps.iter().all(|(_, app)| app.id != owner))
    };
    let mut heading: Option<&str> = None;
    for (index, window) in windows.iter().filter(unowned) {
        if apps.is_empty() && heading != Some(window.detail.as_str()) && !window.detail.is_empty() {
            heading = Some(window.detail.as_str());
            items.push(
                text(window.detail.clone())
                    .key(format!("sources-app-{index}"))
                    .font(type_::LABEL)
                    .color(p.t.text_secondary)
                    .truncate()
                    .px(space::S2)
                    .pt(space::S2)
                    .pb(2.0),
            );
            items.push(window_item(app, &p, window, true));
        } else {
            items.push(window_item(app, &p, window, apps.is_empty()));
        }
    }
    items
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

/// The microphone menu: the system's default first, which follows the
/// system when it changes, then every microphone it offers.
pub fn microphone_select(ui: &mut Ui<'_>, key: &str, width: f32, align: Align) -> El {
    let p = ui.p;
    let app = ui.app;
    let default_name = app
        .microphones
        .iter()
        .find(|device| device.is_default)
        .map(|device| device.name.clone());
    let mut options = vec![match &default_name {
        Some(name) => Option_::new("System default").detail(name.clone()),
        None => Option_::new("System default"),
    }];
    options.extend(
        app.microphones
            .iter()
            .map(|device| Option_::new(device.name.clone())),
    );
    let ids: Vec<String> = app
        .microphones
        .iter()
        .map(|device| device.id.clone())
        .collect();
    let selected = match (&app.settings.inputs.microphone, app.missing_microphone()) {
        (None, _) | (Some(_), Some(_)) => Some(0),
        (Some(id), None) => ids.iter().position(|one| one == id).map(|at| at + 1),
    };
    let chosen = Select {
        key,
        label: "Microphone",
        options,
        selected,
        width,
        align,
    };
    select(&p, app, &mut ui.layer, chosen, move |app, index| {
        let device = index.checked_sub(1).and_then(|at| ids.get(at)).cloned();
        app.edit(|settings| settings.inputs.microphone = device);
    })
}

pub fn system_sound_words(sound: &SystemSound) -> String {
    match sound {
        SystemSound::Everything => "All system sound".to_owned(),
        SystemSound::App { name, .. } => format!("Only {name}"),
        SystemSound::Output(device) => device.name.clone(),
    }
}

/// The system sound menu: all of it, or what the system can separate: one
/// app's sound, or one output device's.
pub fn system_sound_select(ui: &mut Ui<'_>, key: &str, width: f32, align: Align) -> El {
    let p = ui.p;
    let app = ui.app;
    let mut choices = vec![SystemSound::Everything];
    choices.extend(app.system_sounds.iter().cloned());
    let wanted = &app.settings.inputs.system;
    if app.missing_system_sound().is_some() {
        choices.push(wanted.clone());
    }
    let selected = choices.iter().position(|one| match (one, wanted) {
        (SystemSound::App { name: a, .. }, SystemSound::App { name: b, .. }) => a == b,
        (SystemSound::Output(a), SystemSound::Output(b)) => a.id == b.id,
        (one, wanted) => one == wanted,
    });
    let options = choices
        .iter()
        .map(|sound| Option_::new(system_sound_words(sound)))
        .collect();
    let chosen = Select {
        key,
        label: "System sound",
        options,
        selected,
        width,
        align,
    };
    select(&p, app, &mut ui.layer, chosen, move |app, index| {
        if let Some(sound) = choices.get(index).cloned() {
            app.edit(|settings| settings.inputs.system = sound);
        }
    })
}

struct AudioRow {
    key: &'static str,
    label: &'static str,
    glyph: &'static Icon,
    source: AudioSource,
    level: Option<f32>,
    device: El,
    flip: fn(&mut Settings),
    set_volume: fn(&mut Settings, u8),
}

fn audio_row(p: &Paint, narrow: bool, meter_width: f32, audio: AudioRow) -> El {
    let AudioRow {
        key,
        label,
        glyph,
        source,
        level,
        device,
        flip,
        set_volume,
    } = audio;
    let on = source.enabled;
    let shown = if on { 1.0 } else { OFF };
    let mut line = row()
        .key(key)
        .min_h(48.0)
        .pl(space::S3 + space::S1)
        .pr(space::S3 + space::S1)
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
            el().key(format!("{key}-device-box"))
                .grow(1.0)
                .min_w(0.0)
                .opacity(shown)
                .child(device),
        );
    if let Some(level) = level.filter(|_| on) {
        line = line.child(
            el().key(format!("{key}-meter-box"))
                .w(meter_width)
                .child(meter(
                    p,
                    &format!("{key}-meter"),
                    &format!("{label} level"),
                    level,
                )),
        );
    }
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

fn audio(ui: &mut Ui<'_>) -> El {
    let narrow = ui.content_width() < 600.0;
    let metered = ui.app.status.levels.is_some();
    let meter_width = if ui.content_width() < SHORT_METERS_BELOW {
        SHORT_METER_WIDTH
    } else {
        METER_WIDTH
    };
    let beside = 2.0 * space::S3
        + 2.0
        + 16.0
        + AUDIO_LABEL
        + if metered {
            meter_width + space::S3
        } else {
            0.0
        }
        + 36.0
        + 4.0 * space::S3
        + if narrow {
            0.0
        } else {
            VOLUME_WIDTH + space::S2 + 40.0 + space::S3
        };
    let device_width = (ui.content_width() - beside).clamp(96.0, DEVICE_WIDTH);
    let system_device = system_sound_select(ui, "capture-system-sound", device_width, Align::Start);
    let microphone_device = microphone_select(ui, "capture-microphone", device_width, Align::Start);
    let p = ui.p;
    let app = ui.app;
    let audio = app.settings.audio;
    let live = app.status.levels.is_some();
    let meters = app.meters;
    let rows = vec![
        audio_row(
            &p,
            narrow,
            meter_width,
            AudioRow {
                key: "audio-system",
                label: "System sound",
                glyph: &icons::VOLUME_2,
                source: audio.system,
                level: live.then_some(meters.system),
                device: system_device,
                flip: |settings| settings.audio.system.enabled ^= true,
                set_volume: |settings, percent| settings.audio.system.volume_percent = percent,
            },
        ),
        audio_row(
            &p,
            narrow,
            meter_width,
            AudioRow {
                key: "audio-microphone",
                label: "Microphone",
                glyph: &icons::MIC,
                source: audio.microphone,
                level: live.then_some(meters.microphone),
                device: microphone_device,
                flip: |settings| settings.audio.microphone.enabled ^= true,
                set_volume: |settings, percent| settings.audio.microphone.volume_percent = percent,
            },
        ),
    ];
    card(&p, "audio", rows)
}

fn recording_words(app: &Ngine) -> String {
    match (&app.status.recording, &app.target) {
        (Some(recording), _) => match recording.markers {
            0 => String::new(),
            1 => "1 marker".into(),
            many => format!("{many} markers"),
        },
        (None, _) => String::new(),
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
    let mut words = col()
        .key("record-words")
        .grow(1.0)
        .min_w(160.0)
        .min_h(44.0)
        .justify_center()
        .gap(2.0)
        .child(timer);
    if running.is_some_and(|recording| recording.markers > 0) {
        words = words.child(caption(&p, "recording-state", recording_words(app)));
    }
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
        None => cost_words(app, app.settings.replay.length_seconds),
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
        align: Align::End,
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
    row()
        .key("save-buttons")
        .gap(space::S2)
        .flex_wrap()
        .child(save(
            "save-short",
            &format!("Save last {}", seconds(SHORT_SAVE_SECONDS)),
            hotkeys::SAVE_SHORT,
            Action::SaveShort,
        ))
        .children(
            (app.settings.replay.save_seconds < app.settings.replay.length_seconds).then(|| {
                save(
                    "save-custom",
                    &format!("Save last {}", seconds(app.settings.replay.save_seconds)),
                    hotkeys::SAVE_CUSTOM,
                    Action::SaveCustom,
                )
            }),
        )
        .child(save(
            "save-everything",
            "Save whole replay",
            hotkeys::SAVE_EVERYTHING,
            Action::SaveEverything,
        ))
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
                .pb(space::S1)
                .radius(radius::LG)
                .hover(p.t.hover)
                .press(p.t.press)
                .focusable(true)
                .cursor(Cursor::Pointer)
                .on_press(move |app: &mut Ngine| app.open_in_editor(index))
                .child(
                    el().key(format!("{key}-thumbnail"))
                        .w_full()
                        .h(width * 9.0 / 16.0)
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
                        .a11y_hidden(),
                )
                .child(
                    caption(
                        &p,
                        &format!("{key}-details"),
                        format!("{} · {}", length, entry.date),
                    )
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
                    .child(section_title(&p, "recent-title", "Recent"))
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
    let mut content = sheet(ui, "capture")
        .gap(space::S3 + space::S1)
        .child(source(ui))
        .child(record(ui))
        .child(replay(ui))
        .child(audio(ui));
    if let Some(recent) = recent(ui) {
        content = content.child(recent);
    }
    page("capture", content)
}
