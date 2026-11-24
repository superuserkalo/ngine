use rune::tok::{radius, space};
use rune::{Align, Color, Cursor, El, Icon, PhysicalInput, col, el, icon, row, text};

use super::capture::{microphone_select, system_sound_select, volume};
use super::widgets::{
    Button, CONTROL, Choice, Look, NUMBERS, Paint, STRONG, Segments, amount_field, caption, dialog,
    keycaps, section, segmented, setting, toggle, troubled_setting, warned_setting,
};
use super::{EARLY_NOTE, SIMULATED_NOTE, Ui, choose_folder, icons};
use crate::app::{Amount, Folder, NAME, Ngine, Popup, Section, VERSION};
use crate::engine::{Codec, Quality, ReplayStorage, SystemSound, Tracks};
use crate::format;
use crate::hotkeys;
use crate::platform;

const LIST_WIDTH: f32 = 176.0;
const NARROW_LIST_WIDTH: f32 = 148.0;
const LIST_ITEM: f32 = 30.0;
const SELECT_WIDTH: f32 = 200.0;
const AMOUNT_WIDTH: f32 = 148.0;
const HOTKEY_WIDTH: f32 = 160.0;
const SLIDER_WIDTH: f32 = 120.0;
const SHEET_MARGIN: f32 = 32.0;
const WIDEST_SHEET: f32 = 860.0;
const TALLEST_SHEET: f32 = 640.0;
const SHEET_RADIUS: f32 = 14.0;

pub const NOT_SET: &str = "Not set";
pub const WAITING_FOR_KEYS: &str = "Press keys…";

/// What the hotkey recorder says while it listens.
pub fn recorder_hint() -> String {
    format!("Esc cancels, {} clears", platform::ERASE_KEY)
}

pub fn section_label(section: Section) -> &'static str {
    match section {
        Section::Video => "Video",
        Section::Audio => "Audio",
        Section::Replay => "Instant replay",
        Section::Hotkeys => "Hotkeys",
        Section::Storage => "Storage",
        Section::General => "General",
        Section::About => "About",
    }
}

fn section_id(section: Section) -> &'static str {
    match section {
        Section::Video => "video",
        Section::Audio => "audio",
        Section::Replay => "replay",
        Section::Hotkeys => "hotkeys",
        Section::Storage => "storage",
        Section::General => "general",
        Section::About => "about",
    }
}

fn section_icon(section: Section) -> &'static Icon {
    match section {
        Section::Video => &icons::VIDEO,
        Section::Audio => &icons::AUDIO_LINES,
        Section::Replay => &icons::HISTORY,
        Section::Hotkeys => &icons::KEYBOARD,
        Section::Storage => &icons::HARD_DRIVE,
        Section::General => &icons::SLIDERS_HORIZONTAL,
        Section::About => &icons::INFO,
    }
}

fn list(ui: &Ui<'_>, width: f32) -> El {
    let p = &ui.p;
    let mut items = col()
        .key("sections")
        .role("tablist")
        .label("Settings sections")
        .focus_group()
        .gap(2.0);
    for section in Section::ALL {
        let current = ui.app.section == section;
        let ink = if current {
            p.t.text
        } else {
            p.t.text_secondary
        };
        let mut tab = row();
        if current {
            tab = tab.autofocus();
        }
        items = items.child(
            tab.key(format!("section-{}", section_id(section)))
                .role("tab")
                .label(section_label(section))
                .checked(current)
                .h(LIST_ITEM)
                .px(space::S2)
                .gap(space::S2)
                .center_y()
                .radius(radius::SM)
                .fill(if current {
                    p.t.control_off
                } else {
                    Color::TRANSPARENT
                })
                .hover(p.t.hover)
                .press(p.t.press)
                .focusable(true)
                .cursor(Cursor::Pointer)
                .on_press(move |app: &mut Ngine| app.show_section(section))
                .child(icon(section_icon(section), 15.0).color(ink).a11y_hidden())
                .child(
                    text(section_label(section))
                        .font(super::widgets::LABEL)
                        .color(ink)
                        .truncate()
                        .a11y_hidden(),
                ),
        );
    }
    col()
        .key("sections-side")
        .w(width)
        .h_full()
        .px(space::S2)
        .py(space::S3)
        .gap(space::S3)
        .fill(p.sidebar)
        .border_sides([0.0, p.hair, 0.0, 0.0], p.t.separator)
        .child(
            Button::new("settings-close", "Close")
                .only_icon(&icons::X)
                .look(Look::Quiet)
                .help("Close (Esc)")
                .build(p, Ngine::close_settings),
        )
        .child(items)
}

