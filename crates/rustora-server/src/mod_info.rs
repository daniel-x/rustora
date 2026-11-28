// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The mod as the server holds it: the mod data, the lobby rules of `rustora_game::lobby_rules` and the map
//! cache of the server. The map cache scans the map folders only when it needs a map it does not know yet, so a
//! dependent server that is told the package of its initial map starts without a scan.

use rustora_core::hash::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use rustora_core::hash::IndexMap;
use rustora_fs::{Package, Platform};
use rustora_game::lobby_rules::ModLobbyRules;
pub use rustora_game::lobby_rules::{GameSpeed, MAP_STATUS_UNSAFE_CUSTOM_RULES, MapInfo};
use rustora_game::map::MapPackage;
use rustora_game::map_folders::map_folders;
use rustora_game::moddata::ModData;

/// A map of the map cache without its package, which is opened again when the map's rules are needed, so that the
/// packages of all maps, mostly zip files held in memory, do not stay loaded.
struct MapEntry {
    /// The index of the map folder in `map_folders`.
    folder: usize,
    /// The entry of the package in the folder.
    entry: String,
    /// The file name of the package.
    file_name: String,
    /// `Visibility` of map.yaml.
    visibility: Option<String>,
}

impl MapEntry {
    fn new(folder: usize, entry: String, package: &dyn Package, map: &MapPackage) -> MapEntry {
        MapEntry {
            folder,
            entry,
            file_name: Path::new(package.name())
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
            visibility: map.section("Visibility").map(|v| v.value_str().to_string()),
        }
    }

    /// The package and the map, read again.
    fn open(&self, data: &ModData) -> Option<(Arc<dyn Package>, MapPackage)> {
        let folder = map_folders(data).into_iter().nth(self.folder)?;
        let package = folder.package.open_package(&self.entry, &data.file_system)?;
        let map = MapPackage::load(package.clone()).ok()?;
        Some((package, map))
    }
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
    let mut maps = IndexMap::default();
    for (index, folder) in map_folders(data).into_iter().enumerate() {
        let mut entries = folder.package.contents();
        entries.sort();
        for entry in entries {
            let Some(package) = folder.package.open_package(&entry, &data.file_system) else {
                continue;
            };
            if let Ok(map) = MapPackage::load(package.clone()) {
                let uid = map.uid.clone();
                maps.entry(uid)
                    .or_insert_with(|| MapEntry::new(index, entry, package.as_ref(), &map));
            }
        }
    }
    maps
}

/// The uid and the map at the package path `path` inside one of the map folders of the mod.
fn load_map_package(data: &ModData, path: &str) -> Option<(String, MapEntry)> {
    let path = std::fs::canonicalize(path).ok()?;
    map_folders(data).into_iter().enumerate().find_map(|(index, folder)| {
        let folder_path = std::fs::canonicalize(folder.package.name()).ok()?;
        let entry = path.strip_prefix(folder_path).ok()?.to_str()?;
        let package = folder.package.open_package(entry, &data.file_system)?;
        let map = MapPackage::load(package.clone()).ok()?;
        Some((map.uid.clone(), MapEntry::new(index, entry.to_string(), package.as_ref(), &map)))
    })
}

impl ServerMod {
    /// Loads the mod `mod_id`. `map_packages` are the package paths of maps that are known before the map folders
    /// are scanned.
    pub fn load(engine: &Path, support: &str, mod_id: &str, map_packages: &[String]) -> Result<ServerMod, String> {
        let data = ModData::load(Platform::new(engine, support), mod_id).map_err(|e| e.to_string())?;
        let rules = ModLobbyRules::load(&data)?;
        let maps = map_packages.iter().filter_map(|path| load_map_package(&data, path)).collect();
        Ok(ServerMod {
            data,
            rules,
            maps,
            infos: HashMap::default(),
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
            .filter(|(_, e)| e.visibility.as_deref().is_none_or(|v| v.split(',').any(|s| s.trim() == "Lobby")))
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
        self.maps.iter().find(|(_, e)| e.file_name == name).map(|(uid, _)| uid.clone())
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
        let (package, map) = self.maps[uid]
            .open(&self.data)
            .ok_or_else(|| format!("cannot read the package of map {uid}"))?;
        let info = Rc::new(self.rules.map_info(&self.data, &package, &map)?);
        self.infos.insert(uid.to_string(), info.clone());
        Ok(info)
    }
}
