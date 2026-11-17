use std::path::{Path, PathBuf};

use rune::prefs::{Prefs, PrefsError};

use crate::engine::{
    Audio, AudioSource, Codec, Device, Inputs, Quality, Region, ReplayStorage, SystemSound, Tracks,
    Video,
};
use crate::selection::Area;

pub const FRAME_RATES: [u16; 3] = [30, 60, 120];
pub const REPLAY_LENGTHS: [u16; 12] = [15, 30, 45, 60, 90, 120, 180, 300, 600, 900, 1200, 1800];
pub const MEMORY_BUDGETS_MB: [u32; 7] = [256, 384, 512, 768, 1024, 1536, 2048];
pub const SHORT_SAVE_SECONDS: u16 = 30;

pub const SHORTEST_REPLAY: u16 = REPLAY_LENGTHS[0];
pub const LONGEST_REPLAY: u16 = REPLAY_LENGTHS[REPLAY_LENGTHS.len() - 1];
pub const SMALLEST_BUDGET_MB: u32 = 128;
pub const LARGEST_BUDGET_MB: u32 = 16_384;
pub const LOWEST_FRAME_RATE: u16 = 10;
pub const HIGHEST_FRAME_RATE: u16 = 240;
const APP_SOUND: &str = "app:";
const OUTPUT_SOUND: &str = "output:";
const FULL_VOLUME: u8 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaySettings {
    pub length_seconds: u16,
    pub save_seconds: u16,
    pub storage: ReplayStorage,
    pub memory_budget_mb: Option<u32>,
    pub buffer_folder: PathBuf,
    pub clear_after_save: bool,
    pub start_with_app: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hotkeys {
    pub record: String,
    pub selection: String,
    pub save_short: String,
    pub save_custom: String,
    pub save_everything: String,
    pub marker: String,
}

impl Hotkeys {
    pub const COUNT: usize = 6;

    pub fn all(&self) -> [&str; Self::COUNT] {
        [
            &self.record,
            &self.selection,
            &self.save_short,
            &self.save_custom,
            &self.save_everything,
            &self.marker,
        ]
    }

    pub fn set(&mut self, slot: usize, accelerator: String) {
        let slots = [
            &mut self.record,
            &mut self.selection,
            &mut self.save_short,
            &mut self.save_custom,
            &mut self.save_everything,
            &mut self.marker,
        ];
        if let Some(found) = slots.into_iter().nth(slot) {
            *found = accelerator;
        }
    }
}

/// How ngine tells that a file was saved or that something went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Announcements {
    pub overlay: bool,
    pub desktop: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub video: Video,
    pub audio: Audio,
    pub inputs: Inputs,
    pub replay: ReplaySettings,
    pub output_folder: PathBuf,
    pub hotkeys: Hotkeys,
    pub start_at_login: bool,
    pub announcements: Announcements,
    /// The part of a screen recorded last, offered again next time.
    pub selection: Option<Region>,
}

impl Settings {
    pub fn defaults(output_folder: PathBuf, buffer_folder: PathBuf) -> Self {
        let source = AudioSource {
            enabled: true,
            volume_percent: FULL_VOLUME,
        };
        Self {
            video: Video {
                codec: Codec::H264,
                quality: Quality::High,
                frame_rate: 60,
            },
            audio: Audio {
                system: source,
                microphone: source,
                tracks: Tracks::Three,
                app_only: true,
            },
            inputs: Inputs::default(),
            replay: ReplaySettings {
                length_seconds: 60,
                save_seconds: 60,
                storage: ReplayStorage::Auto,
                memory_budget_mb: None,
                buffer_folder,
                clear_after_save: false,
                start_with_app: false,
            },
            output_folder,
            hotkeys: Hotkeys {
                record: "ctrl+alt+r".into(),
                selection: "ctrl+alt+shift+r".into(),
                save_short: "ctrl+alt+s".into(),
                save_custom: "ctrl+alt+d".into(),
                save_everything: "ctrl+alt+a".into(),
                marker: "ctrl+alt+m".into(),
            },
            start_at_login: false,
            announcements: Announcements {
                overlay: true,
                desktop: true,
            },
            selection: None,
        }
    }
}

pub trait Named: Sized + Copy + PartialEq + 'static {
    const CHOICES: &'static [Self];
    fn name(self) -> &'static str;

    fn from_name(name: &str) -> Option<Self> {
        Self::CHOICES
            .iter()
            .copied()
            .find(|choice| choice.name() == name)
    }
}

