use std::time::Duration;

use rune::TextEdit;

use super::Ngine;
use crate::cli;
use crate::format;
use crate::settings::{
    FRAME_RATES, HIGHEST_FRAME_RATE, LARGEST_BUDGET_MB, LONGEST_REPLAY, LOWEST_FRAME_RATE,
    MEMORY_BUDGETS_MB, REPLAY_LENGTHS, SHORTEST_REPLAY, SMALLEST_BUDGET_MB, Settings,
};

const SETTLE_SECONDS: f64 = 0.8;
const MB_PER_GB: f64 = 1000.0;
const AUTOMATIC: &str = "auto";

/// A setting people may set to a number of their own, typed with its unit,
/// with a few presets beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Amount {
    FrameRate,
    ReplayLength,
    SaveLength,
    MemoryBudget,
}

/// What a typed amount sets: a number, or the automatic choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Typed {
    Value(u32),
    Automatic,
}

impl Amount {
    pub fn key(self) -> &'static str {
        match self {
            Self::FrameRate => "settings-frame-rate",
            Self::ReplayLength => "settings-replay-length",
            Self::SaveLength => "settings-save-length",
            Self::MemoryBudget => "settings-memory-budget",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FrameRate => "Frame rate",
            Self::ReplayLength => "Replay length",
            Self::SaveLength => "Custom save length",
            Self::MemoryBudget => "Memory budget",
        }
    }

    fn range(self, settings: &Settings) -> (u32, u32) {
        match self {
            Self::FrameRate => (u32::from(LOWEST_FRAME_RATE), u32::from(HIGHEST_FRAME_RATE)),
            Self::ReplayLength => (u32::from(SHORTEST_REPLAY), u32::from(LONGEST_REPLAY)),
            Self::SaveLength => (
                u32::from(SHORTEST_REPLAY),
                u32::from(settings.replay.length_seconds),
            ),
            Self::MemoryBudget => (SMALLEST_BUDGET_MB, LARGEST_BUDGET_MB),
        }
    }

    /// The presets offered beside the field.
    pub fn presets(self, settings: &Settings) -> Vec<Typed> {
        let values: Vec<u32> = match self {
            Self::FrameRate => FRAME_RATES.iter().map(|rate| u32::from(*rate)).collect(),
            Self::ReplayLength => REPLAY_LENGTHS.iter().map(|s| u32::from(*s)).collect(),
            Self::SaveLength => REPLAY_LENGTHS
                .iter()
                .map(|s| u32::from(*s))
                .filter(|s| *s <= u32::from(settings.replay.length_seconds))
                .collect(),
            Self::MemoryBudget => MEMORY_BUDGETS_MB.to_vec(),
        };
        let mut presets: Vec<Typed> = values.into_iter().map(Typed::Value).collect();
        if self == Self::MemoryBudget {
            presets.insert(0, Typed::Automatic);
        }
        presets
    }

    /// The setting as the field shows it.
    pub fn current(self, settings: &Settings) -> Typed {
        match self {
            Self::FrameRate => Typed::Value(u32::from(settings.video.frame_rate)),
            Self::ReplayLength => Typed::Value(u32::from(settings.replay.length_seconds)),
            Self::SaveLength => Typed::Value(u32::from(settings.replay.save_seconds)),
            Self::MemoryBudget => settings
                .replay
                .memory_budget_mb
                .map_or(Typed::Automatic, Typed::Value),
        }
    }

    pub fn show(self, typed: Typed, automatic_mb: u32) -> String {
        match (self, typed) {
            (Self::FrameRate, Typed::Value(rate)) => format!("{rate} fps"),
            (Self::ReplayLength | Self::SaveLength, Typed::Value(seconds)) => {
                format::length(Duration::from_secs(u64::from(seconds)))
            }
            (Self::MemoryBudget, Typed::Value(mb)) => format::megabytes(u64::from(mb)),
            (_, Typed::Automatic) => {
                format!("Automatic, {}", format::megabytes(u64::from(automatic_mb)))
            }
        }
    }

    /// What the field shows while nobody types in it.
    pub fn text(self, settings: &Settings, automatic_mb: u32) -> String {
        match self.current(settings) {
            Typed::Automatic => "Automatic".to_owned(),
            typed @ Typed::Value(_) => self.show(typed, automatic_mb),
        }
    }

    /// Why `typed` cannot be set, in the words the field shows under it.
    pub fn range_words(self, settings: &Settings) -> String {
        let (low, high) = self.range(settings);
        match self {
            Self::FrameRate => format!("Type a rate from {low} to {high} fps"),
            Self::ReplayLength | Self::SaveLength => format!(
                "Type a length from {} to {}, such as 90 s or 2 min",
                format::length(Duration::from_secs(u64::from(low))),
                format::length(Duration::from_secs(u64::from(high)))
            ),
            Self::MemoryBudget => format!(
                "Type a size from {} to {}, or auto",
                format::megabytes(u64::from(low)),
                format::megabytes(u64::from(high))
            ),
        }
    }

    /// Reads what was typed. A number without a unit is in the field's
    /// unit: frames a second, seconds, or megabytes.
    pub fn parse(self, text: &str, settings: &Settings) -> Option<Typed> {
        let words = text.trim().to_lowercase();
        let value = match self {
            Self::FrameRate => number_before(&words, &["fps", "hz"])?,
            Self::ReplayLength | Self::SaveLength => seconds(&words)?,
            Self::MemoryBudget if words.is_empty() || words.starts_with(AUTOMATIC) => {
                return Some(Typed::Automatic);
            }
            Self::MemoryBudget => megabytes(&words)?,
        };
        let (low, high) = self.range(settings);
        (low..=high).contains(&value).then_some(Typed::Value(value))
    }

    pub fn apply(self, typed: Typed, settings: &mut Settings) {
        let small = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
        match (self, typed) {
            (Self::FrameRate, Typed::Value(rate)) => settings.video.frame_rate = small(rate),
            (Self::ReplayLength, Typed::Value(seconds)) => {
                settings.replay.length_seconds = small(seconds);
            }
            (Self::SaveLength, Typed::Value(seconds)) => {
                settings.replay.save_seconds = small(seconds);
            }
            (Self::MemoryBudget, Typed::Value(mb)) => settings.replay.memory_budget_mb = Some(mb),
            (Self::MemoryBudget, Typed::Automatic) => settings.replay.memory_budget_mb = None,
            (_, Typed::Automatic) => {}
        }
    }
}

