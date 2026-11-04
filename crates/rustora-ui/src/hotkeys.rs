// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Hotkey.cs` and `HotkeyManager.cs`.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use rustora_core::miniyaml::MiniYamlNode;

use crate::input::{KeyInput, Modifiers};

/// A key named as in the `Keycode` enum of C#, with its modifiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub key: String,
    pub modifiers: Modifiers,
}

impl Hotkey {
    /// Port of `Hotkey.TryParse`.
    pub fn parse(text: &str) -> Option<Hotkey> {
        let text = text.trim();
        let (key, mods) = text.split_once(' ').unwrap_or((text, ""));
        if key.is_empty() {
            return None;
        }
        let mut modifiers = Modifiers::default();
        for part in mods.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "none" => {}
                "shift" => modifiers.shift = true,
                "alt" => modifiers.alt = true,
                "ctrl" => modifiers.ctrl = true,
                "meta" => modifiers.meta = true,
                _ => return None,
            }
        }
        Some(Hotkey {
            key: key.to_ascii_uppercase(),
            modifiers,
        })
    }

    /// Port of `HotkeyReference.IsActivatedBy`.
    pub fn is_activated_by(&self, e: &KeyInput) -> bool {
        self.key == e.key && self.modifiers == e.modifiers
    }
}

/// Port of `HotkeyManager`.
#[derive(Default)]
pub struct Hotkeys {
    keys: HashMap<String, Option<Hotkey>>,
    readonly: HashSet<String>,
}

impl Hotkeys {
    /// Reads the definitions of the `Hotkeys` files of the manifest.
    pub fn load(definitions: &[MiniYamlNode]) -> Hotkeys {
        let mut hotkeys = Hotkeys::default();
        for definition in definitions {
            hotkeys
                .keys
                .insert(definition.key.clone(), Hotkey::parse(definition.value.value_str()));
            let readonly = definition
                .value
                .nodes
                .iter()
                .any(|n| n.key == "Readonly" && n.value.value_str().eq_ignore_ascii_case("true"));
            if readonly {
                hotkeys.readonly.insert(definition.key.clone());
            }
        }
        hotkeys
    }

    /// Applies the `Keys` section of the settings.
    pub fn apply_settings(&mut self, settings: &IndexMap<String, String>) {
        for (name, value) in settings {
            if self.keys.contains_key(name) && !self.readonly.contains(name) {
                self.keys.insert(name.clone(), Hotkey::parse(value));
            }
        }
    }

    /// Port of `GetHotkeyReference`: a defined hotkey, else a key given by name.
    pub fn get(&self, name: &str) -> Option<Hotkey> {
        match self.keys.get(name) {
            Some(hotkey) => hotkey.clone(),
            None => Hotkey::parse(name),
        }
    }

    pub fn is_activated(&self, name: &str, e: &KeyInput) -> bool {
        self.get(name).is_some_and(|hotkey| hotkey.is_activated_by(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::KeyEvent;

    fn key(key: &str, modifiers: Modifiers) -> KeyInput {
        KeyInput {
            event: KeyEvent::Down,
            key: key.to_string(),
            modifiers,
            multi_tap: 1,
            repeat: false,
        }
    }

    #[test]
    fn parses_names_and_modifiers() {
        let hotkey = Hotkey::parse("f5 Ctrl, Shift").unwrap();
        assert_eq!(hotkey.key, "F5");
        assert!(hotkey.modifiers.ctrl && hotkey.modifiers.shift && !hotkey.modifiers.alt);
        assert!(Hotkey::parse("").is_none());
        assert!(Hotkey::parse("F5 Hyper").is_none());
    }

    #[test]
    fn settings_override_definitions_except_readonly() {
        let nodes = rustora_core::miniyaml::MiniYaml::from_str("ZoomIn: PAGEUP\nFixed: F1\n\tReadonly: true\n", "t", true).unwrap();
        let mut hotkeys = Hotkeys::load(&nodes);
        let mut settings = IndexMap::new();
        settings.insert("ZoomIn".to_string(), "Z Ctrl".to_string());
        settings.insert("Fixed".to_string(), "F2".to_string());
        hotkeys.apply_settings(&settings);
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert!(hotkeys.is_activated("ZoomIn", &key("Z", ctrl)));
        assert!(hotkeys.is_activated("Fixed", &key("F1", Modifiers::default())));
        assert!(hotkeys.is_activated("tab", &key("TAB", Modifiers::default())));
    }
}