pub fn codec_label(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "H.264",
        Codec::Hevc => "HEVC",
        Codec::Av1 => "AV1",
    }
}

fn codec_id(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
        Codec::Av1 => "av1",
    }
}

fn quality_label(quality: Quality) -> &'static str {
    match quality {
        Quality::Low => "Low",
        Quality::Medium => "Medium",
        Quality::High => "High",
        Quality::Highest => "Highest",
    }
}

fn quality_id(quality: Quality) -> &'static str {
    match quality {
        Quality::Low => "low",
        Quality::Medium => "medium",
        Quality::High => "high",
        Quality::Highest => "highest",
    }
}

fn megabytes_a_minute(app: &Ngine) -> String {
    format!(
        "About {} a minute",
        format::megabytes(app.megabytes_per_minute())
    )
}

fn amount(ui: &mut Ui<'_>, amount: Amount, width: f32) -> El {
    let p = ui.p;
    let app = ui.app;
    amount_field(&p, app, &mut ui.layer, amount, width)
}

fn amount_row(ui: &mut Ui<'_>, key: &str, label: &str, which: Amount, hint: Option<String>) -> El {
    let field = amount(ui, which, AMOUNT_WIDTH);
    let p = ui.p;
    let problem = ui
        .app
        .draft
        .as_ref()
        .filter(|draft| draft.amount == which && draft.typed())
        .and_then(|draft| draft.problem(&ui.app.settings));
    match problem {
        Some(problem) => troubled_setting(&p, key, label, problem, field),
        None => setting(&p, key, label, hint, field),
    }
}

fn video(ui: &mut Ui<'_>) -> Vec<El> {
    let p = ui.p;
    let app = ui.app;
    let video = app.settings.video;
    let codecs = app.capabilities.codecs.clone();
    let codec = segmented(
        &p,
        Segments {
            key: "codec",
            label: "Codec",
            choices: codecs
                .iter()
                .map(|codec| Choice::new(codec_id(*codec), codec_label(*codec)))
                .collect(),
            selected: codecs.iter().position(|codec| *codec == video.codec),
            segment_width: 64.0,
        },
        move |app, index| {
            if let Some(codec) = codecs.get(index) {
                app.edit(|settings| settings.video.codec = *codec);
            }
        },
    );
    let quality = segmented(
        &p,
        Segments {
            key: "quality",
            label: "Quality",
            choices: Quality::ALL
                .iter()
                .map(|quality| Choice::new(quality_id(*quality), quality_label(*quality)))
                .collect(),
            selected: Quality::ALL.iter().position(|one| *one == video.quality),
            segment_width: 72.0,
        },
        |app, index| {
            if let Some(quality) = Quality::ALL.get(index) {
                app.edit(|settings| settings.video.quality = *quality);
            }
        },
    );
    let rate = amount_row(ui, "frame-rate-row", "Frame rate", Amount::FrameRate, None);
    vec![section(
        &p,
        "video",
        "",
        vec![
            setting(&p, "codec-row", "Codec", None, codec),
            setting(
                &p,
                "quality-row",
                "Quality",
                Some(megabytes_a_minute(app)),
                quality,
            ),
            rate,
        ],
    )]
}

