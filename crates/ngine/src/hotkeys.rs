use rune::Shortcut;

use crate::app::Action;
use crate::settings::Hotkeys;

pub const COUNT: usize = Hotkeys::COUNT;

pub const ACTIONS: [Action; COUNT] = [
    Action::ToggleRecording,
    Action::SaveShort,
    Action::SaveCustom,
    Action::SaveEverything,
    Action::AddMarker,
];

pub const UNUSABLE: &str = "ngine cannot use this shortcut. Write it like ctrl+alt+r";
pub const USED_TWICE: &str = "This shortcut is used twice";

pub fn parse(accelerator: &str) -> Option<Shortcut> {
    accelerator
        .parse::<Shortcut>()
        .ok()
        .filter(|shortcut| shortcut.require_modifier().is_ok())
}

pub fn problems(hotkeys: &Hotkeys) -> [Option<String>; COUNT] {
    let parsed = hotkeys.all().map(parse);
    std::array::from_fn(|slot| match &parsed[slot] {
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

pub fn keycaps(accelerator: &str) -> Option<String> {
    parse(accelerator).map(|shortcut| shortcut.keycaps().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
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
        assert_eq!(found[0].as_deref(), Some(UNUSABLE));
        assert_eq!(found[4].as_deref(), Some(UNUSABLE));
        assert_eq!(found[1], None);
    }

    #[test]
    fn two_actions_on_one_shortcut_are_both_named() {
        let mut hotkeys = defaults();
        hotkeys.save_short = hotkeys.record.clone();
        let found = problems(&hotkeys);
        assert_eq!(found[0].as_deref(), Some(USED_TWICE));
        assert_eq!(found[1].as_deref(), Some(USED_TWICE));
    }
}