impl Named for Codec {
    const CHOICES: &'static [Self] = &Self::ALL;
    fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }
}

impl Named for Quality {
    const CHOICES: &'static [Self] = &Self::ALL;
    fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Highest => "highest",
        }
    }
}

impl Named for Tracks {
    const CHOICES: &'static [Self] = &Self::ALL;
    fn name(self) -> &'static str {
        match self {
            Self::Three => "three",
            Self::MixOnly => "mix-only",
        }
    }
}

impl Named for ReplayStorage {
    const CHOICES: &'static [Self] = &Self::ALL;
    fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Memory => "memory",
            Self::Disk => "disk",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Text(String),
    Number(u32),
    Flag(bool),
    Unset,
}

fn folder_entry(folder: &Path) -> Entry {
    Entry::Text(folder.to_string_lossy().into_owned())
}

fn system_sound_entry(sound: &SystemSound) -> Entry {
    match sound {
        SystemSound::Everything => Entry::Unset,
        SystemSound::App { name, .. } => Entry::Text(format!("{APP_SOUND}{name}")),
        SystemSound::Output(device) => Entry::Text(format!("{OUTPUT_SOUND}{}", device.id)),
    }
}

fn system_sound_from(text: &str) -> SystemSound {
    if let Some(name) = text.strip_prefix(APP_SOUND) {
        SystemSound::App {
            id: 0,
            name: name.to_owned(),
        }
    } else if let Some(id) = text.strip_prefix(OUTPUT_SOUND) {
        SystemSound::Output(Device {
            id: id.to_owned(),
            name: id.to_owned(),
            is_default: false,
        })
    } else {
        SystemSound::Everything
    }
}

fn entries(settings: &Settings) -> Vec<(&'static str, Entry)> {
    let Settings {
        video,
        audio,
        inputs,
        replay,
        output_folder,
        hotkeys,
        start_at_login,
        announcements,
        selection,
    } = settings;
    let mut entries = vec![
        ("video.codec", Entry::Text(video.codec.name().into())),
        ("video.quality", Entry::Text(video.quality.name().into())),
        (
            "video.frame_rate",
            Entry::Number(u32::from(video.frame_rate)),
        ),
        ("audio.system.enabled", Entry::Flag(audio.system.enabled)),
        (
            "audio.system.volume",
            Entry::Number(u32::from(audio.system.volume_percent)),
        ),
        (
            "audio.microphone.enabled",
            Entry::Flag(audio.microphone.enabled),
        ),
        (
            "audio.microphone.volume",
            Entry::Number(u32::from(audio.microphone.volume_percent)),
        ),
        ("audio.tracks", Entry::Text(audio.tracks.name().into())),
        ("audio.app_only", Entry::Flag(audio.app_only)),
        (
            "audio.microphone.device",
            inputs.microphone.clone().map_or(Entry::Unset, Entry::Text),
        ),
        (
            "audio.microphone.name",
            inputs
                .microphone_name
                .clone()
                .map_or(Entry::Unset, Entry::Text),
        ),
        ("audio.system.source", system_sound_entry(&inputs.system)),
        (
            "replay.length_seconds",
            Entry::Number(u32::from(replay.length_seconds)),
        ),
        (
            "replay.save_seconds",
            Entry::Number(u32::from(replay.save_seconds)),
        ),
        ("replay.storage", Entry::Text(replay.storage.name().into())),
        (
            "replay.memory_budget_mb",
            replay.memory_budget_mb.map_or(Entry::Unset, Entry::Number),
        ),
        ("replay.buffer_folder", folder_entry(&replay.buffer_folder)),
        (
            "replay.clear_after_save",
            Entry::Flag(replay.clear_after_save),
        ),
        ("replay.start_with_app", Entry::Flag(replay.start_with_app)),
        ("output_folder", folder_entry(output_folder)),
        ("start_at_login", Entry::Flag(*start_at_login)),
        ("announcements.overlay", Entry::Flag(announcements.overlay)),
        ("announcements.desktop", Entry::Flag(announcements.desktop)),
    ];
    entries.extend(hotkey_entries(hotkeys));
    entries.extend(selection_entries(selection.as_ref()));
    entries
}

fn hotkey_entries(hotkeys: &Hotkeys) -> Vec<(&'static str, Entry)> {
    vec![
        ("hotkeys.record", Entry::Text(hotkeys.record.clone())),
        ("hotkeys.selection", Entry::Text(hotkeys.selection.clone())),
        (
            "hotkeys.save_short",
            Entry::Text(hotkeys.save_short.clone()),
        ),
        (
            "hotkeys.save_custom",
            Entry::Text(hotkeys.save_custom.clone()),
        ),
        (
            "hotkeys.save_everything",
            Entry::Text(hotkeys.save_everything.clone()),
        ),
        ("hotkeys.marker", Entry::Text(hotkeys.marker.clone())),
    ]
}