fn audio(ui: &mut Ui<'_>) -> Vec<El> {
    let p = ui.p;
    let microphone = microphone_select(ui, "settings-microphone", SELECT_WIDTH, Align::End);
    let system = system_sound_select(ui, "settings-system-sound", SELECT_WIDTH, Align::End);
    let app = ui.app;
    let audio = app.settings.audio;
    let tracks = segmented(
        &p,
        Segments {
            key: "tracks",
            label: "Tracks in a recording",
            choices: Tracks::ALL
                .iter()
                .map(|tracks| match tracks {
                    Tracks::Three => Choice::new("three", "Separate"),
                    Tracks::MixOnly => Choice::new("mix", "One mix"),
                })
                .collect(),
            selected: Tracks::ALL.iter().position(|one| *one == audio.tracks),
            segment_width: 80.0,
        },
        |app, index| {
            if let Some(tracks) = Tracks::ALL.get(index) {
                app.edit(|settings| settings.audio.tracks = *tracks);
            }
        },
    );
    let everything = app.settings.inputs.system == SystemSound::Everything;
    let missing_system = app
        .missing_system_sound()
        .map(|_| "The chosen app is not running, so all system sound records".to_owned());
    vec![
        section(
            &p,
            "system-sound-section",
            "System sound",
            vec![
                toggle(
                    &p,
                    "system-sound",
                    "Record",
                    None,
                    audio.system.enabled,
                    |app| app.edit(|settings| settings.audio.system.enabled ^= true),
                ),
                device_row(&p, "system-source-row", "Source", missing_system, system),
                setting(
                    &p,
                    "system-volume-row",
                    "Volume",
                    None,
                    volume(
                        &p,
                        "system-volume",
                        "System sound volume",
                        audio.system.volume_percent,
                        SLIDER_WIDTH,
                        |settings, percent| settings.audio.system.volume_percent = percent,
                    ),
                ),
            ]
            .into_iter()
            .chain(everything.then(|| app_sound_row(&p, audio.app_only)))
            .collect(),
        ),
        microphone_section(&p, app, microphone),
        section(
            &p,
            "mixing",
            "",
            vec![setting(
                &p,
                "tracks-row",
                "Tracks in a recording",
                Some("Separate keeps system sound and microphone apart for editing".into()),
                tracks,
            )],
        ),
    ]
}

fn app_sound_row(p: &Paint, app_only: bool) -> El {
    let choice = segmented(
        p,
        Segments {
            key: "app-only",
            label: "When recording a window or an app",
            choices: vec![
                Choice::new("its", "Only that app"),
                Choice::new("all", "All sound"),
            ],
            selected: Some(usize::from(!app_only)),
            segment_width: 80.0,
        },
        |app, index| app.edit(|settings| settings.audio.app_only = index == 0),
    );
    setting(
        p,
        "app-only-row",
        "When recording a window or an app",
        None,
        choice,
    )
}

fn device_row(p: &Paint, key: &str, label: &str, missing: Option<String>, control: El) -> El {
    match missing {
        Some(why) => warned_setting(p, key, label, why, control),
        None => setting(p, key, label, None, control),
    }
}

fn microphone_section(p: &Paint, app: &Ngine, microphone: El) -> El {
    let p = *p;
    let audio = app.settings.audio;
    let missing_microphone = app
        .missing_microphone_name()
        .map(|name| format!("{name} is not connected, so the system default records"));
    section(
        &p,
        "microphone-section",
        "Microphone",
        vec![
            toggle(
                &p,
                "microphone",
                "Record",
                None,
                audio.microphone.enabled,
                |app| app.edit(|settings| settings.audio.microphone.enabled ^= true),
            ),
            device_row(
                &p,
                "microphone-device-row",
                "Device",
                missing_microphone,
                microphone,
            ),
            setting(
                &p,
                "microphone-volume-row",
                "Volume",
                None,
                volume(
                    &p,
                    "microphone-volume",
                    "Microphone volume",
                    audio.microphone.volume_percent,
                    SLIDER_WIDTH,
                    |settings, percent| settings.audio.microphone.volume_percent = percent,
                ),
            ),
        ],
    )
}

