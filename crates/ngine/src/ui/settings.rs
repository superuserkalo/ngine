use std::time::Duration;

use rune::tok::{radius, space, type_};
use rune::{Color, El, Icon, PhysicalInput, col, icon, row, spacer, text};

use super::capture::{cost_words, length_select, volume};
use super::widgets::{
    Button, CONTROL, Look, NUMBERS, Option_, Paint, STRONG, Select, caption, dialog, keycaps,
    section, select, setting, toggle, troubled_setting,
};
use super::{EARLY_NOTE, SIMULATED_NOTE, Ui, choose_folder, icons, page, page_title};
use crate::app::{Folder, NAME, Ngine, Popup, Section, VERSION};
use crate::engine::{Codec, Quality, ReplayStorage, Tracks};
use crate::format;
use crate::hotkeys;
use crate::platform;
use crate::settings::{FRAME_RATES, MEMORY_BUDGETS_MB, REPLAY_LENGTHS};

const LIST_WIDTH: f32 = 176.0;
const NARROW_LIST_WIDTH: f32 = 148.0;
const LIST_ITEM: f32 = 30.0;
const SELECT_WIDTH: f32 = 180.0;
const HOTKEY_WIDTH: f32 = 168.0;
const SLIDER_WIDTH: f32 = 120.0;
const WIDEST_SECTION: f32 = 640.0;

pub const NOT_SET: &str = "Not set";
pub const WAITING_FOR_KEYS: &str = "Press keys…";
pub const RECORDER_HINT: &str = "Press the keys to use. Esc cancels, Delete clears.";

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

fn list(ui: &Ui<'_>) -> El {
    let p = &ui.p;
    let mut items = col()
        .key("sections")
        .role("tablist")
        .label("Settings sections")
        .focus_group()
        .w(if ui.narrow() {
            NARROW_LIST_WIDTH
        } else {
            LIST_WIDTH
        })
        .h_full()
        .px(space::S2)
        .py(super::PAGE_PADDING)
        .gap(2.0)
        .border_sides([0.0, p.hair, 0.0, 0.0], p.t.separator)
        .child(
            page_title(p, "settings-title", "Settings")
                .ml(space::S2)
                .mb(space::S2),
        );
    for section in Section::ALL {
        let current = ui.app.section == section;
        let ink = if current {
            p.t.text
        } else {
            p.t.text_secondary
        };
        items = items.child(
            row()
                .key(format!("section-{}", section_id(section)))
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
                .on_press(move |app: &mut Ngine| app.show_section(section))
                .child(icon(section_icon(section), 15.0).color(ink).a11y_hidden())
                .child(
                    text(section_label(section))
                        .font(type_::LABEL)
                        .color(ink)
                        .truncate()
                        .a11y_hidden(),
                ),
        );
    }
    items
}

fn codec_label(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "H.264",
        Codec::Hevc => "HEVC (H.265)",
        Codec::Av1 => "AV1",
    }
}

fn quality_label(quality: Quality) -> &'static str {
    match quality {
        Quality::Low => "Small",
        Quality::Medium => "Balanced",
        Quality::High => "High",
        Quality::Highest => "Highest",
    }
}

fn seconds(seconds: u16) -> String {
    format::length(Duration::from_secs(u64::from(seconds)))
}

fn megabytes_a_minute(app: &Ngine) -> String {
    format!(
        "About {} for each minute",
        format::megabytes(app.megabytes_per_minute())
    )
}

