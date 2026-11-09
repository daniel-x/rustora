// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The mod as the server holds it: the mod data, the lobby rules of `rustora_game::lobby_rules` and the map
//! cache of the server.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use indexmap::IndexMap;
use rustora_fs::{Package, Platform};
use rustora_game::map::MapPackage;
use rustora_game::moddata::ModData;
pub use rustora_game::lobby_rules::{GameSpeed, MAP_STATUS_UNSAFE_CUSTOM_RULES, MapInfo};
use rustora_game::lobby_rules::ModLobbyRules;

struct MapEntry {
    package: Arc<dyn Package>,
    map: MapPackage,
}

/// The loaded mod, like `ModData` with its `MapCache`.
pub struct ServerMod {
    pub data: ModData,
    pub rules: ModLobbyRules,
    maps: IndexMap<String, MapEntry>,
    infos: HashMap<String, Rc<MapInfo>>,
}

/// The maps of the `MapFolders` of the manifest, as `MapCache.LoadMaps` finds them.
fn scan_maps(data: &ModData) -> IndexMap<String, MapEntry> {
    let mut maps = IndexMap::new();
    let fs = &data.file_system;
    for (name, _) in &data.manifest.map_folders {
        let Some(folder) = fs.open_package(name.strip_prefix('~').unwrap_or(name)) else {
            continue;
        };
        let mut entries = folder.contents();
        entries.sort();
        for entry in entries {
            if let Some(package) = folder.open_package(&entry, fs)
                && let Ok(map) = MapPackage::load(package.clone())
            {
                maps.entry(map.uid.clone()).or_insert(MapEntry { package, map });
            }
        }
    }
    maps
}

impl ServerMod {
    /// Loads the mod `mod_id` and the map list of its `maps` folder.
    pub fn load(engine: &Path, support: &str, mod_id: &str) -> Result<ServerMod, String> {
        let data = ModData::load(Platform::new(engine, support), mod_id).map_err(|e| e.to_string())?;
        let rules = ModLobbyRules::load(&data)?;
        let maps = scan_maps(&data);
        Ok(ServerMod {
            data,
            rules,
            maps,
            infos: HashMap::new(),
        })
    }

    /// The text of a mod Fluent message without arguments, or the key if unknown.
    pub fn message(&self, key: &str) -> String {
        self.rules.message(key)
    }

    /// The uids of the maps offered in the lobby, the map pool of a server without a map repository.
    pub fn lobby_maps(&self) -> Vec<String> {
        self.maps
            .iter()
            .filter(|(_, e)| {
                e.map
                    .section("Visibility")
                    .is_none_or(|v| v.value_str().split(',').any(|s| s.trim() == "Lobby"))
            })
            .map(|(uid, _)| uid.clone())
            .collect()
    }

    /// The uid of a map given by uid or file name.
    pub fn resolve_map(&self, name: &str) -> Option<String> {
        if self.maps.contains_key(name) {
            return Some(name.to_string());
        }
        self.maps
            .iter()
            .find(|(_, e)| Path::new(e.package.name()).file_name().is_some_and(|f| f == name))
            .map(|(uid, _)| uid.clone())
    }

    /// Adds the maps that were added to the map folders since the last scan.
    pub fn rescan_maps(&mut self) {
        for (uid, entry) in scan_maps(&self.data) {
            self.maps.entry(uid).or_insert(entry);
        }
    }

    pub fn map_exists(&self, uid: &str) -> bool {
        self.maps.contains_key(uid)
    }

    /// The rules of the map `uid`, loaded on first use.
    pub fn map_info(&mut self, uid: &str) -> Result<Rc<MapInfo>, String> {
        if let Some(i) = self.infos.get(uid) {
            return Ok(i.clone());
        }
        if !self.maps.contains_key(uid) {
            self.rescan_maps();
        }
        let entry = self.maps.get(uid).ok_or_else(|| format!("unknown map {uid}"))?;
        let info = Rc::new(self.rules.map_info(&self.data, &entry.package, &entry.map)?);
        self.infos.insert(uid.to_string(), info.clone());
        Ok(info)
    }
}
