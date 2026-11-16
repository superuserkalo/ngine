use rune::{Mods, PhysicalInput, PhysicalKeyCode, Shortcut};

use super::Ngine;
use crate::hotkeys;

/// A hotkey field that is waiting for the keys to use.
#[derive(Clone, Debug, PartialEq)]
pub struct HotkeyCapture {
    pub slot: usize,
    pub held: Mods,
    pub refusal: Option<String>,
}

fn is_modifier(key: PhysicalKeyCode) -> bool {
    matches!(
        key,
        PhysicalKeyCode::ShiftLeft
            | PhysicalKeyCode::ShiftRight
            | PhysicalKeyCode::ControlLeft
            | PhysicalKeyCode::ControlRight
            | PhysicalKeyCode::AltLeft
            | PhysicalKeyCode::AltRight
            | PhysicalKeyCode::SuperLeft
            | PhysicalKeyCode::SuperRight
            | PhysicalKeyCode::Fn
            | PhysicalKeyCode::FnLock
    )
}

impl Ngine {
    pub fn begin_hotkey(&mut self, slot: usize) {
        self.popup = None;
        self.capture = Some(HotkeyCapture {
            slot,
            held: Mods::default(),
            refusal: None,
        });
    }

    pub fn cancel_hotkey(&mut self) {
        self.capture = None;
    }

    pub fn is_capturing(&self, slot: usize) -> bool {
        self.capture
            .as_ref()
            .is_some_and(|capture| capture.slot == slot)
    }

    fn refuse_hotkey(&mut self, slot: usize, why: String) {
        self.capture = Some(HotkeyCapture {
            slot,
            held: Mods::default(),
            refusal: Some(why),
        });
    }

    /// Takes the keys pressed for the hotkey in `slot`. A combination another
    /// action has, or one the system refuses, is not kept: the hotkey stays as
    /// it was and the field says why.
    pub fn propose_hotkey(&mut self, slot: usize, shortcut: &Shortcut) {
        let keys = shortcut.keycaps().join(" ");
        if shortcut.require_modifier().is_err() {
            return self.refuse_hotkey(slot, hotkeys::NEEDS_MODIFIER.to_owned());
        }
        if let Some(other) = hotkeys::taken_by(&self.settings.hotkeys, slot, shortcut) {
            let action = hotkeys::label(other, &self.settings);
            return self.refuse_hotkey(slot, format!("{keys} is already used for “{action}”"));
        }
        let before = self.settings.hotkeys.all()[slot].to_owned();
        let accelerator = shortcut.to_string();
        self.edit(|settings| settings.hotkeys.set(slot, accelerator));
        if let Some(problem) = self.hotkey_problems[slot].clone() {
            self.edit(|settings| settings.hotkeys.set(slot, before));
            return self.refuse_hotkey(slot, format!("{keys} was not accepted: {problem}"));
        }
        self.capture = None;
    }

    pub fn clear_hotkey(&mut self, slot: usize) {
        self.capture = None;
        self.edit(|settings| settings.hotkeys.set(slot, String::new()));
    }

    /// The keyboard while a hotkey field waits for keys. Returns whether the
    /// input was taken, so nothing else acts on it.
    pub fn hotkey_input(&mut self, slot: usize, input: PhysicalInput) -> bool {
        let Some(capture) = self.capture.as_mut().filter(|capture| capture.slot == slot) else {
            return false;
        };
        match input {
            PhysicalInput::FocusLost => self.capture = None,
            PhysicalInput::Modifiers(held) => capture.held = held,
            PhysicalInput::Key {
                key,
                mods,
                pressed,
                repeat,
            } => {
                capture.held = mods;
                if !pressed || repeat || is_modifier(key) {
                    return true;
                }
                let bare = mods == Mods::default();
                match key {
                    PhysicalKeyCode::Escape if bare => self.capture = None,
                    PhysicalKeyCode::Backspace | PhysicalKeyCode::Delete if bare => {
                        self.clear_hotkey(slot);
                    }
                    _ => match Shortcut::from_physical_key(key, mods) {
                        Ok(shortcut) => self.propose_hotkey(slot, &shortcut),
                        Err(_) => self.refuse_hotkey(slot, hotkeys::UNUSABLE.to_owned()),
                    },
                }
            }
        }
        true
    }
}
