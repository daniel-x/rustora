// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Hotkey.cs` and `HotkeyManager.cs`.

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
        let key = key.to_ascii_uppercase();
        (key != "UNKNOWN").then_some(Hotkey { key, modifiers })
    }

    /// Port of `Hotkey.FromKeyInput`.
    pub fn from_key_input(e: &KeyInput) -> Hotkey {
        Hotkey {
            key: e.key.clone(),
            modifiers: e.modifiers,
        }
    }

    /// Port of `Hotkey.ToString` as the settings file stores it, `UNKNOWN None` for an invalid hotkey.
    pub fn setting_string(hotkey: Option<&Hotkey>) -> String {
        let Some(h) = hotkey else {
            return "UNKNOWN None".into();
        };
        let m = h.modifiers;
        let names: Vec<&str> = [(m.shift, "Shift"), (m.alt, "Alt"), (m.ctrl, "Ctrl"), (m.meta, "Meta")]
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .collect();
        let mods = if names.is_empty() { "None".to_string() } else { names.join(", ") };
        format!("{} {mods}", h.key)
    }

    /// Port of `Hotkey.DisplayString`. Key names come from `keycode.<name>` messages as `KeycodeFluentKeys` lists
    /// them, falling back to the enum name.
    pub fn display_string(&self, messages: &crate::fluent::Messages) -> String {
        let name = |key: &str, fallback: &str| messages.try_get(key, &[]).unwrap_or_else(|| fallback.to_string());
        let mut ret = name(&format!("keycode.{}", self.key.to_ascii_lowercase()), &self.key);
        let m = self.modifiers;
        let meta_key = if cfg!(target_os = "macos") {
            "keycode-modifier.cmd"
        } else {
            "keycode-modifier.meta"
        };
        for (on, key, fallback) in [
            (m.shift, "keycode-modifier.shift", "Shift"),
            (m.alt, "keycode-modifier.alt", "Alt"),
            (m.ctrl, "keycode-modifier.ctrl", "Ctrl"),
            (m.meta, meta_key, "Meta"),
        ] {
            if on {
                ret = format!("{} + {ret}", name(key, fallback));
            }
        }
        ret
    }

    /// Port of `HotkeyReference.IsActivatedBy`.
    pub fn is_activated_by(&self, e: &KeyInput) -> bool {
        self.key == e.key && self.modifiers == e.modifiers
    }
}

/// Port of `HotkeyDefinition`.
#[derive(Clone, Debug)]
pub struct HotkeyDefinition {
    pub name: String,
    pub default: Option<Hotkey>,
    pub description: String,
    pub types: Vec<String>,
    /// The contexts with the `hotkey-context.` prefix.
    pub contexts: Vec<String>,
    pub readonly: bool,
    pub has_duplicates: bool,
}

/// `HotkeyDefinition.ContextFluentPrefix`.
pub const CONTEXT_FLUENT_PREFIX: &str = "hotkey-context";

fn split_set(value: &str) -> Vec<String> {
    let mut set: Vec<String> = Vec::new();
    for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if !set.iter().any(|s| s == part) {
            set.push(part.to_string());
        }
    }
    set
}

impl HotkeyDefinition {
    fn new(node: &MiniYamlNode) -> HotkeyDefinition {
        let field = |key: &str| node.value.nodes.iter().find(|n| n.key == key);
        let mut default = Hotkey::parse(node.value.value_str());
        let platform = if cfg!(target_os = "macos") {
            "OSX"
        } else if cfg!(windows) {
            "Windows"
        } else {
            "Linux"
        };
        if let Some(over) = field("Platform").and_then(|p| p.value.nodes.iter().find(|n| n.key == platform)) {
            default = Hotkey::parse(over.value.value_str());
        }
        HotkeyDefinition {
            name: node.key.clone(),
            default,
            description: field("Description").map(|n| n.value.value_str().to_string()).unwrap_or_default(),
            types: field("Types").map(|n| split_set(n.value.value_str())).unwrap_or_default(),
            contexts: field("Contexts")
                .map(|n| {
                    split_set(n.value.value_str())
                        .into_iter()
                        .map(|c| format!("{CONTEXT_FLUENT_PREFIX}.{c}"))
                        .collect()
                })
                .unwrap_or_default(),
            readonly: field("Readonly").is_some_and(|n| n.value.value_str().eq_ignore_ascii_case("true")),
            has_duplicates: false,
        }
    }
}