fn number_before(words: &str, units: &[&str]) -> Option<u32> {
    let mut digits = words;
    for unit in units {
        if let Some(stripped) = digits.strip_suffix(unit) {
            digits = stripped;
            break;
        }
    }
    digits.trim().parse().ok()
}

fn seconds(words: &str) -> Option<u32> {
    let compact: String = words
        .replace("minutes", "m")
        .replace("minute", "m")
        .replace("mins", "m")
        .replace("min", "m")
        .replace("seconds", "s")
        .replace("second", "s")
        .replace("sec", "s")
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if let Some((minutes, rest)) = compact.split_once(':') {
        let minutes: u32 = minutes.parse().ok()?;
        let rest: u32 = rest.parse().ok()?;
        return (rest < 60).then(|| minutes.checked_mul(60)?.checked_add(rest))?;
    }
    let length = cli::parse_length(&compact)?;
    u32::try_from(length.as_secs()).ok()
}

fn megabytes(words: &str) -> Option<u32> {
    let compact: String = words.chars().filter(|c| !c.is_whitespace()).collect();
    let (number, scale) = if let Some(number) = compact.strip_suffix("gb") {
        (number, MB_PER_GB)
    } else if let Some(number) = compact.strip_suffix('g') {
        (number, MB_PER_GB)
    } else if let Some(number) = compact.strip_suffix("mb") {
        (number, 1.0)
    } else {
        (compact.strip_suffix('m').unwrap_or(&compact), 1.0)
    };
    let value: f64 = number.replace(',', ".").parse().ok()?;
    let scaled = (value * scale).round();
    (scaled.is_finite() && scaled >= 0.0 && scaled <= f64::from(u32::MAX))
        .then(|| format!("{scaled:.0}").parse().ok())?
}

/// A number being typed into an amount field. It is set once it reads as a
/// value in range and typing has paused, or at once on Return.
pub struct Draft {
    pub amount: Amount,
    pub edit: TextEdit,
    typed_at: f64,
    typed: bool,
}

impl Draft {
    /// Whether something was typed, rather than the field only read.
    pub fn typed(&self) -> bool {
        self.typed
    }

    /// What is wrong with the typed text, if anything.
    pub fn problem(&self, settings: &Settings) -> Option<String> {
        self.amount
            .parse(&self.edit.text, settings)
            .is_none()
            .then(|| self.amount.range_words(settings))
    }
}

impl Ngine {
    /// The field of `amount`, starting a draft with the current value.
    pub fn draft_of(&mut self, amount: Amount) -> &mut TextEdit {
        let fresh = self
            .draft
            .as_ref()
            .is_none_or(|draft| draft.amount != amount);
        if fresh {
            let text = amount.text(&self.settings, self.memory_budget_mb());
            self.draft = Some(Draft {
                amount,
                edit: TextEdit::new(text),
                typed_at: self.clock,
                typed: false,
            });
        }
        let clock = self.clock;
        let draft = self.draft.get_or_insert_with(|| Draft {
            amount,
            edit: TextEdit::new(""),
            typed_at: clock,
            typed: false,
        });
        &mut draft.edit
    }

    pub fn drafted(&mut self) {
        if let Some(draft) = &mut self.draft {
            draft.typed_at = self.clock;
            draft.typed = true;
        }
    }

