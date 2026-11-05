// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MapCache.cs` and `MapPreview.cs`. The HTTP requests of the remote map queries and downloads are made by
//! the client; this module applies their results.
//!
//! Not ported: the custom rules of remote maps, minimap generation for maps without `map.png`, and the directory
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
    DownloadAvailable,
    Downloading,
    DownloadError,
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

/// Port of `MapPreview`. A map that is not on disk has no parent package.
#[derive(Clone)]
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
    /// The size of a running download in bytes, zero before the server sent it.
    pub download_bytes: i64,
    pub download_percentage: i32,
    parent: Option<Arc<dyn Package>>,
    /// The name of the map package inside the parent package.
    entry: String,
}

impl MapPreview {
    /// The `MapPreview` constructor for a uid that is not known locally.
    pub fn unknown(uid: &str, grid_type: MapGridType) -> MapPreview {
        MapPreview {
            uid: uid.to_string(),
            map_format: 0,
            title: "Unknown Map".into(),
            categories: vec!["Unknown".into()],
            author: "Unknown Author".into(),
            tile_set: "unknown".into(),
            players: Vec::new(),
            player_count: 0,
            spawn_points: Vec::new(),
            grid_type,
            bounds: Rectangle::default(),
            minimap_png: None,
            status: MapStatus::Unavailable,
            class: MapClassification::Unknown,
            visibility: MapVisibility::LOBBY,
            modified_date: SystemTime::UNIX_EPOCH,
            package_name: String::new(),
            download_bytes: 0,
            download_percentage: 0,
            parent: None,
            entry: String::new(),
        }
    }

    /// Port of `MapPreview.Package`.
    pub fn open_package(&self, fs: &FileSystem) -> Option<Arc<dyn Package>> {
        self.parent.as_ref()?.open_package(&self.entry, fs)
    }

    /// Port of `UpdateRemoteSearch` for the `DownloadAvailable` status, with `yaml` as the `RemoteMapData` of the map.
    fn apply_remote_data(&mut self, yaml: &MiniYaml, compatibility: &[String]) -> Result<(), GameError> {
        use base64::Engine;
        let get = |key: &str| yaml.nodes.iter().find(|n| n.key == key).map(|n| n.value.value_str());
        let base64 = |key: &str| -> Result<Vec<u8>, GameError> {
            base64::engine::general_purpose::STANDARD
                .decode(get(key).unwrap_or_default().trim())
                .map_err(|e| GameError::Data(format!("{key}: {e}")))
        };
        if !get("downloading").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) {
            self.status = MapStatus::Unavailable;
            return Ok(());
        }
        self.title = get("title").unwrap_or_default().to_string();
        self.categories = get("categories").map_or_else(Vec::new, |v| {
            v.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
        });
        self.author = get("author").unwrap_or_default().to_string();
        self.player_count = get("players").map_or(Ok(0), |v| field("players", v))?;
        self.bounds = get("bounds").map_or(Ok(Rectangle::default()), |v| field("bounds", v))?;
        self.tile_set = get("tileset").unwrap_or_default().to_string();
        self.map_format = get("mapformat").map_or(Ok(0), |v| field("mapformat", v))?;
        let spawns: Vec<i32> = get("spawnpoints").map_or(Ok(Vec::new()), |v| field("spawnpoints", v))?;
        self.spawn_points = spawns.as_chunks::<2>().0.iter().map(|[x, y]| CPos::new(*x, *y)).collect();
        if get("map_grid_type").is_some_and(|v| v.trim() == "RectangularIsometric") {
            self.grid_type = MapGridType::RectangularIsometric;
        }
        self.minimap_png = base64("minimap").ok();
        let players = String::from_utf8_lossy(&base64("players_block")?).into_owned();
        let players = MiniYaml::from_str(&players, "players_block", true)?;
        self.players = players
            .iter()
            .find(|n| n.key == "Players")
            .map_or_else(|| map_players(&players), |n| map_players(&n.value.nodes));
        if !compatibility.iter().any(|c| Some(c.as_str()) == get("game_mod")) {
            self.status = MapStatus::Unavailable;
        }
        Ok(())
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
        download_bytes: 0,
        download_percentage: 0,
        parent: Some(parent.clone()),
        entry: entry.to_string(),
    })
}

/// Port of `MapCache`.
pub struct MapCache {
    pub previews: IndexMap<String, Rc<MapPreview>>,
    grid_type: MapGridType,
    compatibility: Vec<String>,
    /// The first `User` map folder with its path on disk, where downloaded maps go.
    user_folder: Option<(Arc<dyn Package>, std::path::PathBuf)>,
}

impl Default for MapCache {
    fn default() -> Self {
        MapCache {
            previews: IndexMap::new(),
            grid_type: MapGridType::Rectangular,
            compatibility: Vec::new(),
            user_folder: None,
        }
    }
}