/// Port of `HotkeyManager`. An invalid hotkey is `None`.
#[derive(Default)]
pub struct Hotkeys {
    definitions: IndexMap<String, HotkeyDefinition>,
    keys: IndexMap<String, Option<Hotkey>>,
}

impl Hotkeys {
    /// Reads the definitions of the `Hotkeys` files of the manifest.
    pub fn load(nodes: &[MiniYamlNode]) -> Hotkeys {
        let mut hotkeys = Hotkeys::default();
        for node in nodes {
            let definition = HotkeyDefinition::new(node);
            hotkeys.keys.insert(node.key.clone(), definition.default.clone());
            hotkeys.definitions.insert(node.key.clone(), definition);
        }
        hotkeys.update_duplicates();
        hotkeys
    }

    /// Applies the `Keys` section of the settings.
    pub fn apply_settings(&mut self, settings: &IndexMap<String, String>) {
        for (name, value) in settings {
            if self.definitions.get(name).is_some_and(|d| !d.readonly) {
                self.keys.insert(name.clone(), Hotkey::parse(value));
            }
        }
        self.update_duplicates();
    }

    fn update_duplicates(&mut self) {
        let names: Vec<String> = self.definitions.keys().cloned().collect();
        for name in names {
            let value = self.keys.get(&name).cloned().flatten();
            let duplicate = self.first_duplicate(&name, value.as_ref()).is_some();
            if let Some(d) = self.definitions.get_mut(&name) {
                d.has_duplicates = duplicate;
            }
        }
    }

    /// The definitions in file order.
    pub fn definitions(&self) -> impl Iterator<Item = &HotkeyDefinition> {
        self.definitions.values()
    }

    pub fn definition(&self, name: &str) -> Option<&HotkeyDefinition> {
        self.definitions.get(name)
    }

    /// Port of `Set`: changes a hotkey that is not read only and writes it to the `Keys` section of the settings.
    pub fn set(&mut self, name: &str, value: Option<Hotkey>, settings: &mut IndexMap<String, String>) {
        let Some(definition) = self.definitions.get(name) else {
            return;
        };
        if definition.readonly {
            return;
        }
        if hotkeys_equal(value.as_ref(), definition.default.as_ref()) {
            settings.shift_remove(name);
        } else {
            settings.insert(name.to_string(), Hotkey::setting_string(value.as_ref()));
        }
        self.keys.insert(name.to_string(), value);
        self.update_duplicates();
    }

    /// Port of `GetFirstDuplicate`: the name of another hotkey with the same value in a shared context.
    pub fn first_duplicate(&self, name: &str, value: Option<&Hotkey>) -> Option<&HotkeyDefinition> {
        let definition = self.definitions.get(name)?;
        self.keys.iter().find_map(|(other, key)| {
            let other_definition = &self.definitions[other];
            (other != name
                && hotkeys_equal(key.as_ref(), value)
                && other_definition.contexts.iter().any(|c| definition.contexts.contains(c)))
            .then_some(other_definition)
        })
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

/// `Hotkey.operator ==`, under which an invalid hotkey equals nothing.
pub fn hotkeys_equal(a: Option<&Hotkey>, b: Option<&Hotkey>) -> bool {
    a.is_some() && a == b
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

    #[test]
    fn set_tracks_duplicates_and_settings() {
        let yaml = "A: Q\n\tContexts: player\nB: W\n\tContexts: player\nC: Q\n\tContexts: editor\nD:\n\tContexts: player\nE:\n\tContexts: player\n";
        let nodes = rustora_core::miniyaml::MiniYaml::from_str(yaml, "t", true).unwrap();
        let mut hotkeys = Hotkeys::load(&nodes);
        assert!(!hotkeys.definition("A").unwrap().has_duplicates);
        assert!(!hotkeys.definition("D").unwrap().has_duplicates);
        let mut settings = IndexMap::new();
        hotkeys.set("B", Hotkey::parse("Q"), &mut settings);
        assert!(hotkeys.definition("A").unwrap().has_duplicates);
        assert!(hotkeys.definition("B").unwrap().has_duplicates);
        assert!(!hotkeys.definition("C").unwrap().has_duplicates);
        assert_eq!(settings.get("B").map(String::as_str), Some("Q None"));
        hotkeys.set("B", None, &mut settings);
        assert!(!hotkeys.definition("A").unwrap().has_duplicates);
        assert_eq!(settings.get("B").map(String::as_str), Some("UNKNOWN None"));
        hotkeys.set("B", Hotkey::parse("W"), &mut settings);
        assert!(settings.is_empty());
    }
}