fn storage_label(storage: ReplayStorage) -> &'static str {
    match storage {
        ReplayStorage::Auto => "Automatic",
        ReplayStorage::Memory => "Memory",
        ReplayStorage::Disk => "Disk",
    }
}

fn storage_id(storage: ReplayStorage) -> &'static str {
    match storage {
        ReplayStorage::Auto => "auto",
        ReplayStorage::Memory => "memory",
        ReplayStorage::Disk => "disk",
    }
}

fn storage_hint(storage: ReplayStorage) -> &'static str {
    match storage {
        ReplayStorage::Auto => "In memory up to the budget, on disk beyond it",
        ReplayStorage::Memory => "Always in memory, whatever the budget",
        ReplayStorage::Disk => "Always in short files in the buffer folder",
    }
}

fn replay(ui: &mut Ui<'_>) -> Vec<El> {
    let p = ui.p;
    let app = ui.app;
    let replay = &app.settings.replay;
    let cost = super::capture::cost_words(app, replay.length_seconds);
    let length = amount_row(
        ui,
        "replay-length-row",
        "Keep the last",
        Amount::ReplayLength,
        Some(cost),
    );
    let storage = segmented(
        &p,
        Segments {
            key: "storage",
            label: "Where the buffer is kept",
            choices: ReplayStorage::ALL
                .iter()
                .map(|storage| Choice::new(storage_id(*storage), storage_label(*storage)))
                .collect(),
            selected: ReplayStorage::ALL
                .iter()
                .position(|one| *one == replay.storage),
            segment_width: 80.0,
        },
        |app, index| {
            if let Some(storage) = ReplayStorage::ALL.get(index) {
                app.edit(|settings| settings.replay.storage = *storage);
            }
        },
    );
    let storage_kind = replay.storage;
    let clear = replay.clear_after_save;
    let at_start = replay.start_with_app;
    let mut buffer = vec![
        length,
        setting(
            &p,
            "storage-row",
            "Where it is kept",
            Some(storage_hint(storage_kind).into()),
            storage,
        ),
    ];
    if storage_kind == ReplayStorage::Auto {
        buffer.push(amount_row(
            ui,
            "memory-budget-row",
            "Memory budget",
            Amount::MemoryBudget,
            None,
        ));
    }
    let save = amount_row(
        ui,
        "save-length-row",
        "Custom save length",
        Amount::SaveLength,
        None,
    );
    vec![
        section(&p, "buffer", "Buffer", buffer),
        section(
            &p,
            "saving",
            "Saving",
            vec![
                save,
                toggle(
                    &p,
                    "clear-after-save",
                    "Clear the buffer after a save",
                    None,
                    clear,
                    |app| app.edit(|settings| settings.replay.clear_after_save ^= true),
                ),
                toggle(
                    &p,
                    "replay-at-start",
                    "Turn on when ngine starts",
                    None,
                    at_start,
                    |app| app.edit(|settings| settings.replay.start_with_app ^= true),
                ),
            ],
        ),
    ]
}

fn written_alone(accelerator: &str) -> String {
    let written = accelerator.trim();
    if written.chars().count() == 1 {
        written.to_uppercase()
    } else {
        written.to_owned()
    }
}

