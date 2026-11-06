// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! What the server reads from the mod and its maps: game speeds, the map cache, and per map the players, spawn
//! points, lobby options (`ILobbyOptions` implementers), bots, factions and player color rules.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use indexmap::IndexMap;
use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::MiniYaml;
use rustora_engine::actor_info::{TraitNode, parse_trait_nodes};
use rustora_engine::terrain::TerrainInfo;
use rustora_fs::{MapFs, Package, Platform};
use rustora_game::color_picker::ColorPicker;
use rustora_game::map::MapPackage;
use rustora_game::moddata::ModData;
use rustora_mods_common::world::{PlayerReference, map_players};
use rustora_net::session::{LobbyOptionState, Session};

/// `Session.MapStatus.Playable`.
pub const MAP_STATUS_PLAYABLE: i32 = 2;
/// `Session.MapStatus.Incompatible`.
pub const MAP_STATUS_INCOMPATIBLE: i32 = 4;
/// `Session.MapStatus.UnsafeCustomRules`.
pub const MAP_STATUS_UNSAFE_CUSTOM_RULES: i32 = 8;
/// `MapPlayers.MaximumPlayerCount`.
const MAXIMUM_PLAYER_COUNT: usize = 64;
/// The traits that implement `ILobbyCustomRulesIgnore`.
const CUSTOM_RULES_IGNORED: [&str; 8] = [
    "TerrainLightSource",
    "TintPostProcessEffect",
    "ElevatedBridgeLayer",
    "WeatherOverlay",
    "TerrainTunnel",
    "TerrainLighting",
    "ElevatedBridgePlaceholder",
    "TerrainTunnelLayer",
];

#[derive(Clone, Debug)]
pub struct GameSpeed {
    pub timestep: i32,
    pub order_latency: i32,
}

/// `Traits.LobbyOption`; `name` and `values` hold the text shown to players.
#[derive(Clone, Debug)]
pub struct LobbyOption {
    pub id: String,
    pub name: String,
    pub values: IndexMap<String, String>,
    pub default_value: String,
    pub is_locked: bool,
    pub is_boolean: bool,
    pub description: Option<String>,
    pub is_visible: bool,
    pub display_order: i32,
}

