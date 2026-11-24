use std::time::Duration;

use rune::Shortcut;

use crate::app::Action;
use crate::format;
use crate::settings::{Hotkeys, SHORT_SAVE_SECONDS, Settings};

pub const COUNT: usize = Hotkeys::COUNT;
pub const RECORD: usize = 0;
pub const SELECTION: usize = 1;
pub const SAVE_SHORT: usize = 2;
pub const SAVE_CUSTOM: usize = 3;
pub const SAVE_EVERYTHING: usize = 4;
pub const MARKER: usize = 5;

pub const ACTIONS: [Action; COUNT] = [
    Action::ToggleRecording,
    Action::RecordSelection,
    Action::SaveShort,
    Action::SaveCustom,
    Action::SaveEverything,
    Action::AddMarker,
];

pub const UNUSABLE: &str = "ngine cannot use this hotkey";
pub const NOT_OFFERED: &str = "This desktop does not offer global shortcuts";
pub const USED_TWICE: &str = "Two actions share this hotkey";
pub use crate::platform::NEEDS_MODIFIER;

fn seconds(seconds: u16) -> String {
    format::length(Duration::from_secs(u64::from(seconds)))
}

/// What the hotkey in `slot` does, in the words every screen uses for it.
pub fn label(slot: usize, settings: &Settings) -> String {
    match slot {
        RECORD => "Start or stop recording".into(),
        SELECTION => "Record a selection".into(),
        SAVE_SHORT => format!("Save last {}", seconds(SHORT_SAVE_SECONDS)),
        SAVE_CUSTOM => format!("Save last {}", seconds(settings.replay.save_seconds)),
        SAVE_EVERYTHING => "Save whole replay".into(),
        _ => "Add a marker".into(),
    }
}

pub fn parse(accelerator: &str) -> Option<Shortcut> {
    accelerator
        .parse::<Shortcut>()
        .ok()
        .filter(|shortcut| shortcut.require_modifier().is_ok())
}

/// What the system does with `shortcut` when it keeps it for itself.
pub fn reserved(shortcut: &Shortcut) -> Option<&'static str> {
    crate::platform::RESERVED_SHORTCUTS
        .iter()
        .find(|(accelerator, _)| accelerator.parse::<Shortcut>().ok().as_ref() == Some(shortcut))
        .map(|(_, what)| *what)
}

fn lacks_modifier(accelerator: &str) -> bool {
    accelerator
        .parse::<Shortcut>()
        .is_ok_and(|shortcut| shortcut.require_modifier().is_err())
}

pub fn is_unset(accelerator: &str) -> bool {
    accelerator.trim().is_empty()
}

/// What is wrong with each hotkey before the system is asked for it. A hotkey
/// that is not set has nothing wrong with it.
pub fn problems(hotkeys: &Hotkeys) -> [Option<String>; COUNT] {
    let all = hotkeys.all();
    let parsed = all.map(parse);
    std::array::from_fn(|slot| match &parsed[slot] {
        None if is_unset(all[slot]) => None,
        None if lacks_modifier(all[slot]) => Some(NEEDS_MODIFIER.to_owned()),
        None => Some(UNUSABLE.to_owned()),
        Some(shortcut) => {
            let uses = parsed
                .iter()
                .filter(|other| other.as_ref() == Some(shortcut))
                .count();
            (uses > 1).then(|| USED_TWICE.to_owned())
        }
    })
}

/// The other action that already has `shortcut`, if any.
pub fn taken_by(hotkeys: &Hotkeys, slot: usize, shortcut: &Shortcut) -> Option<usize> {
    hotkeys
        .all()
        .into_iter()
        .enumerate()
        .find(|(other, accelerator)| {
            *other != slot && parse(accelerator).as_ref() == Some(shortcut)
        })
        .map(|(other, _)| other)
}

pub fn keycaps(accelerator: &str) -> Option<Vec<String>> {
    parse(accelerator).map(|shortcut| shortcut.keycaps())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn defaults() -> Hotkeys {
        Settings::defaults(PathBuf::new(), PathBuf::new()).hotkeys
    }

    #[test]
    fn the_default_hotkeys_are_usable_and_distinct() {
        assert_eq!(problems(&defaults()), [const { None }; COUNT]);
    }

    #[test]
    fn a_shortcut_without_a_modifier_or_with_an_unknown_key_is_named() {
        let mut hotkeys = defaults();
        hotkeys.record = "r".into();
        hotkeys.marker = "ctrl+alt+banana".into();
        let found = problems(&hotkeys);
        assert_eq!(found[RECORD].as_deref(), Some(NEEDS_MODIFIER));
        assert_eq!(found[MARKER].as_deref(), Some(UNUSABLE));
        assert_eq!(found[SAVE_SHORT], None);
    }

    #[test]
    fn two_actions_on_one_shortcut_are_both_named() {
        let mut hotkeys = defaults();
        hotkeys.save_short = hotkeys.record.clone();
        let found = problems(&hotkeys);
        assert_eq!(found[RECORD].as_deref(), Some(USED_TWICE));
        assert_eq!(found[SAVE_SHORT].as_deref(), Some(USED_TWICE));
    }

    #[test]
    fn a_hotkey_that_is_not_set_is_no_problem_and_shares_with_nobody() {
        let mut hotkeys = defaults();
        hotkeys.marker = String::new();
        hotkeys.save_everything = String::new();
        assert_eq!(problems(&hotkeys), [const { None }; COUNT]);
        assert_eq!(keycaps(&hotkeys.marker), None);
    }

    #[test]
    fn a_shortcut_is_written_the_way_it_is_read() {
        for accelerator in defaults().all() {
            let shortcut = parse(accelerator).expect("a default parses");
            assert_eq!(shortcut.to_string(), accelerator);
        }
    }

    #[test]
    fn the_action_that_already_has_a_shortcut_is_found() {
        let hotkeys = defaults();
        let record = parse(&hotkeys.record).expect("the default parses");
        assert_eq!(taken_by(&hotkeys, MARKER, &record), Some(RECORD));
        assert_eq!(taken_by(&hotkeys, RECORD, &record), None);
    }
}