fn hotkey_face(p: &Paint, app: &Ngine, slot: usize, key: &str) -> El {
    if let Some(capture) = app.capture.as_ref().filter(|capture| capture.slot == slot) {
        let held = rune::modifier_keycaps(capture.held);
        if held.is_empty() {
            return text(WAITING_FOR_KEYS)
                .key(format!("{key}-waiting"))
                .font(super::widgets::LABEL)
                .color(p.accent)
                .a11y_hidden();
        }
        return keycaps(p, &format!("{key}-held"), &held).a11y_hidden();
    }
    let accelerator = app.settings.hotkeys.all()[slot];
    match hotkeys::keycaps(accelerator) {
        Some(caps) => keycaps(p, &format!("{key}-caps"), &caps).a11y_hidden(),
        None if !hotkeys::is_unset(accelerator) => text(written_alone(accelerator))
            .key(format!("{key}-written"))
            .font(super::widgets::LABEL)
            .color(p.danger)
            .truncate()
            .a11y_hidden(),
        None => text(NOT_SET)
            .key(format!("{key}-unset"))
            .font(super::widgets::LABEL)
            .color(p.faint)
            .a11y_hidden(),
    }
}

fn hotkey(p: &Paint, app: &Ngine, slot: usize) -> El {
    let key = format!("hotkey-{slot}");
    let label = hotkeys::label(slot, &app.settings);
    let capturing = app.is_capturing(slot);
    let accelerator = app.settings.hotkeys.all()[slot];
    let current =
        hotkeys::keycaps(accelerator).map_or_else(|| NOT_SET.to_owned(), |caps| caps.join(" "));
    let recorder = row()
        .key(key.clone())
        .role("button")
        .label(format!("{label}: {current}"))
        .help("Click, then press the new keys")
        .w(HOTKEY_WIDTH)
        .h(CONTROL)
        .px(space::S2)
        .center()
        .radius(radius::MD)
        .fill(if capturing {
            p.accent.alpha(0.1)
        } else {
            p.t.control_track
        })
        .border(
            if capturing { 1.5 } else { p.hair },
            if capturing { p.accent } else { p.outline },
        )
        .hover(p.t.hover)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press(move |app: &mut Ngine| {
            if app.is_capturing(slot) {
                app.cancel_hotkey();
            } else {
                app.begin_hotkey(slot);
            }
        })
        .on_physical_input(move |app: &mut Ngine, input: &PhysicalInput, _| {
            app.hotkey_input(slot, *input)
        })
        .child(hotkey_face(p, app, slot, &key));
    let reset = if app.hotkey_changed(slot) {
        Button::new(&format!("{key}-reset"), "Reset to default")
            .only_icon(&icons::ROTATE_CCW)
            .look(Look::Quiet)
            .build(p, move |app| app.reset_hotkey(slot))
    } else {
        el().key(format!("{key}-reset-space"))
            .size(CONTROL, CONTROL)
            .a11y_hidden()
    };
    let clear = if hotkeys::is_unset(accelerator) {
        el().key(format!("{key}-clear-space"))
            .size(CONTROL, CONTROL)
            .a11y_hidden()
    } else {
        Button::new(&format!("{key}-clear"), "Clear hotkey")
            .only_icon(&icons::X)
            .look(Look::Quiet)
            .build(p, move |app| app.clear_hotkey(slot))
    };
    let control = row()
        .key(format!("{key}-control"))
        .gap(2.0)
        .center_y()
        .child(recorder)
        .child(reset)
        .child(clear);
    let row_key = format!("{key}-row");
    let refusal = app
        .capture
        .as_ref()
        .filter(|capture| capture.slot == slot)
        .and_then(|capture| capture.refusal.clone());
    match (refusal, app.hotkey_problems[slot].clone()) {
        (Some(trouble), _) | (None, Some(trouble)) => {
            troubled_setting(p, &row_key, &label, trouble, control)
        }
        (None, None) if capturing => setting(p, &row_key, &label, Some(recorder_hint()), control),
        (None, None) => setting(p, &row_key, &label, None, control),
    }
}

pub const BIND_IN_THE_DESKTOP: &str = "Bind keys in the desktop's own settings to ngine's commands, such as ngine replay save --last 30s.";

