use std::time::Duration;

use rune::El;
use rune::tok::space;
use rune::{col, row, text};

use super::build_note;
use super::record::cost_words;
use super::widgets::{
    Choice, Input, Paint, STRONG, Segments, Stops, caption, card, input, section, segmented,
    setting, slider, toggle, troubled_setting,
};
use crate::app::{Field, NAME, Ngine, SettingsTab, VERSION};
use crate::engine::{Codec, Quality, ReplayStorage, Tracks};
use crate::format;
use crate::hotkeys;
use crate::settings::{FRAME_RATES, MEMORY_BUDGETS_MB, Named, REPLAY_LENGTHS, Settings};

const TAB_WIDTH: f32 = 84.0;
const FOLDER_WIDTH: f32 = 300.0;
const HOTKEY_WIDTH: f32 = 150.0;
const VOLUME_STEP: u8 = 5;
const VOLUME_STOPS: u16 = 21;

const TABS: [(SettingsTab, &str, &str); 5] = [
    (SettingsTab::Video, "video", "Video"),
    (SettingsTab::Audio, "audio", "Audio"),
    (SettingsTab::Replay, "replay", "Replay"),
    (SettingsTab::Hotkeys, "hotkeys", "Hotkeys"),
    (SettingsTab::General, "general", "General"),
];

const HOTKEY_LABELS: [&str; hotkeys::COUNT] = [
    "Start or stop recording",
    "Save the last 30 s",
    "Save the custom length",
    "Save everything in the buffer",
    "Add a marker",
];

fn tabs(p: &Paint, app: &Ngine) -> El {
    let segments = Segments {
        key: "settings-tab",
        label: "Settings",
        choices: TABS.map(|(_, id, label)| Choice::new(id, label)).into(),
        selected: TABS.iter().position(|(tab, ..)| *tab == app.settings_tab),
        segment_width: TAB_WIDTH,
    };
    segmented(p, segments, |app, index| {
        if let Some((tab, ..)) = TABS.get(index) {
            app.settings_tab = *tab;
        }
    })
}

