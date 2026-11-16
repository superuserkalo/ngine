use std::collections::HashMap;
use std::path::{Path, PathBuf};

const COLORS_FILE: &str = "colors.toml";
const LIGHT_MARKER: &str = "light.mode";
const SURFACE_LIFT: u16 = 60;
const BORDER_LIFT: u16 = 180;
const MUTED_TEXT_FADE: u16 = 450;
const WHOLE: u16 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Reads `#rrggbb` or `rrggbb`.
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#').unwrap_or(text.trim());
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        Some(Self::new(channel(0)?, channel(2)?, channel(4)?))
    }

    /// Moves `per_mille` thousandths of the way from this colour to
    /// `toward`.
    pub fn mix(self, toward: Self, per_mille: u16) -> Self {
        let share = i32::from(per_mille.min(WHOLE));
        let whole = i32::from(WHOLE);
        let blend = |from: u8, to: u8| {
            let (from, to) = (i32::from(from), i32::from(to));
            let mixed = (from * whole + (to - from) * share + whole / 2).div_euclid(whole);
            u8::try_from(mixed).unwrap_or(u8::MAX)
        };
        Self::new(
            blend(self.r, toward.r),
            blend(self.g, toward.g),
            blend(self.b, toward.b),
        )
    }
}

/// The colours of the current Omarchy theme, by the role they play in
/// ngine's window. Every role is filled: one the theme does not name is
/// derived from the ones it does, or taken from Omarchy's own defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColors {
    pub light: bool,
    pub background: Rgb,
    pub surface: Rgb,
    pub border: Rgb,
    pub text: Rgb,
    pub muted_text: Rgb,
    pub accent: Rgb,
    pub recording: Rgb,
    pub warning: Rgb,
}

const DEFAULT_BACKGROUND: Rgb = Rgb::new(0x10, 0x13, 0x15);
const DEFAULT_TEXT: Rgb = Rgb::new(0xca, 0xcc, 0xcc);
const DEFAULT_RECORDING: Rgb = Rgb::new(0xa5, 0x55, 0x55);
const DEFAULT_WARNING: Rgb = Rgb::new(0xe0, 0xaf, 0x68);

fn entries(colors_toml: &str) -> HashMap<&str, &str> {
    colors_toml
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with('[') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|rest| rest.split_once('"'))
                .map_or(value, |(inside, _)| inside);
            Some((key.trim(), value))
        })
        .collect()
}

impl ThemeColors {
    /// Reads a theme's `colors.toml`, in Omarchy 4's names (`foreground`,
    /// `red`, `lighter_background` and the rest) or Omarchy 3's (`foreground`
    /// and `color0` to `color15`). `light_marker` is whether the theme folder
    /// holds Omarchy 3's `light.mode` file.
    pub fn parse(colors_toml: &str, light_marker: bool) -> Self {
        let entries = entries(colors_toml);
        let color = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| entries.get(key).and_then(|value| Rgb::parse(value)))
        };
        let background = color(&["background", "bg", "color0"]).unwrap_or(DEFAULT_BACKGROUND);
        let text = color(&["foreground", "fg", "color7"]).unwrap_or(DEFAULT_TEXT);
        let accent = color(&["accent", "color4", "blue"]).unwrap_or(text);
        Self {
            light: entries.get("mode").is_some_and(|mode| *mode == "light") || light_marker,
            background,
            surface: color(&["lighter_background", "lighter_bg"])
                .unwrap_or_else(|| background.mix(text, SURFACE_LIFT)),
            border: color(&["muted", "color8"])
                .unwrap_or_else(|| background.mix(text, BORDER_LIFT)),
            text,
            muted_text: color(&["dark_foreground", "dark_fg"])
                .unwrap_or_else(|| text.mix(background, MUTED_TEXT_FADE)),
            accent,
            recording: color(&["red", "color1"]).unwrap_or(DEFAULT_RECORDING),
            warning: color(&["yellow", "color3"]).unwrap_or(DEFAULT_WARNING),
        }
    }
}

/// The folders where Omarchy keeps the current theme, newest layout first:
/// Omarchy 4 under the state folder, Omarchy 3 under the config folder.
pub fn current_folders(home: &Path, state_home: Option<&Path>) -> [PathBuf; 2] {
    let state_home = state_home.map_or_else(|| home.join(".local/state"), Path::to_path_buf);
    [
        state_home.join("omarchy/current"),
        home.join(".config/omarchy/current"),
    ]
}