fn selection_entries(selection: Option<&Region>) -> Vec<(&'static str, Entry)> {
    let selection_part = |part: fn(&Region) -> u32| {
        selection.map_or(Entry::Unset, |region| Entry::Number(part(region)))
    };
    vec![
        (
            "selection.screen",
            selection.map_or(Entry::Unset, |region| Entry::Text(region.screen.clone())),
        ),
        ("selection.x", selection_part(|region| region.area.x)),
        ("selection.y", selection_part(|region| region.area.y)),
        (
            "selection.width",
            selection_part(|region| region.area.width),
        ),
        (
            "selection.height",
            selection_part(|region| region.area.height),
        ),
    ]
}

struct Reader<'a>(&'a Prefs);

impl Reader<'_> {
    fn named<T: Named>(&self, key: &str, default: T) -> T {
        self.0
            .get::<String>(key)
            .and_then(|name| T::from_name(&name))
            .unwrap_or(default)
    }

    fn flag(&self, key: &str, default: bool) -> bool {
        self.0.get(key).unwrap_or(default)
    }

    fn hotkey(&self, key: &str, default: String) -> String {
        self.0
            .get::<String>(key)
            .map_or(default, |text| text.trim().to_owned())
    }

    fn folder(&self, key: &str, default: PathBuf) -> PathBuf {
        self.0
            .get::<String>(key)
            .filter(|text| !text.trim().is_empty())
            .map_or(default, PathBuf::from)
    }

    fn text(&self, key: &str) -> Option<String> {
        self.0
            .get::<String>(key)
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    }

    fn seconds(&self, key: &str, default: u16) -> u16 {
        self.0.get::<u16>(key).map_or(default, |seconds| {
            seconds.clamp(SHORTEST_REPLAY, LONGEST_REPLAY)
        })
    }

    fn volume(&self, key: &str, default: u8) -> u8 {
        self.0
            .get::<u8>(key)
            .map_or(default, |volume| volume.min(FULL_VOLUME))
    }

    fn selection(&self) -> Option<Region> {
        let number = |key: &str| self.0.get::<u32>(key);
        let area = Area::new(
            number("selection.x")? & !1,
            number("selection.y")? & !1,
            number("selection.width")? & !1,
            number("selection.height")? & !1,
        );
        Some(Region {
            screen: self.text("selection.screen")?,
            area,
        })
        .filter(|region| region.area.is_usable())
    }

    fn source(&self, enabled: &str, volume: &str, default: AudioSource) -> AudioSource {
        AudioSource {
            enabled: self.flag(enabled, default.enabled),
            volume_percent: self.volume(volume, default.volume_percent),
        }
    }
}

fn read(prefs: &Prefs, defaults: Settings) -> Settings {
    let reader = Reader(prefs);
    let Settings {
        video,
        audio,
        inputs,
        replay,
        output_folder,
        hotkeys,
        start_at_login,
        announcements,
        selection,
    } = defaults;
    let length_seconds = reader.seconds("replay.length_seconds", replay.length_seconds);
    Settings {
        video: Video {
            codec: reader.named("video.codec", video.codec),
            quality: reader.named("video.quality", video.quality),
            frame_rate: prefs
                .get::<u16>("video.frame_rate")
                .filter(|rate| (LOWEST_FRAME_RATE..=HIGHEST_FRAME_RATE).contains(rate))
                .unwrap_or(video.frame_rate),
        },
        audio: Audio {
            system: reader.source("audio.system.enabled", "audio.system.volume", audio.system),
            microphone: reader.source(
                "audio.microphone.enabled",
                "audio.microphone.volume",
                audio.microphone,
            ),
            tracks: reader.named("audio.tracks", audio.tracks),
            app_only: reader.flag("audio.app_only", audio.app_only),
        },
        inputs: Inputs {
            microphone: reader.text("audio.microphone.device").or(inputs.microphone),
            microphone_name: reader
                .text("audio.microphone.name")
                .or(inputs.microphone_name),
            system: reader
                .text("audio.system.source")
                .map_or(inputs.system, |text| system_sound_from(&text)),
        },
        replay: ReplaySettings {
            length_seconds,
            save_seconds: reader
                .seconds("replay.save_seconds", replay.save_seconds)
                .min(length_seconds),
            storage: reader.named("replay.storage", replay.storage),
            memory_budget_mb: prefs
                .get::<u32>("replay.memory_budget_mb")
                .map(|budget| budget.clamp(SMALLEST_BUDGET_MB, LARGEST_BUDGET_MB)),
            buffer_folder: reader.folder("replay.buffer_folder", replay.buffer_folder),
            clear_after_save: reader.flag("replay.clear_after_save", replay.clear_after_save),
            start_with_app: reader.flag("replay.start_with_app", replay.start_with_app),
        },
        output_folder: reader.folder("output_folder", output_folder),
        hotkeys: Hotkeys {
            record: reader.hotkey("hotkeys.record", hotkeys.record),
            selection: reader.hotkey("hotkeys.selection", hotkeys.selection),
            save_short: reader.hotkey("hotkeys.save_short", hotkeys.save_short),
            save_custom: reader.hotkey("hotkeys.save_custom", hotkeys.save_custom),
            save_everything: reader.hotkey("hotkeys.save_everything", hotkeys.save_everything),
            marker: reader.hotkey("hotkeys.marker", hotkeys.marker),
        },
        start_at_login: reader.flag("start_at_login", start_at_login),
        announcements: Announcements {
            overlay: reader.flag("announcements.overlay", announcements.overlay),
            desktop: reader.flag("announcements.desktop", announcements.desktop),
        },
        selection: reader.selection().or(selection),
    }
}