fn hotkeys_section(ui: &Ui<'_>) -> Vec<El> {
    let p = &ui.p;
    let mut shown = vec![section(
        p,
        "hotkeys",
        "",
        (0..hotkeys::COUNT)
            .map(|slot| hotkey(p, ui.app, slot))
            .collect(),
    )];
    let not_offered = ui
        .app
        .hotkey_problems
        .iter()
        .flatten()
        .any(|problem| problem == hotkeys::NOT_OFFERED);
    if not_offered {
        shown.push(
            caption(p, "hotkeys-commands", BIND_IN_THE_DESKTOP)
                .max_w(520.0)
                .mx(space::S1),
        );
    }
    shown
}

fn folder_row(p: &Paint, key: &str, label: &str, folder: Folder, path: &std::path::Path) -> El {
    let opened = path.to_owned();
    let shown = path.display().to_string();
    let place = row()
        .key(format!("{key}-path"))
        .role("button")
        .label(format!("Open {shown} in {}", platform::FILE_MANAGER))
        .help(format!("Open in {}", platform::FILE_MANAGER))
        .h(CONTROL)
        .min_w(0.0)
        .shrink()
        .px(space::S2)
        .gap(space::S2)
        .center_y()
        .radius(radius::MD)
        .fill(p.t.control_track)
        .border(p.hair, p.outline)
        .hover(p.t.hover)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press(move |app: &mut Ngine| app.open_folder(&opened))
        .child(icon(&icons::FOLDER_OPEN, 14.0).color(p.faint).a11y_hidden())
        .child(
            text(shown)
                .key(format!("{key}-value"))
                .font(super::widgets::LABEL)
                .color(p.t.text)
                .truncate_middle()
                .min_w(0.0)
                .a11y_hidden(),
        );
    let controls = row()
        .key(format!("{key}-controls"))
        .gap(space::S2)
        .center_y()
        .w(300.0)
        .shrink()
        .min_w(0.0)
        .justify_end()
        .child(place)
        .child(Button::new(&format!("{key}-change"), "Change…").build_tx(p, choose_folder(folder)));
    setting(p, key, label, None, controls)
}

fn room(minutes: u64) -> String {
    match minutes {
        0..120 => format!("{minutes} min"),
        _ => format!("{} hours", minutes / 60),
    }
}

fn storage(ui: &Ui<'_>) -> Vec<El> {
    let p = &ui.p;
    let app = ui.app;
    let free = app.free_space.map_or_else(
        || "Unknown".to_owned(),
        |bytes| format!("{} free", format::free_space(bytes)),
    );
    let minutes = app.free_space.and_then(|bytes| {
        let per_minute = app.megabytes_per_minute().max(1) * 1_000_000;
        (bytes / per_minute > 0).then(|| {
            format!(
                "Room for about {} of recording at this quality",
                room(bytes / per_minute)
            )
        })
    });
    vec![section(
        p,
        "folders",
        "",
        vec![
            folder_row(
                p,
                "output-folder",
                "Recordings and clips",
                Folder::Output,
                &app.settings.output_folder,
            ),
            setting(
                p,
                "free-space",
                "Free space",
                minutes,
                text(free)
                    .key("free-space-value")
                    .font(NUMBERS)
                    .color(p.t.text_secondary),
            ),
            folder_row(
                p,
                "buffer-folder",
                "Instant replay buffer",
                Folder::Buffer,
                &app.settings.replay.buffer_folder,
            ),
        ],
    )]
}

fn general(ui: &Ui<'_>) -> Vec<El> {
    let p = &ui.p;
    vec![section(
        p,
        "startup",
        "",
        vec![toggle(
            p,
            "start-at-login",
            "Start ngine when you log in",
            None,
            ui.app.settings.start_at_login,
            |app| app.edit(|settings| settings.start_at_login ^= true),
        )],
    )]
}

fn about(ui: &Ui<'_>) -> Vec<El> {
    let p = &ui.p;
    let mut words = col()
        .key("about-words")
        .gap(space::S1)
        .child(
            text(format!("{NAME} {VERSION}"))
                .key("about-version")
                .font(STRONG)
                .color(p.t.text),
        )
        .child(caption(p, "about-note", EARLY_NOTE));
    if ui.app.capabilities.simulated {
        words = words.child(caption(p, "about-simulated", SIMULATED_NOTE));
    }
    words = words.child(caption(
        p,
        "about-licences",
        "Free software under the MIT licence. Icons from Lucide, under the ISC licence.",
    ));
    vec![words]
}