fn codec_label(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "H.264",
        Codec::Hevc => "HEVC",
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

fn choose<T: Named>(
    p: &Paint,
    key: &str,
    label: &str,
    width: f32,
    current: T,
    choice: impl Fn(T) -> Choice,
    set: fn(&mut Settings, T),
) -> El {
    let segments = Segments {
        key,
        label,
        choices: T::CHOICES.iter().copied().map(choice).collect(),
        selected: T::CHOICES
            .iter()
            .position(|candidate| *candidate == current),
        segment_width: width,
    };
    segmented(p, segments, move |app, index| {
        if let Some(picked) = T::CHOICES.get(index) {
            app.edit(|settings| set(settings, *picked));
        }
    })
}

fn megabytes_per_minute(app: &Ngine) -> String {
    format!(
        "About {} for each minute",
        format::megabytes(app.megabytes_per_minute())
    )
}

fn folder(p: &Paint, key: &'static str, label: &'static str, app: &Ngine, field: Field) -> El {
    let edit = match field {
        Field::BufferFolder => &app.fields.buffer_folder,
        _ => &app.fields.output_folder,
    };
    input(
        p,
        &Input {
            key,
            label,
            edit,
            field,
            width: FOLDER_WIDTH,
            invalid: false,
            applies_on_enter: false,
        },
    )
}

fn video(p: &Paint, app: &Ngine) -> El {
    let video = app.settings.video;
    let missing: Vec<&str> = Codec::ALL
        .into_iter()
        .filter(|codec| !app.capabilities.codecs.contains(codec))
        .map(codec_label)
        .collect();
    let codec_hint = (!missing.is_empty()).then(|| {
        format!(
            "{} cannot be encoded on this machine",
            missing.join(" and ")
        )
    });
    let available = app.capabilities.codecs.clone();
    let codecs = choose(
        p,
        "codec",
        "Codec",
        72.0,
        video.codec,
        move |codec| {
            Choice::new(codec.name(), codec_label(codec)).available(available.contains(&codec))
        },
        |settings, codec| settings.video.codec = codec,
    );
    let qualities = choose(
        p,
        "quality",
        "Quality",
        72.0,
        video.quality,
        |quality| Choice::new(quality.name(), quality_label(quality)),
        |settings, quality| settings.video.quality = quality,
    );
    let rates = Segments {
        key: "frame-rate",
        label: "Frame rate",
        choices: vec![
            Choice::new("30", "30"),
            Choice::new("60", "60"),
            Choice::new("120", "120"),
        ],
        selected: FRAME_RATES
            .iter()
            .position(|rate| *rate == video.frame_rate),
        segment_width: 56.0,
    };
    let rates = segmented(p, rates, |app, index| {
        if let Some(rate) = FRAME_RATES.get(index) {
            app.edit(|settings| settings.video.frame_rate = *rate);
        }
    });
    let folder = folder(
        p,
        "output-folder",
        "Output folder",
        app,
        Field::OutputFolder,
    );
    col()
        .key("settings-video")
        .gap(space::S3 + space::S1)
        .child(section(
            p,
            "video",
            "Video",
            vec![
                setting(p, "codec-row", "Codec", codec_hint, codecs),
                setting(
                    p,
                    "quality-row",
                    "Quality",
                    Some(megabytes_per_minute(app)),
                    qualities,
                ),
                setting(p, "frame-rate-row", "Frames per second", None, rates),
            ],
        ))
        .child(section(
            p,
            "output",
            "Files",
            vec![setting(
                p,
                "output-folder-row",
                "Save recordings and clips to",
                None,
                folder,
            )],
        ))
}

fn volume(
    p: &Paint,
    key: &'static str,
    label: &'static str,
    percent: u8,
    set: fn(&mut Settings, u8),
) -> El {
    let stops = Stops {
        key,
        label,
        index: u16::from(percent / VOLUME_STEP),
        count: VOLUME_STOPS,
        value: format!("{percent} %"),
    };
    slider(p, stops, move |app, index| {
        let percent = u8::try_from(index)
            .unwrap_or(u8::MAX)
            .saturating_mul(VOLUME_STEP);
        app.edit(|settings| set(settings, percent.min(100)));
    })
}

fn audio(p: &Paint, app: &Ngine) -> El {
    let audio = app.settings.audio;
    let tracks = choose(
        p,
        "tracks",
        "Audio tracks",
        96.0,
        audio.tracks,
        |tracks| match tracks {
            Tracks::Three => Choice::new(tracks.name(), "Three tracks"),
            Tracks::MixOnly => Choice::new(tracks.name(), "Mix only"),
        },
        |settings, tracks| settings.audio.tracks = tracks,
    );
    col()
        .key("settings-audio")
        .gap(space::S3 + space::S1)
        .child(section(
            p,
            "sources",
            "Sources",
            vec![
                toggle(p, "system-sound", "System sound", None, audio.system.enabled, |app| {
                    app.edit(|settings| settings.audio.system.enabled ^= true);
                }),
                setting(
                    p,
                    "system-volume-row",
                    "System sound volume",
                    None,
                    volume(
                        p,
                        "system-volume",
                        "System sound volume",
                        audio.system.volume_percent,
                        |settings, percent| settings.audio.system.volume_percent = percent,
                    ),
                ),
                toggle(p, "microphone", "Microphone", None, audio.microphone.enabled, |app| {
                    app.edit(|settings| settings.audio.microphone.enabled ^= true);
                }),
                setting(
                    p,
                    "microphone-volume-row",
                    "Microphone volume",
                    None,
                    volume(
                        p,
                        "microphone-volume",
                        "Microphone volume",
                        audio.microphone.volume_percent,
                        |settings, percent| settings.audio.microphone.volume_percent = percent,
                    ),
                ),
            ],
        ))
        .child(section(
            p,
            "mixing",
            "Tracks",
            vec![
                setting(
                    p,
                    "tracks-row",
                    "Tracks in a recording",
                    Some(
                        "Three tracks keep the mix, the system sound and the microphone apart, so a clip can be rebalanced later. About 2 MB more for each minute."
                            .into(),
                    ),
                    tracks,
                ),
                toggle(
                    p,
                    "app-only",
                    "Record only the sound of the app being recorded",
                    Some(
                        "For a window or an app. A screen recording always has all system sound."
                            .into(),
                    ),
                    audio.app_only,
                    |app| app.edit(|settings| settings.audio.app_only ^= true),
                ),
            ],
        ))
}

fn nearest(stops: &[u16], value: u16) -> u16 {
    let index = stops
        .iter()
        .position(|stop| *stop >= value)
        .unwrap_or(stops.len().saturating_sub(1));
    u16::try_from(index).unwrap_or(0)
}

fn seconds(seconds: u16) -> String {
    format::length(Duration::from_secs(u64::from(seconds)))
}

fn replay_length(p: &Paint, app: &Ngine) -> El {
    let length = app.settings.replay.length_seconds;
    let stops = Stops {
        key: "replay-length",
        label: "Replay length",
        index: nearest(&REPLAY_LENGTHS, length),
        count: u16::try_from(REPLAY_LENGTHS.len()).unwrap_or(1),
        value: seconds(length),
    };
    slider(p, stops, |app, index| {
        if let Some(length) = REPLAY_LENGTHS.get(usize::from(index)) {
            app.edit(|settings| settings.replay.length_seconds = *length);
        }
    })
}

fn save_length(p: &Paint, app: &Ngine) -> El {
    let replay = &app.settings.replay;
    let offered: Vec<u16> = REPLAY_LENGTHS
        .into_iter()
        .filter(|length| *length <= replay.length_seconds)
        .collect();
    let stops = Stops {
        key: "save-length",
        label: "Custom save length",
        index: nearest(&offered, replay.save_seconds),
        count: u16::try_from(offered.len()).unwrap_or(1),
        value: seconds(replay.save_seconds),
    };
    slider(p, stops, move |app, index| {
        if let Some(length) = offered.get(usize::from(index)) {
            app.edit(|settings| settings.replay.save_seconds = *length);
        }
    })
}

fn memory_budget(p: &Paint, app: &Ngine) -> El {
    let chosen = app.settings.replay.memory_budget_mb;
    let index = chosen.map_or(0, |budget| {
        MEMORY_BUDGETS_MB
            .iter()
            .position(|stop| *stop >= budget)
            .map_or(MEMORY_BUDGETS_MB.len(), |position| position + 1)
    });
    let megabytes = format::megabytes(u64::from(app.memory_budget_mb()));
    let stops = Stops {
        key: "memory-budget",
        label: "Memory budget",
        index: u16::try_from(index).unwrap_or(0),
        count: u16::try_from(MEMORY_BUDGETS_MB.len() + 1).unwrap_or(1),
        value: if chosen.is_some() {
            megabytes
        } else {
            format!("Automatic, {megabytes}")
        },
    };
    slider(p, stops, |app, index| {
        let budget = usize::from(index)
            .checked_sub(1)
            .and_then(|position| MEMORY_BUDGETS_MB.get(position))
            .copied();
        app.edit(|settings| settings.replay.memory_budget_mb = budget);
    })
}

fn storage_hint(storage: ReplayStorage) -> &'static str {
    match storage {
        ReplayStorage::Auto => "In memory while it fits the budget, on disk beyond that",
        ReplayStorage::Memory => "Always in memory, whatever the budget",
        ReplayStorage::Disk => "Always in short files in the buffer folder",
    }
}