fn video(ui: &mut Ui<'_>) -> Vec<El> {
    let p = ui.p;
    let app = ui.app;
    let video = app.settings.video;
    let codecs = app.capabilities.codecs.clone();
    let codec = Select {
        key: "codec",
        label: "Codec",
        options: codecs
            .iter()
            .map(|codec| Option_::new(codec_label(*codec)))
            .collect(),
        selected: codecs.iter().position(|codec| *codec == video.codec),
        width: SELECT_WIDTH,
    };
    let codec_hint = match codecs.len() {
        0 | 1 => "The only one this computer can encode".to_owned(),
        _ => "Only the ones this computer can encode are listed".to_owned(),
    };
    let codec = select(&p, app, &mut ui.layer, codec, move |app, index| {
        if let Some(codec) = codecs.get(index) {
            app.edit(|settings| settings.video.codec = *codec);
        }
    });
    let quality = Select {
        key: "quality",
        label: "Quality",
        options: Quality::ALL
            .iter()
            .map(|quality| Option_::new(quality_label(*quality)))
            .collect(),
        selected: Quality::ALL.iter().position(|one| *one == video.quality),
        width: SELECT_WIDTH,
    };
    let quality = select(&p, app, &mut ui.layer, quality, |app, index| {
        if let Some(quality) = Quality::ALL.get(index) {
            app.edit(|settings| settings.video.quality = *quality);
        }
    });
    let rate = Select {
        key: "frame-rate",
        label: "Frame rate",
        options: FRAME_RATES
            .iter()
            .map(|rate| Option_::new(format!("{rate} fps")))
            .collect(),
        selected: FRAME_RATES
            .iter()
            .position(|rate| *rate == video.frame_rate),
        width: SELECT_WIDTH,
    };
    let rate = select(&p, app, &mut ui.layer, rate, |app, index| {
        if let Some(rate) = FRAME_RATES.get(index) {
            app.edit(|settings| settings.video.frame_rate = *rate);
        }
    });
    vec![section(
        &p,
        "video",
        "Encoding",
        vec![
            setting(&p, "codec-row", "Codec", Some(codec_hint), codec),
            setting(
                &p,
                "quality-row",
                "Quality",
                Some(megabytes_a_minute(app)),
                quality,
            ),
            setting(&p, "frame-rate-row", "Frame rate", None, rate),
        ],
    )]
}