fn reset_question(ui: &mut Ui<'_>, section: Section) {
    let p = ui.p;
    let actions = vec![
        Button::new("reset-cancel", "Cancel").build(&p, Ngine::close_popup),
        Button::new("reset-confirm", "Reset")
            .look(Look::Danger)
            .build(&p, move |app| app.reset_section(section)),
    ];
    dialog(
        &p,
        &mut ui.layer,
        "reset-ask",
        &format!(
            "Reset the {} settings?",
            section_label(section).to_lowercase()
        ),
        "Only this section changes.".into(),
        actions,
    );
}

fn close(app: &mut Ngine) {
    app.close_settings();
}

/// The settings, in a sheet over the window: the sections down the side,
/// the chosen one beside them. Escape, the close button and a press outside
/// close it.
pub fn sheet(ui: &mut Ui<'_>) {
    let p = ui.p;
    let start = ui.layer.len();
    let section = ui.app.section;
    let rows = match section {
        Section::Video => video(ui),
        Section::Audio => audio(ui),
        Section::Replay => replay(ui),
        Section::Hotkeys => hotkeys_section(ui),
        Section::Storage => storage(ui),
        Section::General => general(ui),
        Section::About => about(ui),
    };
    if let Some(Popup::ConfirmReset(asked)) = ui.app.popup {
        reset_question(ui, asked);
    }
    let floating = ui.layer.split_off(start);
    let centred = (ui.window.0 - 2.0 * SHEET_MARGIN).clamp(1.0, WIDEST_SHEET);
    let covers = (ui.window.0 - centred) / 2.0 < ui.sidebar_width() + space::S2;
    let width = if covers {
        (ui.window.0 - 2.0 * space::S2).max(1.0)
    } else {
        centred
    };
    let top = super::TITLEBAR.max(SHEET_MARGIN);
    let room = (ui.window.1 - top - SHEET_MARGIN / 2.0).max(1.0);
    let height = room.min(TALLEST_SHEET);
    let list_width = if width < 760.0 {
        NARROW_LIST_WIDTH
    } else {
        LIST_WIDTH
    };
    let content = col()
        .key(format!("settings-{}", section_id(section)))
        .w_full()
        .px(space::S6)
        .pt(space::S6)
        .pb(space::S6)
        .gap(space::S6)
        .children(rows);
    let mut body = col()
        .key("settings-body")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(super::page("settings", content));
    if section != Section::About {
        body = body.child(
            row()
                .key("section-foot")
                .px(space::S6)
                .py(space::S3)
                .border_sides([p.hair, 0.0, 0.0, 0.0], p.t.separator)
                .child(
                    Button::new("section-reset", "Reset to defaults")
                        .help(format!(
                            "Put the {} settings back to what a fresh install has",
                            section_label(section).to_lowercase()
                        ))
                        .build(&p, move |app| app.open_popup(Popup::ConfirmReset(section))),
                ),
        );
    }
    let panel = row()
        .key("settings")
        .role("dialog")
        .label("Settings")
        .w(width)
        .h(height)
        .radius(SHEET_RADIUS)
        .clip_rounded(SHEET_RADIUS)
        .fill(p.t.bg)
        .border(p.hair, p.outline)
        .shadow(24.0, 64.0, p.t.shadow)
        .modal(true, close)
        .child(list(ui, list_width))
        .child(body);
    ui.layer.push(
        col()
            .key("settings-scrim")
            .role("none")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .center()
            .pt(top - SHEET_MARGIN / 2.0)
            .fill(Color::BLACK.alpha(0.6))
            .on_press(close)
            .child(panel),
    );
    ui.layer.extend(floating);
}