pub struct Store {
    prefs: Prefs,
    saved: Vec<(&'static str, Entry)>,
    found_damaged: bool,
    defaults: Option<Settings>,
}

impl Store {
    pub fn open(file: &Path) -> Self {
        let existed = file.exists();
        let prefs = Prefs::at(file);
        Self {
            prefs,
            saved: Vec::new(),
            found_damaged: existed && !file.exists(),
            defaults: None,
        }
    }

    pub fn in_memory() -> Self {
        Self {
            prefs: Prefs::memory(),
            saved: Vec::new(),
            found_damaged: false,
            defaults: None,
        }
    }

    /// True when the file could not be read and was set aside, so the
    /// settings in use are the defaults.
    pub fn found_damaged(&self) -> bool {
        self.found_damaged
    }

    /// The settings a fresh install starts with, once [`Store::load`] ran.
    pub fn defaults(&self) -> Option<&Settings> {
        self.defaults.as_ref()
    }

    pub fn load(&mut self, defaults: Settings) -> Settings {
        self.defaults = Some(defaults.clone());
        let settings = read(&self.prefs, defaults);
        self.saved = entries(&settings)
            .into_iter()
            .filter(|(key, _)| self.prefs.contains(key))
            .collect();
        settings
    }

    pub fn save(&mut self, settings: &Settings) -> Result<(), PrefsError> {
        let wanted = entries(settings);
        let mut outcome = Ok(());
        for (key, entry) in &wanted {
            if self.saved.contains(&(*key, entry.clone())) {
                continue;
            }
            let written = match entry {
                Entry::Text(text) => self.prefs.set(key, text),
                Entry::Number(number) => self.prefs.set(key, number),
                Entry::Flag(flag) => self.prefs.set(key, flag),
                Entry::Unset => self.prefs.remove(key),
            };
            outcome = outcome.and(written);
        }
        self.saved = wanted;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Settings {
        Settings::defaults(
            PathBuf::from("/videos/ngine"),
            PathBuf::from("/cache/replay"),
        )
    }

    fn temp_file(test: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("ngine-settings-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("a temp folder");
        folder.join("settings.json")
    }

    fn changed() -> Settings {
        let mut settings = defaults();
        settings.video.codec = Codec::Hevc;
        settings.video.quality = Quality::Low;
        settings.video.frame_rate = 120;
        settings.audio.microphone = AudioSource {
            enabled: false,
            volume_percent: 35,
        };
        settings.audio.tracks = Tracks::MixOnly;
        settings.audio.app_only = false;
        settings.inputs = Inputs {
            microphone: Some("usb-microphone-7".into()),
            microphone_name: Some("USB microphone".into()),
            system: SystemSound::App {
                id: 0,
                name: "Discord".into(),
            },
        };
        settings.replay.length_seconds = 300;
        settings.replay.save_seconds = 120;
        settings.replay.storage = ReplayStorage::Disk;
        settings.replay.memory_budget_mb = Some(1024);
        settings.replay.buffer_folder = PathBuf::from("/fast/buffer");
        settings.replay.clear_after_save = true;
        settings.replay.start_with_app = true;
        settings.output_folder = PathBuf::from("/clips");
        settings.hotkeys.marker = "ctrl+shift+m".into();
        settings.start_at_login = true;
        settings.announcements = Announcements {
            overlay: false,
            desktop: false,
        };
        settings.selection = Some(Region {
            screen: "screen-69733382".into(),
            area: Area::new(200, 100, 1280, 720),
        });
        settings
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let file = temp_file("missing");
        let mut store = Store::open(&file);
        assert_eq!(store.load(defaults()), defaults());
        assert!(!store.found_damaged());
    }

    #[test]
    fn every_setting_survives_a_save_and_a_load() {
        let file = temp_file("round-trip");
        let mut store = Store::open(&file);
        store.load(defaults());
        store.save(&changed()).expect("settings saved");

        let mut reopened = Store::open(&file);
        assert_eq!(reopened.load(defaults()), changed());
        std::fs::remove_file(&file).expect("temp file removed");
    }

    #[test]
    fn going_back_to_the_automatic_budget_survives_a_load() {
        let file = temp_file("budget");
        let mut store = Store::open(&file);
        store.load(defaults());
        store.save(&changed()).expect("settings saved");
        let mut automatic = changed();
        automatic.replay.memory_budget_mb = None;
        store.save(&automatic).expect("settings saved");

        let mut reopened = Store::open(&file);
        assert_eq!(reopened.load(defaults()).replay.memory_budget_mb, None);
        std::fs::remove_file(&file).expect("temp file removed");
    }

    #[test]
    fn a_damaged_file_gives_the_defaults_and_is_set_aside() {
        let file = temp_file("damaged");
        std::fs::write(&file, b"{ \"video.codec\": \"hev").expect("damaged file written");
        let mut store = Store::open(&file);
        assert_eq!(store.load(defaults()), defaults());
        assert!(store.found_damaged());
        let set_aside = file.with_extension("json.bad");
        assert!(set_aside.exists());
        std::fs::remove_file(set_aside).expect("temp file removed");
    }

    #[test]
    fn values_out_of_range_or_of_the_wrong_kind_fall_back_one_by_one() {
        let file = temp_file("out-of-range");
        std::fs::write(
            &file,
            br#"{
                "video.codec": "vp9",
                "video.quality": "low",
                "video.frame_rate": 500,
                "audio.system.volume": 250,
                "replay.length_seconds": 5,
                "replay.save_seconds": 900,
                "replay.memory_budget_mb": 90000,
                "replay.clear_after_save": "yes",
                "output_folder": ""
            }"#,
        )
        .expect("file written");
        let mut store = Store::open(&file);
        let settings = store.load(defaults());
        assert!(!store.found_damaged());
        assert_eq!(settings.video.codec, Codec::H264);
        assert_eq!(settings.video.quality, Quality::Low);
        assert_eq!(settings.video.frame_rate, 60);
        assert_eq!(settings.audio.system.volume_percent, 100);
        assert_eq!(settings.replay.length_seconds, 15);
        assert_eq!(settings.replay.save_seconds, 15);
        assert_eq!(settings.replay.memory_budget_mb, Some(LARGEST_BUDGET_MB));
        assert!(!settings.replay.clear_after_save);
        assert_eq!(settings.output_folder, defaults().output_folder);
        std::fs::remove_file(&file).expect("temp file removed");
    }

    #[test]
    fn typed_values_between_the_presets_and_a_chosen_output_survive_a_load() {
        let file = temp_file("typed");
        let mut store = Store::open(&file);
        store.load(defaults());
        let mut typed = defaults();
        typed.video.frame_rate = 75;
        typed.replay.length_seconds = 100;
        typed.replay.memory_budget_mb = Some(3000);
        typed.inputs.system = SystemSound::Output(Device {
            id: "speakers-2".into(),
            name: "speakers-2".into(),
            is_default: false,
        });
        store.save(&typed).expect("settings saved");

        let mut reopened = Store::open(&file);
        assert_eq!(reopened.load(defaults()), typed);
        std::fs::remove_file(&file).expect("temp file removed");
    }

    #[test]
    fn a_settings_file_from_before_devices_could_be_chosen_records_the_defaults() {
        let file = temp_file("before-devices");
        std::fs::write(&file, br#"{ "audio.microphone.enabled": true }"#).expect("file written");
        let mut store = Store::open(&file);
        assert_eq!(store.load(defaults()).inputs, Inputs::default());
        std::fs::remove_file(&file).expect("temp file removed");
    }
}