fn audio(ui: &mut Ui<'_>) -> Vec<El> {
    let p = ui.p;
    let app = ui.app;
    let audio = app.settings.audio;
    let tracks = Select {
        key: "tracks",
        label: "Tracks",
        options: Tracks::ALL
            .iter()
            .map(|tracks| match tracks {
                Tracks::Three => Option_::new("Three tracks"),
                Tracks::MixOnly => Option_::new("Mix only"),
            })
            .collect(),
        selected: Tracks::ALL.iter().position(|one| *one == audio.tracks),
        width: SELECT_WIDTH,
    };
    let tracks = select(&p, app, &mut ui.layer, tracks, |app, index| {
        if let Some(tracks) = Tracks::ALL.get(index) {
            app.edit(|settings| settings.audio.tracks = *tracks);
        }
    });
    vec![
        section(
            &p,
            "sources",
            "Sources",
            vec![
                toggle(&p, "system-sound", "System sound", None, audio.system.enabled, |app| {
                    app.edit(|settings| settings.audio.system.enabled ^= true);
                }),
                setting(
                    &p,
                    "system-volume-row",
                    "System sound volume",
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
                toggle(&p, "microphone", "Microphone", None, audio.microphone.enabled, |app| {
                    app.edit(|settings| settings.audio.microphone.enabled ^= true);
                }),
                setting(
                    &p,
                    "microphone-volume-row",
                    "Microphone volume",
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
        ),
        section(
            &p,
            "mixing",
            "Tracks",
            vec![
                setting(
                    &p,
                    "tracks-row",
                    "Tracks in a recording",
                    Some("Three tracks keep system sound and microphone apart for later. About 2 MB more a minute.".into()),
                    tracks,
                ),
                toggle(
                    &p,
                    "app-only",
                    "Only the recorded app's sound",
                    Some("For a window or an app. A screen always records all system sound.".into()),
                    audio.app_only,
                    |app| app.edit(|settings| settings.audio.app_only ^= true),
                ),
            ],
        ),
    ]
}

fn storage_label(storage: ReplayStorage) -> &'static str {
    match storage {
        ReplayStorage::Auto => "Automatic",
        ReplayStorage::Memory => "Memory",
        ReplayStorage::Disk => "Disk",
    }
}

fn storage_hint(storage: ReplayStorage) -> &'static str {
    match storage {
        ReplayStorage::Auto => "In memory while it fits the budget, on disk beyond that",
        ReplayStorage::Memory => "Always in memory, whatever the budget",
        ReplayStorage::Disk => "Always in short files in the buffer folder",
    }
}

fn budget_select(ui: &mut Ui<'_>) -> El {
    let p = ui.p;
    let app = ui.app;
    let replay = &app.settings.replay;
    let automatic = format!(
        "Automatic, {}",
        format::megabytes(u64::from(app.capabilities.default_memory_budget_mb))
    );
    let mut budgets = vec![Option_::new(automatic)];
    budgets.extend(
        MEMORY_BUDGETS_MB
            .iter()
            .map(|budget| Option_::new(format::megabytes(u64::from(*budget)))),
    );
    let budget = Select {
        key: "memory-budget",
        label: "Memory budget",
        options: budgets,
        selected: Some(replay.memory_budget_mb.map_or(0, |chosen| {
            MEMORY_BUDGETS_MB
                .iter()
                .position(|budget| *budget == chosen)
                .map_or(0, |at| at + 1)
        })),
        width: SELECT_WIDTH,
    };
    select(&p, app, &mut ui.layer, budget, |app, index| {
        let chosen = index
            .checked_sub(1)
            .and_then(|at| MEMORY_BUDGETS_MB.get(at))
            .copied();
        app.edit(|settings| settings.replay.memory_budget_mb = chosen);
    })
}

fn replay(ui: &mut Ui<'_>) -> Vec<El> {
    let p = ui.p;
    let length = length_select(ui, "settings-replay-length", SELECT_WIDTH);
    let app = ui.app;
    let replay = &app.settings.replay;
    let offered: Vec<u16> = REPLAY_LENGTHS
        .into_iter()
        .filter(|length| *length <= replay.length_seconds)
        .collect();
    let save = Select {
        key: "save-length",
        label: "Custom save length",
        options: offered
            .iter()
            .map(|length| Option_::new(seconds(*length)))
            .collect(),
        selected: offered
            .iter()
            .position(|length| *length == replay.save_seconds),
        width: SELECT_WIDTH,
    };
    let save = select(&p, app, &mut ui.layer, save, move |app, index| {
        if let Some(length) = offered.get(index) {
            app.edit(|settings| settings.replay.save_seconds = *length);
        }
    });
    let storage = Select {
        key: "storage",
        label: "Where the buffer is kept",
        options: ReplayStorage::ALL
            .iter()
            .map(|storage| Option_::new(storage_label(*storage)))
            .collect(),
        selected: ReplayStorage::ALL
            .iter()
            .position(|one| *one == replay.storage),
        width: SELECT_WIDTH,
    };
    let storage = select(&p, app, &mut ui.layer, storage, |app, index| {
        if let Some(storage) = ReplayStorage::ALL.get(index) {
            app.edit(|settings| settings.replay.storage = *storage);
        }
    });
    let budget = budget_select(ui);
    vec![
        section(
            &p,
            "buffer",
            "Buffer",
            vec![
                setting(
                    &p,
                    "replay-length-row",
                    "Keep the last",
                    Some(cost_words(app, replay.length_seconds)),
                    length,
                ),
                setting(
                    &p,
                    "storage-row",
                    "Where it is kept",
                    Some(storage_hint(replay.storage).into()),
                    storage,
                ),
                setting(&p, "memory-budget-row", "Memory budget", None, budget),
            ],
        ),
        section(
            &p,
            "saving",
            "Saving",
            vec![
                setting(&p, "save-length-row", "Custom save length", None, save),
                toggle(
                    &p,
                    "clear-after-save",
                    "Clear the buffer after a save",
                    None,
                    replay.clear_after_save,
                    |app| app.edit(|settings| settings.replay.clear_after_save ^= true),
                ),
                toggle(
                    &p,
                    "replay-at-start",
                    "Turn on when ngine starts",
                    None,
                    replay.start_with_app,
                    |app| app.edit(|settings| settings.replay.start_with_app ^= true),
                ),
            ],
        ),
    ]
}

fn hotkey_face(p: &Paint, app: &Ngine, slot: usize, key: &str) -> El {
    if app.is_capturing(slot) {
        return text(WAITING_FOR_KEYS)
            .key(format!("{key}-waiting"))
            .font(type_::LABEL)
            .color(p.accent)
            .a11y_hidden();
    }
    match hotkeys::keycaps(app.settings.hotkeys.all()[slot]) {
        Some(caps) => keycaps(p, &format!("{key}-caps"), &caps).a11y_hidden(),
        None => text(NOT_SET)
            .key(format!("{key}-unset"))
            .font(type_::LABEL)
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
        .help("Click, then press the keys to use")
        .w(HOTKEY_WIDTH)
        .h(CONTROL)
        .px(space::S2)
        .center()
        .radius(radius::MD)
        .fill(p.t.control_track)
        .border(
            if capturing { 1.5 } else { p.hair },
            if capturing { p.accent } else { p.outline },
        )
        .hover(p.t.hover)
        .focusable(true)
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
    let clear = Button::new(&format!("{key}-clear"), "Clear hotkey")
        .only_icon(&icons::X)
        .look(Look::Quiet)
        .blocked(hotkeys::is_unset(accelerator).then(|| "No hotkey is set".to_owned()))
        .build(p, move |app| app.clear_hotkey(slot));
    let control = row()
        .key(format!("{key}-control"))
        .gap(space::S1)
        .center_y()
        .child(recorder)
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
        (None, None) if capturing => {
            setting(p, &row_key, &label, Some(RECORDER_HINT.into()), control)
        }
        (None, None) => setting(p, &row_key, &label, None, control),
    }
}

fn hotkeys_section(ui: &Ui<'_>) -> Vec<El> {
    let p = &ui.p;
    vec![
        section(
            p,
            "hotkeys",
            "Work in any app",
            (0..hotkeys::COUNT)
                .map(|slot| hotkey(p, ui.app, slot))
                .collect(),
        ),
        caption(
            p,
            "hotkeys-commands",
            "Where the system keeps hotkeys to itself, bind keys to a command instead, such as: ngine replay save --last 30s",
        )
        .mx(space::S1),
    ]
}

fn folder_row(p: &Paint, key: &str, label: &str, folder: Folder, path: &std::path::Path) -> El {
    let opened = path.to_owned();
    let controls = row()
        .key(format!("{key}-controls"))
        .gap(space::S1 + 2.0)
        .child(Button::new(&format!("{key}-change"), "Change…").build_tx(p, choose_folder(folder)))
        .child(
            Button::new(&format!("{key}-open"), "Open")
                .help(format!("Open in {}", platform::FILE_MANAGER))
                .build(p, move |app| app.open_folder(&opened)),
        );
    setting(p, key, label, Some(path.display().to_string()), controls)
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
                "Room for about {} of recording at the current quality",
                room(bytes / per_minute)
            )
        })
    });
    vec![section(
        p,
        "folders",
        "Folders",
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
        "Starting",
        vec![toggle(
            p,
            "start-at-login",
            "Start ngine when you log in",
            Some("It starts in the tray, without opening this window".into()),
            ui.app.settings.start_at_login,
            |app| app.edit(|settings| settings.start_at_login ^= true),
        )],
    )]
}