fn replay(p: &Paint, app: &Ngine) -> El {
    let replay = &app.settings.replay;
    let storage = choose(
        p,
        "storage",
        "Replay storage",
        84.0,
        replay.storage,
        |storage| match storage {
            ReplayStorage::Auto => Choice::new(storage.name(), "Automatic"),
            ReplayStorage::Memory => Choice::new(storage.name(), "Memory"),
            ReplayStorage::Disk => Choice::new(storage.name(), "Disk"),
        },
        |settings, storage| settings.replay.storage = storage,
    );
    let buffer_folder = folder(
        p,
        "buffer-folder",
        "Buffer folder",
        app,
        Field::BufferFolder,
    );
    col()
        .key("settings-replay")
        .gap(space::S3 + space::S1)
        .child(section(
            p,
            "buffer",
            "Buffer",
            vec![
                setting(
                    p,
                    "replay-length-row",
                    "Keep the last",
                    Some(cost_words(app)),
                    replay_length(p, app),
                ),
                setting(
                    p,
                    "storage-row",
                    "Where the buffer lives",
                    Some(storage_hint(replay.storage).into()),
                    storage,
                ),
                setting(
                    p,
                    "memory-budget-row",
                    "Memory budget",
                    None,
                    memory_budget(p, app),
                ),
                setting(p, "buffer-folder-row", "Buffer folder", None, buffer_folder),
            ],
        ))
        .child(section(
            p,
            "saving",
            "Saving",
            vec![
                setting(
                    p,
                    "save-length-row",
                    "Custom save length",
                    None,
                    save_length(p, app),
                ),
                toggle(
                    p,
                    "clear-after-save",
                    "Clear the buffer after a save",
                    None,
                    replay.clear_after_save,
                    |app| app.edit(|settings| settings.replay.clear_after_save ^= true),
                ),
                toggle(
                    p,
                    "replay-at-start",
                    "Turn instant replay on when ngine starts",
                    None,
                    replay.start_with_app,
                    |app| app.edit(|settings| settings.replay.start_with_app ^= true),
                ),
            ],
        ))
}