impl LobbyOption {
    /// Reads `{prefix}Description`, `{prefix}Visible` and `{prefix}DisplayOrder` of the trait. The default description
    /// key has the label key with `.description` for `.label`.
    fn decorated(mut self, game_mod: &ServerMod, t: &TraitNode, prefix: &str, label_key: &str) -> LobbyOption {
        let default_key = label_key.strip_suffix(".label").map(|k| format!("{k}.description"));
        let key = field(t, &format!("{prefix}Description")).map(str::to_string).or(default_key);
        self.description = key.map(|k| game_mod.message(&k).replace("\\n", "\n"));
        self.is_visible = field_bool(t, &format!("{prefix}Visible"), true);
        self.display_order = field(t, &format!("{prefix}DisplayOrder"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        self
    }

    /// `LobbyOption.Label`.
    pub fn label(&self, value: &str) -> String {
        let text = self.values.get(value).map_or(value, String::as_str);
        if self.is_boolean { text.to_lowercase() } else { text.to_string() }
    }
}

/// `FactionInfo` with message keys.
#[derive(Clone, Debug)]
pub struct FactionData {
    pub internal_name: String,
    pub name: String,
    pub side: Option<String>,
    pub description: Option<String>,
    pub selectable: bool,
}

/// `IBotInfo`.
#[derive(Clone, Debug)]
pub struct BotInfo {
    pub bot_type: String,
    pub name: String,
}

/// The `MapPreview` of the server with the rules it needs.
pub struct MapInfo {
    pub uid: String,
    pub title: String,
    pub players: IndexMap<String, PlayerReference>,
    pub spawn_points: usize,
    pub status: i32,
    pub options: Vec<LobbyOption>,
    pub bots: Vec<BotInfo>,
    pub selectable_factions: Vec<String>,
    pub factions: Vec<FactionData>,
    pub color_picker: ColorPicker,
    pub restricted_colors: Vec<Color>,
    pub briefing: Option<String>,
    /// The `MissionData` trait of the world, if there is one.
    pub mission: Option<MissionData>,
}

/// The fields of `MissionDataInfo` that the mission browser shows.
#[derive(Clone, Debug, Default)]
pub struct MissionData {
    pub briefing: Option<String>,
    pub briefing_video: Option<String>,
    pub background_video: Option<String>,
    pub start_video: Option<String>,
}

impl MapInfo {
    /// `LobbyCommands.LoadMapSettings`: sets every option of this map in `lobby`, keeping preferred values.
    pub fn load_map_settings(&self, lobby: &mut Session) {
        for o in &self.options {
            let state = lobby.global_settings.lobby_options.entry(o.id.clone()).or_default();
            let mut value = o.default_value.clone();
            let mut preferred = o.default_value.clone();
            if let (Some(p), Some(v)) = (&state.preferred_value, &state.value) {
                if !o.is_locked {
                    if o.values.contains_key(p) {
                        value = p.clone();
                    } else if o.values.contains_key(v) {
                        value = v.clone();
                    }
                }
                preferred = p.clone();
            }
            *state = LobbyOptionState {
                value: Some(value),
                preferred_value: Some(preferred),
                is_locked: o.is_locked,
            };
        }
    }

    /// `LobbyUtils.InsufficientEnabledSpawnPoints`.
    pub fn insufficient_enabled_spawn_points(&self, lobby: &Session) -> bool {
        if self.spawn_points == 0 {
            return false;
        }
        let available = (1..=self.spawn_points as i32)
            .filter(|s| !lobby.disabled_spawn_points.contains(s))
            .count();
        available < lobby.clients.iter().filter(|c| !c.is_observer()).count()
    }
}

struct MapEntry {
    package: Arc<dyn Package>,
    map: MapPackage,
}

/// The loaded mod, like `ModData` with its `MapCache`.
pub struct ServerMod {
    pub data: ModData,
    pub default_speed: String,
    pub speeds: IndexMap<String, GameSpeed>,
    speed_names: IndexMap<String, String>,
    fluent: HashMap<String, String>,
    /// The `FluentMessages` of the map whose information loads, the fallback bundle of `MapPreview.GetMessage`.
    map_fluent: std::cell::RefCell<HashMap<String, String>>,
    restricted_colors: HashMap<String, Vec<Color>>,
    maps: IndexMap<String, MapEntry>,
    infos: HashMap<String, Rc<MapInfo>>,
}

fn field<'a>(t: &'a TraitNode, key: &str) -> Option<&'a str> {
    t.yaml.nodes.iter().find(|n| n.key == key).map(|n| n.value.value_str())
}

fn field_bool(t: &TraitNode, key: &str, default: bool) -> bool {
    field(t, key).map_or(default, |v| v.trim().eq_ignore_ascii_case("true"))
}

/// `Exts.TryParseInt32Invariant` based lists like `TimeLimitOptions: 0, 10`.
fn field_ints(t: &TraitNode, key: &str, default: &[i32]) -> Vec<i32> {
    field(t, key).map_or_else(
        || default.to_vec(),
        |v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
    )
}

/// Reads `ColorPickerManagerInfo` from the traits of the world actor.
fn read_color_picker(world: &[TraitNode]) -> ColorPicker {
    let mut color_picker = ColorPicker::default();
    if let Some(t) = world.iter().find(|t| t.name == "ColorPickerManager") {
        let floats = |k: &str, d: [f32; 2]| -> [f32; 2] {
            let v: Vec<f32> = field(t, k).map_or_else(Vec::new, |v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect());
            if v.len() == 2 { [v[0], v[1]] } else { d }
        };
        color_picker.saturation_range = floats("HsvSaturationRange", color_picker.saturation_range);
        color_picker.value_range = floats("HsvValueRange", color_picker.value_range);
        if let Some(v) = field(t, "SimilarityThreshold").and_then(|v| v.trim().parse().ok()) {
            color_picker.similarity_threshold = v;
        }
        color_picker.preset_colors = field(t, "PresetColors").map_or_else(Vec::new, |v| v.split(',').filter_map(Color::parse).collect());
    }
    color_picker
}

