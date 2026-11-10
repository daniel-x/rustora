// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The mod as the server holds it: the mod data, the lobby rules of `rustora_game::lobby_rules` and the map
//! cache of the server. The map cache scans the map folders only when it needs a map it does not know yet, so a
//! dependent server that is told the package of its initial map starts without a scan.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use indexmap::IndexMap;
use rustora_fs::{Package, Platform};
use rustora_game::lobby_rules::ModLobbyRules;
pub use rustora_game::lobby_rules::{GameSpeed, MAP_STATUS_UNSAFE_CUSTOM_RULES, MapInfo};
use rustora_game::map::MapPackage;
use rustora_game::map_folders::map_folders;
use rustora_game::moddata::ModData;

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
    scanned: bool,
}

/// The maps of the map folders of the mod, each folder in name order.
fn scan_maps(data: &ModData) -> IndexMap<String, MapEntry> {
    let mut maps = IndexMap::new();
    for folder in map_folders(data) {
        let mut packages = folder.map_packages(&data.file_system);
        packages.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, package) in packages {
            if let Ok(map) = MapPackage::load(package.clone()) {
                maps.entry(map.uid.clone()).or_insert(MapEntry { package, map });
            }
        }
    }
    maps
}

/// The map at the package path `path` inside one of the map folders of the mod.
fn load_map_package(data: &ModData, path: &str) -> Option<MapEntry> {
    let path = std::fs::canonicalize(path).ok()?;
    map_folders(data).into_iter().find_map(|folder| {
        let folder_path = std::fs::canonicalize(folder.package.name()).ok()?;
        let entry = path.strip_prefix(folder_path).ok()?.to_str()?;
        let package = folder.package.open_package(entry, &data.file_system)?;
        let map = MapPackage::load(package.clone()).ok()?;
        Some(MapEntry { package, map })
    })
}

impl ServerMod {
    /// Loads the mod `mod_id`. `map_packages` are the package paths of maps that are known before the map folders
    /// are scanned.
    pub fn load(engine: &Path, support: &str, mod_id: &str, map_packages: &[String]) -> Result<ServerMod, String> {
        let data = ModData::load(Platform::new(engine, support), mod_id).map_err(|e| e.to_string())?;
        let rules = ModLobbyRules::load(&data)?;
        let maps = map_packages
            .iter()
            .filter_map(|path| load_map_package(&data, path))
            .map(|entry| (entry.map.uid.clone(), entry))
            .collect();
        Ok(ServerMod {
            data,
            rules,
            maps,
            infos: HashMap::new(),
            scanned: false,
        })
    }

    /// The text of a mod Fluent message without arguments, or the key if unknown.
    pub fn message(&self, key: &str) -> String {
        self.rules.message(key)
    }

    fn ensure_scanned(&mut self) {
        if !self.scanned {
            self.rescan_maps();
        }
    }

    /// The uids of the maps offered in the lobby, the map pool of a server without a map repository.
    pub fn lobby_maps(&mut self) -> Vec<String> {
        self.ensure_scanned();
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
    pub fn resolve_map(&mut self, name: &str) -> Option<String> {
        if self.maps.contains_key(name) {
            return Some(name.to_string());
        }
        self.ensure_scanned();
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
        self.scanned = true;
    }

    /// Whether the map `uid` is known, after a rescan if it is not.
    pub fn map_exists(&mut self, uid: &str) -> bool {
        if !self.maps.contains_key(uid) {
            self.rescan_maps();
        }
        self.maps.contains_key(uid)
    }

    /// The rules of the map `uid`, loaded on first use.
    pub fn map_info(&mut self, uid: &str) -> Result<Rc<MapInfo>, String> {
        if let Some(i) = self.infos.get(uid) {
            return Ok(i.clone());
        }
        if !self.map_exists(uid) {
            return Err(format!("unknown map {uid}"));
        }
        let entry = &self.maps[uid];
        let info = Rc::new(self.rules.map_info(&self.data, &entry.package, &entry.map)?);
        self.infos.insert(uid.to_string(), info.clone());
        Ok(info)
    }
}
