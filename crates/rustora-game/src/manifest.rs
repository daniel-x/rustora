// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `OpenRA.Game/Manifest.cs`: the parsed `mod.yaml` of a mod.

use std::sync::Arc;

use indexmap::IndexMap;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_fs::Package;

use crate::GameError;

#[derive(Clone, Debug, Default)]
pub struct ModMetadata {
    pub title: String,
    pub version: String,
    pub website: String,
    pub web_icon32: String,
    pub window_title: Option<String>,
    pub hidden: bool,
}

/// Describes what is to be loaded in order to run a mod.
pub struct Manifest {
    pub id: String,
    pub package: Arc<dyn Package>,
    pub metadata: ModMetadata,
    pub rules: Vec<String>,
    pub server_traits: Vec<String>,
    pub sequences: Vec<String>,
    pub model_sequences: Vec<String>,
    pub cursors: Vec<String>,
    pub chrome: Vec<String>,
    pub chrome_layout: Vec<String>,
    pub weapons: Vec<String>,
    pub voices: Vec<String>,
    pub notifications: Vec<String>,
    pub music: Vec<String>,
    pub fluent_messages: Vec<String>,
    pub tile_sets: Vec<String>,
    pub chrome_metrics: Vec<String>,
    pub missions: Vec<String>,
    pub hotkeys: Vec<String>,
    pub map_compatibility: Vec<String>,
    pub map_folders: IndexMap<String, String>,
    pub file_system: MiniYaml,
    pub load_screen: MiniYaml,
    pub default_order_generator: Option<String>,
    pub assemblies: Vec<String>,
    pub sound_formats: Vec<String>,
    pub sprite_formats: Vec<String>,
    pub package_formats: Vec<String>,
    pub video_formats: Vec<String>,
    pub font_sheet_size: i32,
    pub cursor_sheet_size: i32,
    /// All top level sections, after `Include` expansion and merging.
    pub yaml: IndexMap<String, MiniYaml>,
}

/// `FieldLoader` semantics for `string[]`: split at commas, drop empty entries, trim.
pub fn split_list(value: Option<&str>) -> Vec<String> {
    value.map_or_else(Vec::new, |v| {
        v.split(',').filter(|s| !s.is_empty()).map(|s| s.trim().to_string()).collect()
    })
}

impl Manifest {
    pub fn load(mod_id: &str, package: Arc<dyn Package>) -> Result<Self, GameError> {
        let name = package.name().to_string();
        let data = package
            .get("mod.yaml")
            .ok_or_else(|| GameError::Data(format!("{name}: mod.yaml not found")))?;
        let mut nodes = MiniYaml::from_bytes(&data, &format!("{name}:mod.yaml"), true)?;
        let mut i = nodes.len();
        while i > 0 {
            i -= 1;
            if nodes[i].key != "Include" {
                continue;
            }
            let file = nodes[i].value.value_str().to_string();
            let contents = package
                .get(&file)
                .ok_or_else(|| GameError::Data(format!("{}: File `{file}` not found.", nodes[i].location)))?;
            let included = MiniYaml::from_bytes(&contents, &format!("{name}:{file}"), true)?;
            nodes.splice(i..=i, included);
        }

        let merged = MiniYaml::merge(vec![nodes])?;
        let yaml: IndexMap<String, MiniYaml> = merged.into_iter().map(|n| (n.key, n.value)).collect();

        let list = |key: &str| -> Vec<String> {
            yaml.get(key)
                .map_or_else(Vec::new, |y| y.nodes.iter().map(|n| n.key.clone()).collect())
        };
        let value = |key: &str| -> Option<String> { yaml.get(key).and_then(|y| y.value.clone()) };
        let value_list = |key: &str| split_list(yaml.get(key).and_then(|y| y.value.as_deref()));

        let meta = yaml
            .get("Metadata")
            .ok_or_else(|| GameError::Data("`Metadata` section is not defined.".into()))?;
        let field = |k: &str| meta.nodes.iter().find(|n| n.key == k).and_then(|n| n.value.value.clone());
        let metadata = ModMetadata {
            title: field("Title").unwrap_or_default(),
            version: field("Version").unwrap_or_default(),
            website: field("Website").unwrap_or_default(),
            web_icon32: field("WebIcon32").unwrap_or_default(),
            window_title: field("WindowTitle"),
            hidden: field("Hidden").is_some_and(|v| v.eq_ignore_ascii_case("true")),
        };

        let map_folders = yaml
            .get("MapFolders")
            .map(|y| y.nodes.iter().map(|n| (n.key.clone(), n.value.value_str().to_string())).collect())
            .unwrap_or_default();

        let file_system = yaml
            .get("FileSystem")
            .cloned()
            .ok_or_else(|| GameError::Data("`FileSystem` section is not defined.".into()))?;
        let load_screen = yaml
            .get("LoadScreen")
            .cloned()
            .ok_or_else(|| GameError::Data("`LoadScreen` section is not defined.".into()))?;

        let mut map_compatibility = vec![mod_id.to_string()];
        if let Some(v) = value("SupportsMapsFrom") {
            map_compatibility.extend(v.split(',').map(|c| c.trim().to_string()));
        }

        let int = |key: &str, default: i32| value(key).and_then(|v| v.parse().ok()).unwrap_or(default);

        Ok(Manifest {
            id: mod_id.to_string(),
            metadata,
            rules: list("Rules"),
            server_traits: list("ServerTraits"),
            sequences: list("Sequences"),
            model_sequences: list("ModelSequences"),
            cursors: list("Cursors"),
            chrome: list("Chrome"),
            chrome_layout: list("ChromeLayout"),
            weapons: list("Weapons"),
            voices: list("Voices"),
            notifications: list("Notifications"),
            music: list("Music"),
            fluent_messages: list("FluentMessages"),
            tile_sets: list("TileSets"),
            chrome_metrics: list("ChromeMetrics"),
            missions: list("Missions"),
            hotkeys: list("Hotkeys"),
            map_compatibility,
            map_folders,
            file_system,
            load_screen,
            default_order_generator: value("DefaultOrderGenerator"),
            assemblies: value_list("Assemblies"),
            sound_formats: value_list("SoundFormats"),
            sprite_formats: value_list("SpriteFormats"),
            package_formats: value_list("PackageFormats"),
            video_formats: value_list("VideoFormats"),
            font_sheet_size: int("FontSheetSize", 512),
            cursor_sheet_size: int("CursorSheetSize", 512),
            package,
            yaml,
        })
    }

    /// A custom (non reserved) top level section, like `Manifest.Get<T>()` reads one.
    pub fn section(&self, key: &str) -> Option<&MiniYaml> {
        self.yaml.get(key)
    }
}

/// Converts a list of nodes to `(key, value)` pairs, like `Dictionary<string, string>` from FieldLoader.
pub fn node_pairs(nodes: &[MiniYamlNode]) -> Vec<(String, Option<String>)> {
    nodes.iter().map(|n| (n.key.clone(), n.value.value.clone())).collect()
}