    /// Forgets a field that was only read, so it shows the setting again.
    pub(super) fn forget_untyped_draft(&mut self) {
        if self.draft.as_ref().is_some_and(|draft| !draft.typed) {
            self.draft = None;
        }
    }

    /// Sets what was typed when it is a value in range. Returns whether it
    /// was set.
    pub fn commit_draft(&mut self) -> bool {
        let Some(draft) = self.draft.as_ref().filter(|draft| draft.typed) else {
            return false;
        };
        let amount = draft.amount;
        let Some(typed) = amount.parse(&draft.edit.text, &self.settings) else {
            return false;
        };
        self.draft = None;
        self.edit(|settings| amount.apply(typed, settings));
        true
    }

    pub fn choose_amount(&mut self, amount: Amount, typed: Typed) {
        self.draft = None;
        self.popup = None;
        self.edit(|settings| amount.apply(typed, settings));
    }

    pub fn discard_draft(&mut self) {
        self.draft = None;
    }

    pub(super) fn settle_draft(&mut self) -> bool {
        let settled = self
            .draft
            .as_ref()
            .is_some_and(|draft| draft.typed && self.clock - draft.typed_at >= SETTLE_SECONDS);
        settled && self.commit_draft()
    }

    pub(super) fn draft_waits(&self) -> bool {
        self.draft.as_ref().is_some_and(|draft| {
            draft.typed
                && self.clock - draft.typed_at < SETTLE_SECONDS
                && draft.problem(&self.settings).is_none()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::demo;

    fn settings() -> Settings {
        demo::settings()
    }

    #[test]
    fn a_frame_rate_is_typed_with_or_without_its_unit_and_stays_in_range() {
        let s = settings();
        let rate = |text| Amount::FrameRate.parse(text, &s);
        assert_eq!(rate("75"), Some(Typed::Value(75)));
        assert_eq!(rate("144 fps"), Some(Typed::Value(144)));
        assert_eq!(rate(" 50Hz "), Some(Typed::Value(50)));
        assert_eq!(rate("5"), None);
        assert_eq!(rate("500"), None);
        assert_eq!(rate("fast"), None);
    }

    #[test]
    fn a_length_reads_as_people_write_it() {
        let s = settings();
        let length = |text| Amount::ReplayLength.parse(text, &s);
        assert_eq!(length("90"), Some(Typed::Value(90)));
        assert_eq!(length("2 min"), Some(Typed::Value(120)));
        assert_eq!(length("1 min 30 s"), Some(Typed::Value(90)));
        assert_eq!(length("1:45"), Some(Typed::Value(105)));
        assert_eq!(length("10m"), Some(Typed::Value(600)));
        assert_eq!(length("5 s"), None);
        assert_eq!(length("2 hours"), None);
        assert_eq!(length("1:75"), None);
    }

    #[test]
    fn a_save_length_is_no_longer_than_the_replay() {
        let mut s = settings();
        s.replay.length_seconds = 60;
        assert_eq!(Amount::SaveLength.parse("45 s", &s), Some(Typed::Value(45)));
        assert_eq!(Amount::SaveLength.parse("2 min", &s), None);
        assert!(
            Amount::SaveLength
                .presets(&s)
                .iter()
                .all(|preset| *preset <= Typed::Value(60) || *preset == Typed::Automatic)
        );
    }

    #[test]
    fn a_budget_is_in_megabytes_unless_it_says_gigabytes_and_empty_means_automatic() {
        let s = settings();
        let budget = |text| Amount::MemoryBudget.parse(text, &s);
        assert_eq!(budget("768"), Some(Typed::Value(768)));
        assert_eq!(budget("1.5 GB"), Some(Typed::Value(1500)));
        assert_eq!(budget("3g"), Some(Typed::Value(3000)));
        assert_eq!(budget(""), Some(Typed::Automatic));
        assert_eq!(budget("Automatic"), Some(Typed::Automatic));
        assert_eq!(budget("64 MB"), None);
        assert_eq!(budget("40 GB"), None);
    }

    #[test]
    fn a_typed_value_is_set_after_a_pause_or_on_return_and_a_wrong_one_never() {
        let mut app = demo::idle();
        app.draft_of(Amount::FrameRate).text = "75".into();
        app.drafted();
        assert!(app.draft_waits());
        assert!(!app.settle_draft());
        app.clock += SETTLE_SECONDS;
        assert!(app.settle_draft());
        assert_eq!(app.settings.video.frame_rate, 75);
        assert!(app.draft.is_none());

        app.draft_of(Amount::FrameRate).text = "900".into();
        app.drafted();
        assert!(!app.commit_draft());
        assert_eq!(app.settings.video.frame_rate, 75);
        assert_eq!(
            app.draft
                .as_ref()
                .and_then(|draft| draft.problem(&app.settings)),
            Some("Type a rate from 10 to 240 fps".to_owned())
        );
        app.discard_draft();
        assert_eq!(
            Amount::FrameRate.text(&app.settings, 512),
            "75 fps".to_owned()
        );
    }
}