fn parse_fluent(text: &str, out: &mut HashMap<String, String>) {
    let mut current = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if let Some(attribute) = key.strip_prefix('.') {
            out.insert(format!("{current}.{attribute}"), value.to_string());
        } else if !line.starts_with(char::is_whitespace) {
            current = key.to_string();
            if !value.is_empty() {
                out.insert(current.clone(), value.to_string());
            }
        }
    }
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
        let game_speeds = data.manifest.yaml.get("GameSpeeds").ok_or("mod has no GameSpeeds")?;
        let default_speed = game_speeds
            .node_with_key("DefaultSpeed")
            .map_err(|e| e.to_string())?
            .value
            .value_str()
            .to_string();
        let mut speeds = IndexMap::new();
        let mut speed_names = IndexMap::new();
        for n in &game_speeds.node_with_key("Speeds").map_err(|e| e.to_string())?.value.nodes {
            let get = |k: &str| n.value.nodes.iter().find(|c| c.key == k).map(|c| c.value.value_str().to_string());
            let int = |k: &str| get(k).and_then(|v| v.trim().parse().ok());
            speeds.insert(
                n.key.clone(),
                GameSpeed {
                    timestep: int("Timestep").unwrap_or(40),
                    order_latency: int("OrderLatency").unwrap_or(3),
                },
            );
            speed_names.insert(n.key.clone(), get("Name").unwrap_or_default());
        }

        let mut fluent = HashMap::new();
        for f in &data.manifest.fluent_messages {
            if let Some(bytes) = data.file_system.try_open(f) {
                parse_fluent(&String::from_utf8_lossy(&bytes), &mut fluent);
            }
        }

        let mut restricted_colors = HashMap::new();
        for f in &data.manifest.tile_sets {
            let y = ModData::load_yaml(&data.file_system, std::slice::from_ref(f), None).map_err(|e| e.to_string())?;
            let info = TerrainInfo::load(&y)?;
            let colors = info
                .terrain_types
                .iter()
                .filter(|t| t.restrict_player_color)
                .map(|t| t.color)
                .collect();
            restricted_colors.insert(info.id, colors);
        }

        let maps = scan_maps(&data);
        Ok(ServerMod {
            data,
            default_speed,
            speeds,
            speed_names,
            fluent,
            map_fluent: Default::default(),
            restricted_colors,
            maps,
            infos: HashMap::new(),
        })
    }

    /// The text of a fluent message or attribute without arguments, or the key if unknown. The mod messages take
    /// precedence over those of the map whose information loads.
    pub fn message(&self, key: &str) -> String {
        if let Some(m) = self.fluent.get(key) {
            return m.clone();
        }
        self.map_fluent.borrow().get(key).cloned().unwrap_or_else(|| key.to_string())
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

    /// `ColorPickerManagerInfo` of the default mod rules and the restricted colors of all tile sets.
    pub fn default_color_rules(&self) -> Result<(ColorPicker, Vec<Color>), String> {
        let rules = ModData::load_yaml(&self.data.file_system, &self.data.manifest.rules, None).map_err(|e| e.to_string())?;
        let world = rules
            .iter()
            .find(|n| n.key.eq_ignore_ascii_case("World"))
            .ok_or("rules lack World")?;
        let world = parse_trait_nodes("World", &world.value.nodes).map_err(|e| e.to_string())?;
        let mut terrain: Vec<Color> = Vec::new();
        for color in self.restricted_colors.values().flatten() {
            if !terrain.contains(color) {
                terrain.push(*color);
            }
        }
        Ok((read_color_picker(&world), terrain))
    }

    /// The rules of the map `uid`, loaded on first use.
    pub fn map_info(&mut self, uid: &str) -> Result<Rc<MapInfo>, String> {
        if let Some(i) = self.infos.get(uid) {
            return Ok(i.clone());
        }
        if !self.maps.contains_key(uid) {
            self.rescan_maps();
        }
        let info = Rc::new(self.load_map_info(uid)?);
        self.infos.insert(uid.to_string(), info.clone());
        Ok(info)
    }

    fn load_map_info(&self, uid: &str) -> Result<MapInfo, String> {
        let entry = self.maps.get(uid).ok_or_else(|| format!("unknown map {uid}"))?;
        let map = &entry.map;
        let fs = MapFs {
            package: entry.package.as_ref(),
            fs: &self.data.file_system,
        };
        let mut map_fluent = HashMap::new();
        for file in map
            .section("FluentMessages")
            .map(MiniYaml::value_str)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|f| !f.is_empty())
        {
            if let Some(bytes) = rustora_fs::ReadFs::try_open(&fs, file) {
                parse_fluent(&String::from_utf8_lossy(&bytes), &mut map_fluent);
            }
        }
        *self.map_fluent.borrow_mut() = map_fluent;
        let info = self.build_map_info(uid, entry, &fs);
        self.map_fluent.borrow_mut().clear();
        info
    }

    fn build_map_info(&self, uid: &str, entry: &MapEntry, fs: &MapFs) -> Result<MapInfo, String> {
        let map = &entry.map;
        let rules = ModData::load_yaml(fs, &self.data.manifest.rules, map.section("Rules")).map_err(|e| e.to_string())?;
        let actor = |name: &str| -> Result<Vec<TraitNode>, String> {
            let n = rules
                .iter()
                .find(|n| n.key.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("rules lack {name}"))?;
            parse_trait_nodes(name, &n.value.nodes).map_err(|e| e.to_string())
        };
        let (player, world) = (actor("Player")?, actor("World")?);
        let players = map_players(map.section_nodes("Players"));
        let spawn_points = map
            .section_nodes("Actors")
            .iter()
            .filter(|n| n.value.value_str() == "mpspawn")
            .count();
        let tileset = map.section("Tileset").map_or("", MiniYaml::value_str);

        let mut status = MAP_STATUS_PLAYABLE;
        if self.defines_unsafe_custom_rules(entry) {
            status |= MAP_STATUS_UNSAFE_CUSTOM_RULES;
        }
        if players.len() > MAXIMUM_PLAYER_COUNT {
            status = MAP_STATUS_INCOMPATIBLE;
        }

        let color_picker = read_color_picker(&world);

        Ok(MapInfo {
            uid: uid.to_string(),
            title: map.section("Title").map_or("", MiniYaml::value_str).to_string(),
            spawn_points,
            status,
            options: self.lobby_options(&player, &world),
            bots: player
                .iter()
                .filter(|t| t.name == "ModularBot")
                .filter_map(|t| {
                    Some(BotInfo {
                        bot_type: field(t, "Type")?.to_string(),
                        name: field(t, "Name").unwrap_or_default().to_string(),
                    })
                })
                .collect(),
            selectable_factions: world
                .iter()
                .filter(|t| t.name == "Faction" && field_bool(t, "Selectable", true))
                .filter_map(|t| field(t, "InternalName").map(str::to_string))
                .collect(),
            factions: world
                .iter()
                .filter(|t| t.name == "Faction")
                .filter_map(|t| {
                    Some(FactionData {
                        internal_name: field(t, "InternalName")?.to_string(),
                        name: field(t, "Name").unwrap_or_default().to_string(),
                        side: field(t, "Side").map(str::to_string),
                        description: field(t, "Description").map(str::to_string),
                        selectable: field_bool(t, "Selectable", true),
                    })
                })
                .collect(),
            color_picker,
            restricted_colors: self.restricted_colors.get(tileset).cloned().unwrap_or_default(),
            briefing: world
                .iter()
                .find(|t| t.name == "MissionData")
                .and_then(|t| field(t, "Briefing"))
                .filter(|b| !b.is_empty())
                .map(|b| b.replace("\\n", "\n")),
            mission: world.iter().find(|t| t.name == "MissionData").map(|t| MissionData {
                briefing: field(t, "Briefing").map(str::to_string),
                briefing_video: field(t, "BriefingVideo").map(str::to_string),
                background_video: field(t, "BackgroundVideo").map(str::to_string),
                start_video: field(t, "StartVideo").map(str::to_string),
            }),
            players,
        })
    }

    /// `Ruleset.DefinesUnsafeCustomRules` of a map.
    fn defines_unsafe_custom_rules(&self, entry: &MapEntry) -> bool {
        let map = &entry.map;
        let custom = |k: &str| map.section(k).is_some_and(|y| !y.value_str().is_empty() || !y.nodes.is_empty());
        if ["Weapons", "Voices", "Notifications", "Sequences"].iter().any(|k| custom(k)) {
            return true;
        }
        let flagged = |actors: &[rustora_core::miniyaml::MiniYamlNode]| {
            actors.iter().any(|a| {
                a.value.nodes.iter().any(|t| {
                    let name = t.key.split('@').next().unwrap_or("");
                    !name.starts_with('-') && name != "Inherits" && !CUSTOM_RULES_IGNORED.contains(&name)
                })
            })
        };
        let Some(rules) = map.section("Rules") else {
            return false;
        };
        if flagged(&rules.nodes) {
            return true;
        }
        rules.value_str().split(',').map(str::trim).filter(|f| !f.is_empty()).any(|f| {
            let fs = MapFs {
                package: entry.package.as_ref(),
                fs: &self.data.file_system,
            };
            rustora_fs::ReadFs::try_open(&fs, f)
                .and_then(|d| MiniYaml::from_bytes(&d, f, true).ok())
                .is_some_and(|nodes| flagged(&nodes))
        })
    }

    fn boolean_option(&self, t: &TraitNode, id: &str, label: &str, prefix: &str, enabled: bool) -> LobbyOption {
        let name = field(t, &format!("{prefix}Label")).unwrap_or(label);
        let default_value = if field_bool(t, &format!("{prefix}Enabled"), enabled) {
            "True"
        } else {
            "False"
        };
        LobbyOption {
            id: id.to_string(),
            name: self.message(name),
            values: IndexMap::from([
                ("True".to_string(), "Enabled".to_string()),
                ("False".to_string(), "Disabled".to_string()),
            ]),
            default_value: default_value.to_string(),
            is_locked: field_bool(t, &format!("{prefix}Locked"), false),
            is_boolean: true,
            description: None,
            is_visible: true,
            display_order: 0,
        }
        .decorated(self, t, prefix, name)
    }

    fn dropdown_option(
        &self,
        id: &str,
        name: &str,
        values: IndexMap<String, String>,
        default_value: String,
        is_locked: bool,
    ) -> LobbyOption {
        LobbyOption {
            id: id.to_string(),
            name: self.message(name),
            values,
            default_value,
            is_locked,
            is_boolean: false,
            description: None,
            is_visible: true,
            display_order: 0,
        }
    }

    /// The `ILobbyOptions.LobbyOptions` of all traits of the player and world actors, the player traits first.
    fn lobby_options(&self, player: &[TraitNode], world: &[TraitNode]) -> Vec<LobbyOption> {
        let mut out = Vec::new();
        for t in player.iter().chain(world) {
            match t.name.as_str() {
                "LobbyPrerequisiteCheckbox" => {
                    let id = field(t, "ID").unwrap_or_default();
                    let label = field(t, "Label").unwrap_or_default();
                    out.push(self.boolean_option(t, id, label, "", false));
                }
                "PlayerResources" => {
                    let values: IndexMap<String, String> = field_ints(t, "SelectableCash", &[2500, 5000, 10000, 20000])
                        .into_iter()
                        .map(|c| (c.to_string(), format!("${c}")))
                        .collect();
                    if !values.is_empty() {
                        let label = field(t, "DefaultCashDropdownLabel").unwrap_or("Starting Cash");
                        out.push(
                            self.dropdown_option(
                                "startingcash",
                                label,
                                values,
                                field(t, "DefaultCash").unwrap_or("5000").trim().to_string(),
                                field_bool(t, "DefaultCashDropdownLocked", false),
                            )
                            .decorated(self, t, "DefaultCashDropdown", label),
                        );
                    }
                }
                "DeveloperMode" => out.push(self.boolean_option(t, "cheats", "checkbox-debug-menu.label", "Checkbox", false)),
                "Shroud" => {
                    out.push(self.boolean_option(t, "explored", "checkbox-explored-map.label", "ExploredMapCheckbox", false));
                    out.push(self.boolean_option(t, "fog", "checkbox-fog-of-war.label", "FogCheckbox", true));
                }
                "MapOptions" => {
                    out.push(self.boolean_option(t, "shortgame", "checkbox-short-game.label", "ShortGameCheckbox", true));
                    let tech: IndexMap<String, String> = player
                        .iter()
                        .filter(|p| p.name == "ProvidesTechPrerequisite")
                        .filter_map(|p| Some((field(p, "Id")?.to_string(), self.message(field(p, "Name")?))))
                        .collect();
                    if !tech.is_empty() {
                        let label = field(t, "TechLevelDropdownLabel").unwrap_or("dropdown-tech-level.label");
                        out.push(
                            self.dropdown_option(
                                "techlevel",
                                label,
                                tech,
                                field(t, "TechLevel").unwrap_or("unrestricted").to_string(),
                                field_bool(t, "TechLevelDropdownLocked", false),
                            )
                            .decorated(self, t, "TechLevelDropdown", label),
                        );
                    }
                    let speeds = self.speed_names.iter().map(|(k, n)| (k.clone(), self.message(n))).collect();
                    let label = field(t, "GameSpeedDropdownLabel").unwrap_or("dropdown-game-speed.label");
                    out.push(
                        self.dropdown_option(
                            "gamespeed",
                            label,
                            speeds,
                            field(t, "GameSpeed").unwrap_or(&self.default_speed).to_string(),
                            field_bool(t, "GameSpeedDropdownLocked", false),
                        )
                        .decorated(self, t, "GameSpeedDropdown", label),
                    );
                }
                "MapBuildRadius" => {
                    out.push(self.boolean_option(t, "allybuild", "checkbox-ally-build-radius.label", "AllyBuildRadiusCheckbox", true));
                    out.push(self.boolean_option(t, "buildradius", "checkbox-build-radius.label", "BuildRadiusCheckbox", true));
                }
                "CrateSpawner" => out.push(self.boolean_option(t, "crates", "checkbox-crates.label", "Checkbox", true)),
                "MapCreeps" => out.push(self.boolean_option(t, "creeps", "dropdown-map-creeps.label", "Checkbox", true)),
                "MapStartingLocations" => out.push(self.boolean_option(
                    t,
                    "separateteamspawns",
                    "checkbox-separate-team-spawns.label",
                    "SeparateTeamSpawnsCheckbox",
                    true,
                )),
                "SpawnStartingUnits" => {
                    let units: IndexMap<String, String> = world
                        .iter()
                        .filter(|w| w.name == "StartingUnits")
                        .filter_map(|w| {
                            Some((
                                field(w, "Class")?.to_string(),
                                self.message(field(w, "ClassName").unwrap_or_default()),
                            ))
                        })
                        .collect();
                    if !units.is_empty() {
                        let label = field(t, "DropdownLabel").unwrap_or("dropdown-starting-units.label");
                        out.push(
                            self.dropdown_option(
                                "startingunits",
                                label,
                                units,
                                field(t, "StartingUnitsClass").unwrap_or("none").to_string(),
                                field_bool(t, "DropdownLocked", false),
                            )
                            .decorated(self, t, "Dropdown", label),
                        );
                    }
                }
                "TimeLimitManager" => {
                    let values = field_ints(t, "TimeLimitOptions", &[0, 10, 20, 30, 40, 60, 90])
                        .into_iter()
                        .map(|m| (m.to_string(), if m == 0 { "No limit".to_string() } else { format!("{m} minutes") }))
                        .collect();
                    let label = field(t, "TimeLimitLabel").unwrap_or("dropdown-time-limit.label");
                    let mut option = self
                        .dropdown_option(
                            "timelimit",
                            label,
                            values,
                            field(t, "TimeLimitDefault").unwrap_or("0").trim().to_string(),
                            field_bool(t, "TimeLimitLocked", false),
                        )
                        .decorated(self, t, "TimeLimit", label);
                    option.is_visible = field_bool(t, "TimeLimitDropdownVisible", true);
                    out.push(option);
                }
                "ScriptLobbyDropdown" => {
                    let values = t.yaml.nodes.iter().find(|n| n.key == "Values").map_or_else(IndexMap::new, |v| {
                        v.value
                            .nodes
                            .iter()
                            .map(|n| (n.key.clone(), self.message(n.value.value_str())))
                            .collect()
                    });
                    let label = field(t, "Label").unwrap_or_default();
                    out.push(
                        self.dropdown_option(
                            field(t, "ID").unwrap_or_default(),
                            label,
                            values,
                            field(t, "Default").unwrap_or_default().to_string(),
                            field_bool(t, "Locked", false),
                        )
                        .decorated(self, t, "", label),
                    );
                }
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fluent_messages_and_attributes() {
        let mut m = HashMap::new();
        parse_fluent(
            "# comment\ncheckbox-short-game =\n    .label = Short Game\n    .description = Bases\n\nplain = Text here\n",
            &mut m,
        );
        assert_eq!(m["checkbox-short-game.label"], "Short Game");
        assert_eq!(m["plain"], "Text here");
    }
}
