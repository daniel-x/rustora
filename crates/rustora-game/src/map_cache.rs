// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MapCache.cs` and the local part of `MapPreview.cs`.
//!
//! Not ported: remote map queries and downloads, minimap generation for maps without `map.png`, and the directory
//! watcher.

use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use indexmap::IndexMap;
use rustora_core::fieldloader::{ArrayAware, Rectangle, get_value};
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_core::world_types::CPos;
use rustora_fs::{FileSystem, Package};

use crate::GameError;
use crate::map::compute_uid;
use crate::moddata::ModData;

/// The oldest map format that `Map.SupportedMapFormat` accepts.
pub const SUPPORTED_MAP_FORMAT: i32 = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapClassification {
    Unknown,
    System,
    User,
    Remote,
}

impl MapClassification {
    fn parse(value: &str) -> MapClassification {
        match value {
            "System" => MapClassification::System,
            "User" => MapClassification::User,
            "Remote" => MapClassification::Remote,
            _ => MapClassification::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapStatus {
    Available,
    Unavailable,
    Searching,
}

/// `MapVisibility` as flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapVisibility(pub u32);

impl MapVisibility {
    pub const LOBBY: MapVisibility = MapVisibility(1);
    pub const SHELLMAP: MapVisibility = MapVisibility(2);
    pub const MISSION_SELECTOR: MapVisibility = MapVisibility(4);

    fn parse(value: &str) -> MapVisibility {
        let bits = value.split(',').fold(0, |bits, part| {
            bits | match part.trim() {
                "Lobby" => 1,
                "Shellmap" => 2,
                "MissionSelector" => 4,
                _ => 0,
            }
        });
        MapVisibility(bits)
    }

    pub fn has(self, flag: MapVisibility) -> bool {
        self.0 & flag.0 == flag.0
    }

    pub fn intersects(self, other: MapVisibility) -> bool {
        self.0 & other.0 != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapGridType {
    Rectangular,
    RectangularIsometric,
}

/// The part of `PlayerReference` that the map chooser needs.
#[derive(Clone, Debug)]
pub struct MapPlayer {
    pub name: String,
    pub playable: bool,
    pub allow_bots: bool,
}

/// Port of `MapPreview` for a map on disk.
pub struct MapPreview {
    pub uid: String,
    pub map_format: i32,
    pub title: String,
    pub categories: Vec<String>,
    pub author: String,
    pub tile_set: String,
    pub players: Vec<MapPlayer>,
    pub player_count: i32,
    pub spawn_points: Vec<CPos>,
    pub grid_type: MapGridType,
    pub bounds: Rectangle,
    pub minimap_png: Option<Vec<u8>>,
    pub status: MapStatus,
    pub class: MapClassification,
    pub visibility: MapVisibility,
    pub modified_date: SystemTime,
    pub package_name: String,
    parent: Arc<dyn Package>,
    /// The name of the map package inside the parent package.
    entry: String,
}

impl MapPreview {
    /// Port of `MapPreview.Package`.
    pub fn open_package(&self, fs: &FileSystem) -> Option<Arc<dyn Package>> {
        self.parent.open_package(&self.entry, fs)
    }

    /// Port of `IsSuitableInitialMap`.
    fn is_suitable_initial_map(&self) -> bool {
        self.status == MapStatus::Available
            && self.visibility.has(MapVisibility::LOBBY)
            && self.categories.iter().any(|c| c == "Conquest")
            && self.players.iter().all(|p| p.allow_bots)
            && self.bounds.width <= 128
            && self.bounds.height <= 128
    }

    /// Port of `MapPreview.Delete`: only the `User` folder holds deletable maps.
    pub fn delete(&self) -> std::io::Result<()> {
        let path = std::path::Path::new(&self.package_name);
        if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
    }
}

fn field<T: rustora_core::fieldloader::FieldValue + ArrayAware>(name: &str, value: &str) -> Result<T, GameError> {
    get_value(name, Some(value)).map_err(|e| GameError::Data(e.to_string()))
}

fn map_players(nodes: &[MiniYamlNode]) -> Vec<MapPlayer> {
    nodes
        .iter()
        .map(|node| {
            let value = |key: &str| node.value.nodes.iter().find(|n| n.key == key).map(|n| n.value.value_str());
            let flag = |key: &str, default: bool| value(key).map_or(default, |v| v.trim().eq_ignore_ascii_case("true"));
            MapPlayer {
                name: value("Name").unwrap_or_default().to_string(),
                playable: flag("Playable", false),
                allow_bots: flag("AllowBots", true),
            }
        })
        .collect()
}

fn spawn_points(actors: &[MiniYamlNode]) -> Result<Vec<CPos>, GameError> {
    actors
        .iter()
        .filter(|node| node.value.value_str() == "mpspawn")
        .map(|node| {
            let location = node
                .value
                .nodes
                .iter()
                .find(|n| n.key == "Location")
                .ok_or_else(|| GameError::Data("spawn without a location".into()))?;
            field::<CPos>("Location", location.value.value_str())
        })
        .collect()
}

fn preview_from_package(
    package: &Arc<dyn Package>,
    parent: &Arc<dyn Package>,
    entry: &str,
    class: MapClassification,
    compatibility: &[String],
    grid_type: MapGridType,
) -> Result<MapPreview, GameError> {
    let name = package.name().to_string();
    let data = package
        .get("map.yaml")
        .ok_or_else(|| GameError::Data("Required file map.yaml not present in this map".into()))?;
    let yaml = MiniYaml::new(None, MiniYaml::from_bytes(&data, &format!("{name}:map.yaml"), true)?);
    let get = |key: &str| yaml.nodes.iter().find(|n| n.key == key).map(|n| &n.value);
    let text = |key: &str| get(key).map(|v| v.value_str().to_string());

    let map_format: i32 = get("MapFormat").map_or(Ok(0), |v| field("MapFormat", v.value_str()))?;
    if map_format < SUPPORTED_MAP_FORMAT {
        return Err(GameError::Data(format!("Map format {map_format} is not supported.")));
    }
    let uid = compute_uid(package.as_ref(), map_format)?;
    let requires_mod = text("RequiresMod").unwrap_or_default();
    let mut status = if compatibility.is_empty() || compatibility.contains(&requires_mod) {
        MapStatus::Available
    } else {
        MapStatus::Unavailable
    };
    let spawns = spawn_points(get("Actors").map_or(&[], |v| v.nodes.as_slice())).unwrap_or_else(|_| {
        status = MapStatus::Unavailable;
        Vec::new()
    });
    let players = get("Players").map_or_else(Vec::new, |v| map_players(&v.nodes));
    let modified_date = std::fs::metadata(&name)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(MapPreview {
        uid,
        map_format,
        title: text("Title").unwrap_or_default(),
        categories: text("Categories").map_or_else(Vec::new, |v| {
            v.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
        }),
        author: text("Author").unwrap_or_default(),
        tile_set: text("Tileset").unwrap_or_default(),
        player_count: players.iter().filter(|p| p.playable).count() as i32,
        players,
        spawn_points: spawns,
        grid_type,
        bounds: get("Bounds").map_or(Ok(Rectangle::default()), |v| field("Bounds", v.value_str()))?,
        minimap_png: package.get("map.png"),
        status,
        class,
        visibility: text("Visibility").map_or(MapVisibility::LOBBY, |v| MapVisibility::parse(&v)),
        modified_date,
        package_name: name,
        parent: parent.clone(),
        entry: entry.to_string(),
    })
}

/// Port of `MapCache`.
#[derive(Default)]
pub struct MapCache {
    pub previews: IndexMap<String, Rc<MapPreview>>,
}

impl MapCache {
    /// Port of `LoadMaps`. A map that fails to load is skipped with a log line.
    pub fn load(mod_data: &ModData) -> MapCache {
        let mut cache = MapCache::default();
        let manifest = &mod_data.manifest;
        let compatibility = &manifest.map_compatibility;
        let grid_type = match manifest.section("MapGrid").and_then(|g| g.nodes.iter().find(|n| n.key == "Type")) {
            Some(node) if node.value.value_str() == "RectangularIsometric" => MapGridType::RectangularIsometric,
            _ => MapGridType::Rectangular,
        };
        let fs = &mod_data.file_system;
        for (name, class) in &manifest.map_folders {
            let class = MapClassification::parse(class);
            let name = name.strip_prefix('~').unwrap_or(name);
            let resolved = fs.platform().resolve_path(name);
            if resolved.starts_with(&*fs.platform().support_dir.to_string_lossy())
                && !std::path::Path::new(&resolved).is_file()
                && let Err(e) = std::fs::create_dir_all(&resolved)
            {
                log::warn!("Cannot create the map folder {resolved}: {e}");
            }
            let Some(folder) = fs.open_package(name) else {
                continue;
            };
            for entry in folder.contents() {
                let Some(package) = folder.open_package(&entry, fs) else {
                    continue;
                };
                match preview_from_package(&package, &folder, &entry, class, compatibility, grid_type) {
                    Ok(preview) => {
                        cache.previews.insert(preview.uid.clone(), Rc::new(preview));
                    }
                    Err(e) => log::warn!("Failed to load map: {entry}: {e}"),
                }
            }
        }
        cache
    }

    pub fn get(&self, uid: &str) -> Option<Rc<MapPreview>> {
        self.previews.get(uid).cloned()
    }

    /// Port of `MapPreview.Delete` and the cache update after it.
    pub fn delete(&mut self, uid: &str) -> std::io::Result<()> {
        if let Some(preview) = self.previews.get(uid) {
            preview.delete()?;
            self.previews.shift_remove(uid);
        }
        Ok(())
    }

    /// Port of `ChooseInitialMap`. `random` maps a count to an index below it.
    pub fn choose_initial_map(&self, initial: Option<&str>, random: &mut dyn FnMut(usize) -> usize) -> String {
        let usable = |m: &MapPreview| {
            m.status == MapStatus::Available
                && m.visibility.has(MapVisibility::LOBBY)
                && matches!(m.class, MapClassification::System | MapClassification::User)
        };
        if let Some(uid) = initial.filter(|uid| !uid.is_empty())
            && self.previews.get(uid).is_some_and(|m| usable(m))
        {
            return uid.to_string();
        }
        let suitable: Vec<&Rc<MapPreview>> = self.previews.values().filter(|m| m.is_suitable_initial_map()).collect();
        if !suitable.is_empty() {
            return suitable[random(suitable.len())].uid.clone();
        }
        self.previews
            .values()
            .find(|m| usable(m))
            .map_or_else(String::new, |m| m.uid.clone())
    }
}