/// The current Omarchy theme's colours, or `None` when this is not Omarchy
/// or no theme is set.
pub fn read(home: &Path, state_home: Option<&Path>) -> Option<ThemeColors> {
    current_folders(home, state_home)
        .iter()
        .map(|current| current.join("theme"))
        .find_map(|theme| {
            let colors = std::fs::read_to_string(theme.join(COLORS_FILE)).ok()?;
            Some(ThemeColors::parse(
                &colors,
                theme.join(LIGHT_MARKER).exists(),
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO_NIGHT_4: &str = include_str!("../../fixtures/omarchy/tokyo-night-4.toml");
    const TOKYO_NIGHT_3: &str = include_str!("../../fixtures/omarchy/tokyo-night-3.toml");
    const LATTE_4: &str = include_str!("../../fixtures/omarchy/catppuccin-latte-4.toml");
    const LATTE_3: &str = include_str!("../../fixtures/omarchy/catppuccin-latte-3.toml");

    fn hex(text: &str) -> Rgb {
        Rgb::parse(text).expect("a colour")
    }

    #[test]
    fn an_omarchy_4_theme_fills_every_role_from_its_own_names() {
        let colors = ThemeColors::parse(TOKYO_NIGHT_4, false);
        assert_eq!(
            colors,
            ThemeColors {
                light: false,
                background: hex("#1a1b26"),
                surface: hex("#24283b"),
                border: hex("#414868"),
                text: hex("#a9b1d6"),
                muted_text: hex("#565f89"),
                accent: hex("#7aa2f7"),
                recording: hex("#f7768e"),
                warning: hex("#e0af68"),
            }
        );
    }

    #[test]
    fn an_omarchy_3_theme_maps_its_numbered_colours_and_derives_the_rest() {
        let colors = ThemeColors::parse(TOKYO_NIGHT_3, false);
        assert!(!colors.light);
        assert_eq!(colors.background, hex("#1a1b26"));
        assert_eq!(colors.text, hex("#a9b1d6"));
        assert_eq!(colors.accent, hex("#7aa2f7"));
        assert_eq!(colors.recording, hex("#f7768e"));
        assert_eq!(colors.warning, hex("#e0af68"));
        assert_eq!(colors.border, hex("#444b6a"));
        assert_eq!(
            colors.surface,
            hex("#1a1b26").mix(hex("#a9b1d6"), SURFACE_LIFT)
        );
        assert_eq!(
            colors.muted_text,
            hex("#a9b1d6").mix(hex("#1a1b26"), MUTED_TEXT_FADE)
        );
    }

    #[test]
    fn light_themes_say_so_in_either_layout() {
        let latte = ThemeColors::parse(LATTE_4, false);
        assert!(latte.light);
        assert_eq!(latte.background, hex("#eff1f5"));
        assert_eq!(latte.recording, hex("#d20f39"));
        assert!(!ThemeColors::parse(LATTE_3, false).light);
        assert!(ThemeColors::parse(LATTE_3, true).light);
    }

    #[test]
    fn an_empty_or_broken_file_gives_omarchys_defaults() {
        let colors = ThemeColors::parse("accent = \"not a colour\"\n[section]\nnonsense", false);
        assert_eq!(colors.background, DEFAULT_BACKGROUND);
        assert_eq!(colors.text, DEFAULT_TEXT);
        assert_eq!(colors.accent, DEFAULT_TEXT);
        assert_eq!(colors.recording, DEFAULT_RECORDING);
        assert_eq!(colors.warning, DEFAULT_WARNING);
    }

    #[test]
    fn colours_parse_and_mix() {
        assert_eq!(Rgb::parse("#ff8000"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(Rgb::parse("ff8000"), Some(Rgb::new(255, 128, 0)));
        for bad in ["", "#fff", "#gg0000", "#ff80001", "#ÿÿÿ"] {
            assert_eq!(Rgb::parse(bad), None, "{bad}");
        }
        let black = Rgb::new(0, 0, 0);
        let white = Rgb::new(255, 255, 255);
        assert_eq!(black.mix(white, 500), Rgb::new(128, 128, 128));
        assert_eq!(black.mix(white, 2000), white);
    }

    #[test]
    fn the_newest_layout_wins_and_a_missing_theme_reads_as_none() {
        let home = std::env::temp_dir().join(format!("ngine-omarchy-theme-{}", std::process::id()));
        assert_eq!(read(&home, None), None);
        let [state, config] = current_folders(&home, None);
        std::fs::create_dir_all(config.join("theme")).expect("a theme folder");
        std::fs::write(config.join("theme").join(COLORS_FILE), LATTE_3).expect("colours written");
        std::fs::write(config.join("theme").join(LIGHT_MARKER), "").expect("marker written");
        assert!(read(&home, None).is_some_and(|colors| colors.light));
        std::fs::create_dir_all(state.join("theme")).expect("a theme folder");
        std::fs::write(state.join("theme").join(COLORS_FILE), TOKYO_NIGHT_4)
            .expect("colours written");
        assert_eq!(
            read(&home, None),
            Some(ThemeColors::parse(TOKYO_NIGHT_4, false))
        );
        std::fs::remove_dir_all(&home).expect("temp folder removed");
    }
}