impl MapCache {
    /// Port of `LoadMaps`. A map that fails to load is skipped with a log line.
    pub fn load(mod_data: &ModData) -> MapCache {
        let manifest = &mod_data.manifest;
        let compatibility = &manifest.map_compatibility;
        let grid_type = match manifest.section("MapGrid").and_then(|g| g.nodes.iter().find(|n| n.key == "Type")) {
            Some(node) if node.value.value_str() == "RectangularIsometric" => MapGridType::RectangularIsometric,
            _ => MapGridType::Rectangular,
        };
        let mut cache = MapCache {
            grid_type,
            compatibility: compatibility.clone(),
            ..MapCache::default()
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
            if class == MapClassification::User && cache.user_folder.is_none() && std::path::Path::new(&resolved).is_dir() {
                cache.user_folder = Some((folder.clone(), resolved.clone().into()));
            }
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

    /// The `MapCache` indexer: the preview of `uid`, added as an unknown map when the cache lacks it.
    pub fn entry(&mut self, uid: &str) -> Rc<MapPreview> {
        let grid_type = self.grid_type;
        self.previews
            .entry(uid.to_string())
            .or_insert_with(|| Rc::new(MapPreview::unknown(uid, grid_type)))
            .clone()
    }

    /// Replaces the preview of `uid` with a changed copy. Holders of the old `Rc` keep the old state.
    pub fn update(&mut self, uid: &str, change: impl FnOnce(&mut MapPreview)) {
        let mut preview = (*self.entry(uid)).clone();
        change(&mut preview);
        self.previews.insert(uid.to_string(), Rc::new(preview));
    }

    /// Port of `UpdateRemoteSearch`. `yaml` is the `RemoteMapData` for the `DownloadAvailable` status.
    pub fn update_remote_search(&mut self, uid: &str, status: MapStatus, yaml: Option<&MiniYaml>) {
        let compatibility = self.compatibility.clone();
        self.update(uid, |preview| {
            preview.status = status;
            preview.class = MapClassification::Remote;
            if let (MapStatus::DownloadAvailable, Some(yaml)) = (status, yaml)
                && let Err(e) = preview.apply_remote_data(yaml, &compatibility)
            {
                log::debug!("Failed parsing mapserver response: {e}");
            }
        });
    }

    /// The install part of `MapPreview.Install`: writes the downloaded `data` as `filename` into the user map folder
    /// and loads it in place of the remote preview.
    pub fn install(&mut self, uid: &str, filename: &str, data: &[u8]) -> Result<(), GameError> {
        let (folder, path) = self
            .user_folder
            .clone()
            .ok_or_else(|| GameError::Data("Map install directory not found".into()))?;
        let filename = std::path::Path::new(filename)
            .file_name()
            .and_then(|f| f.to_str())
            .ok_or_else(|| GameError::Data(format!("invalid map file name {filename}")))?;
        std::fs::write(path.join(filename), data).map_err(|e| GameError::Data(e.to_string()))?;
        log::debug!("Downloaded map to '{filename}'");
        let package: Arc<dyn Package> = Arc::new(
            rustora_fs::ZipPackage::open_file(&path.join(filename)).ok_or_else(|| GameError::Data(format!("cannot open {filename}")))?,
        );
        let preview = preview_from_package(
            &package,
            &folder,
            filename,
            MapClassification::User,
            &self.compatibility,
            self.grid_type,
        )?;
        if preview.uid != uid {
            log::warn!("Downloaded map {filename} has the uid {} instead of {uid}", preview.uid);
        }
        self.previews.insert(preview.uid.clone(), Rc::new(preview));
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_remote_map_data() {
        let players = "PlayerReference@Multi0:\n\tName: Multi0\n\tPlayable: True\n";
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, players);
        let text = format!(
            "abc:\n\ttitle: Freezer\n\tauthor: Upps\n\tplayers: 2\n\tgame_mod: ra\n\tbounds: 1,1,100,90\n\tspawnpoints: 85,74, 16,17\n\ttileset: SNOW\n\tdownloading: True\n\tmapformat: 12\n\tmap_grid_type: Rectangular\n\tcategories: RAGL-17\n\tplayers_block: {encoded}\n\tminimap: iVBO\n"
        );
        let nodes = MiniYaml::from_str(&text, "test", true).unwrap();
        let mut cache = MapCache {
            compatibility: vec!["ra".into()],
            ..MapCache::default()
        };
        cache.update_remote_search("abc", MapStatus::DownloadAvailable, Some(&nodes[0].value));
        let map = cache.get("abc").unwrap();
        assert_eq!(map.status, MapStatus::DownloadAvailable);
        assert_eq!(map.class, MapClassification::Remote);
        assert_eq!((map.title.as_str(), map.author.as_str(), map.player_count), ("Freezer", "Upps", 2));
        assert_eq!(map.spawn_points, vec![CPos::new(85, 74), CPos::new(16, 17)]);
        assert_eq!(map.categories, vec!["RAGL-17".to_string()]);
        assert_eq!(map.players.len(), 1);
        assert!(map.players[0].playable);

        cache.compatibility = vec!["cnc".into()];
        cache.update_remote_search("cnc", MapStatus::DownloadAvailable, Some(&nodes[0].value));
        assert_eq!(cache.get("cnc").unwrap().status, MapStatus::Unavailable);
    }
}