fn about(ui: &Ui<'_>) -> Vec<El> {
    let p = &ui.p;
    let mut words = col()
        .key("about-words")
        .padding(space::S3)
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
    vec![section(p, "about", "This build", vec![words])]
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
        "They go back to what a fresh install has. Nothing else changes.".into(),
        actions,
    );
}

pub fn view(ui: &mut Ui<'_>) -> El {
    let p = ui.p;
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
    let mut head = row()
        .key("section-head")
        .center_y()
        .h(CONTROL)
        .child(
            text(section_label(section))
                .key("section-title")
                .role("heading")
                .level(2)
                .font(super::widgets::HEADING)
                .color(p.t.text),
        )
        .child(spacer());
    if section != Section::About {
        head = head.child(
            Button::new("section-reset", "Reset")
                .icon(&icons::ROTATE_CCW)
                .look(Look::Quiet)
                .help(format!(
                    "Put the {} settings back to their defaults",
                    section_label(section).to_lowercase()
                ))
                .build(&p, move |app| app.open_popup(Popup::ConfirmReset(section))),
        );
    }
    let list_width = if ui.narrow() {
        NARROW_LIST_WIDTH
    } else {
        LIST_WIDTH
    };
    let width =
        (ui.main_width() - list_width - 2.0 * super::PAGE_PADDING).clamp(1.0, WIDEST_SECTION);
    let content = col()
        .key(format!("settings-{}", section_id(section)))
        .w(width)
        .self_center()
        .py(super::PAGE_PADDING)
        .gap(space::S6 - 4.0)
        .child(head)
        .children(rows);
    row()
        .key("settings")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(list(ui))
        .child(page("settings", content).grow(1.0).min_w(0.0))
}