fn hotkey(p: &Paint, app: &Ngine, slot: usize) -> El {
    let key = format!("hotkey-{slot}");
    let saved = app.settings.hotkeys.all()[slot];
    let typed = app.fields.hotkeys[slot].text.trim();
    let problem = app.hotkey_problems[slot].clone();
    let waiting = typed != saved;
    let field = input(
        p,
        &Input {
            key: &key,
            label: HOTKEY_LABELS[slot],
            edit: &app.fields.hotkeys[slot],
            field: Field::Hotkey(slot),
            width: HOTKEY_WIDTH,
            invalid: problem.is_some(),
            applies_on_enter: true,
        },
    );
    let label = if slot == 2 {
        format!(
            "Save the last {}",
            seconds(app.settings.replay.save_seconds)
        )
    } else {
        HOTKEY_LABELS[slot].to_owned()
    };
    let row_key = format!("{key}-row");
    match problem {
        _ if waiting => {
            let hint = Some("Press Enter to use it".to_owned());
            setting(p, &row_key, &label, hint, field)
        }
        Some(trouble) => troubled_setting(p, &row_key, &label, trouble, field),
        None => setting(p, &row_key, &label, hotkeys::keycaps(saved), field),
    }
}

fn hotkeys_tab(p: &Paint, app: &Ngine) -> El {
    col()
        .key("settings-hotkeys")
        .gap(space::S3 + space::S1)
        .child(section(
            p,
            "hotkeys",
            "Hotkeys that work in any app",
            (0..hotkeys::COUNT).map(|slot| hotkey(p, app, slot)).collect(),
        ))
        .child(
            col()
                .key("hotkeys-help")
                .px(space::S3)
                .gap(space::S1)
                .child(caption(
                    p,
                    "hotkeys-format",
                    "Write keys joined with +, such as ctrl+alt+r or ctrl+shift+f9.",
                ))
                .child(caption(
                    p,
                    "hotkeys-commands",
                    "A compositor can bind keys to commands instead, such as: ngine replay save --last 30s",
                )),
        )
}

fn general(p: &Paint, app: &Ngine) -> El {
    col()
        .key("settings-general")
        .gap(space::S3 + space::S1)
        .child(section(
            p,
            "startup",
            "Starting",
            vec![toggle(
                p,
                "start-at-login",
                "Start ngine when you log in",
                Some("It starts in the tray, without opening this window".into()),
                app.settings.start_at_login,
                |app| app.edit(|settings| settings.start_at_login ^= true),
            )],
        ))
        .child(card(
            p,
            "about",
            vec![
                row().key("about-row").padding(space::S3).child(
                    col()
                        .key("about-words")
                        .gap(space::S1)
                        .child(
                            text(format!("{NAME} {VERSION}"))
                                .key("about-version")
                                .font(STRONG)
                                .color(p.t.text),
                        )
                        .child(caption(p, "about-note", build_note(app))),
                ),
            ],
        ))
}

pub fn view(p: &Paint, app: &Ngine) -> El {
    let tab = match app.settings_tab {
        SettingsTab::Video => video(p, app),
        SettingsTab::Audio => audio(p, app),
        SettingsTab::Replay => replay(p, app),
        SettingsTab::Hotkeys => hotkeys_tab(p, app),
        SettingsTab::General => general(p, app),
    };
    col()
        .key("settings")
        .gap(space::S3 + space::S1)
        .child(tabs(p, app))
        .child(tab)
}
