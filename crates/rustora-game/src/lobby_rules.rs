// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! What the lobby reads from the mod and its maps: game speeds, and per map the players, spawn points, lobby
//! options (`ILobbyOptions` implementers), bots, factions and player color rules. The server and the client lobby
//! share it, like `MapPreview` and the rulesets in C#.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_fs::{MapFs, Package};
use rustora_net::session::{LobbyOptionState, Session};

use crate::actor_yaml::{TraitNode, parse_trait_nodes};
use crate::color_picker::ColorPicker;
use crate::map::MapPackage;
use crate::moddata::ModData;
use crate::players::{PlayerReference, map_players};

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
    fn decorated(mut self, messages: &Messages, t: &TraitNode, prefix: &str, label_key: &str) -> LobbyOption {
        let default_key = label_key.strip_suffix(".label").map(|k| format!("{k}.description"));
        let key = field(t, &format!("{prefix}Description")).map(str::to_string).or(default_key);
        self.description = key.map(|k| messages.get(&k).replace("\\n", "\n"));
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

/// The lobby rules of a map, what `MapPreview` and its `Ruleset` tell the lobby.
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

/// The `Id` of a tile set and the colors of its terrain types with `RestrictPlayerColor`, in the type order of
/// `TerrainInfo`.
fn restricted_player_colors(tileset: &[MiniYamlNode]) -> Result<(String, Vec<Color>), String> {
    let section = |k: &str| {
        tileset
            .iter()
            .find(|n| n.key == k)
            .map(|n| &n.value)
            .ok_or_else(|| format!("tileset lacks {k}"))
    };
    let field = |y: &MiniYaml, k: &str| y.nodes.iter().find(|n| n.key == k).map(|n| n.value.value_str().to_string());
    let id = field(section("General")?, "Id").unwrap_or_default();
    let mut types: Vec<(String, Color)> = section("Terrain")?
        .nodes
        .iter()
        .filter(|n| field(&n.value, "RestrictPlayerColor").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")))
        .filter_map(|n| Some((field(&n.value, "Type")?, Color::parse(&field(&n.value, "Color")?)?)))
        .collect();
    types.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((id, types.into_iter().map(|(_, c)| c).collect()))
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

/// The Fluent messages of the mod and, as fallback, of a map, like `MapPreview.GetMessage`. Messages without
/// arguments only; an unknown key stands for itself.
pub struct Messages<'a> {
    pub mod_messages: &'a HashMap<String, String>,
    pub map_messages: &'a HashMap<String, String>,
}

impl Messages<'_> {
    pub fn get(&self, key: &str) -> String {
        self.mod_messages
            .get(key)
            .or_else(|| self.map_messages.get(key))
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }
}

/// The lobby relevant rules of a mod: game speeds, Fluent messages and the colors that tile sets restrict.
pub struct ModLobbyRules {
    pub default_speed: String,
    pub speeds: IndexMap<String, GameSpeed>,
    speed_names: IndexMap<String, String>,
    fluent: HashMap<String, String>,
    restricted_colors: HashMap<String, Vec<Color>>,
}

impl ModLobbyRules {
    pub fn load(data: &ModData) -> Result<ModLobbyRules, String> {
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
            let (id, colors) = restricted_player_colors(&y)?;
            restricted_colors.insert(id, colors);
        }
        Ok(ModLobbyRules {
            default_speed,
            speeds,
            speed_names,
            fluent,
            restricted_colors,
        })
    }

    /// The text of a mod Fluent message or attribute without arguments, or the key if unknown.
    pub fn message(&self, key: &str) -> String {
        self.fluent.get(key).cloned().unwrap_or_else(|| key.to_string())
    }

    /// `ColorPickerManagerInfo` of the default mod rules and the restricted colors of all tile sets.
    pub fn default_color_rules(&self, data: &ModData) -> Result<(ColorPicker, Vec<Color>), String> {
        let rules = ModData::load_yaml(&data.file_system, &data.manifest.rules, None).map_err(|e| e.to_string())?;
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

    /// The lobby rules of `map`, read from its package `package` with the mod `data`.
    pub fn map_info(&self, data: &ModData, package: &Arc<dyn Package>, map: &MapPackage) -> Result<MapInfo, String> {
        let fs = MapFs {
            package: package.as_ref(),
            fs: &data.file_system,
        };
        let mut map_messages = HashMap::new();
        for file in map
            .section("FluentMessages")
            .map(MiniYaml::value_str)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|f| !f.is_empty())
        {
            if let Some(bytes) = rustora_fs::ReadFs::try_open(&fs, file) {
                parse_fluent(&String::from_utf8_lossy(&bytes), &mut map_messages);
            }
        }
        let messages = Messages {
            mod_messages: &self.fluent,
            map_messages: &map_messages,
        };
        self.build_map_info(data, map, &fs, &messages)
    }

    fn build_map_info(&self, data: &ModData, map: &MapPackage, fs: &MapFs, messages: &Messages) -> Result<MapInfo, String> {
        let rules = ModData::load_yaml(fs, &data.manifest.rules, map.section("Rules")).map_err(|e| e.to_string())?;
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
        if defines_unsafe_custom_rules(map, fs) {
            status |= MAP_STATUS_UNSAFE_CUSTOM_RULES;
        }
        if players.len() > MAXIMUM_PLAYER_COUNT {
            status = MAP_STATUS_INCOMPATIBLE;
        }

        let color_picker = read_color_picker(&world);

        Ok(MapInfo {
            uid: map.uid.clone(),
            title: map.section("Title").map_or("", MiniYaml::value_str).to_string(),
            spawn_points,
            status,
            options: self.lobby_options(&player, &world, messages),
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

    fn boolean_option(messages: &Messages, t: &TraitNode, id: &str, label: &str, prefix: &str, enabled: bool) -> LobbyOption {
        let name = field(t, &format!("{prefix}Label")).unwrap_or(label);
        let default_value = if field_bool(t, &format!("{prefix}Enabled"), enabled) {
            "True"
        } else {
            "False"
        };
        LobbyOption {
            id: id.to_string(),
            name: messages.get(name),
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
        .decorated(messages, t, prefix, name)
    }

    fn dropdown_option(
        messages: &Messages,
        id: &str,
        name: &str,
        values: IndexMap<String, String>,
        default_value: String,
        is_locked: bool,
    ) -> LobbyOption {
        LobbyOption {
            id: id.to_string(),
            name: messages.get(name),
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
    fn lobby_options(&self, player: &[TraitNode], world: &[TraitNode], messages: &Messages) -> Vec<LobbyOption> {
        let mut out = Vec::new();
        for t in player.iter().chain(world) {
            match t.name.as_str() {
                "LobbyPrerequisiteCheckbox" => {
                    let id = field(t, "ID").unwrap_or_default();
                    let label = field(t, "Label").unwrap_or_default();
                    out.push(Self::boolean_option(messages, t, id, label, "", false));
                }
                "PlayerResources" => {
                    let values: IndexMap<String, String> = field_ints(t, "SelectableCash", &[2500, 5000, 10000, 20000])
                        .into_iter()
                        .map(|c| (c.to_string(), format!("${c}")))
                        .collect();
                    if !values.is_empty() {
                        let label = field(t, "DefaultCashDropdownLabel").unwrap_or("Starting Cash");
                        out.push(
                            Self::dropdown_option(messages, 
                                "startingcash",
                                label,
                                values,
                                field(t, "DefaultCash").unwrap_or("5000").trim().to_string(),
                                field_bool(t, "DefaultCashDropdownLocked", false),
                            )
                            .decorated(messages, t, "DefaultCashDropdown", label),
                        );
                    }
                }
                "DeveloperMode" => out.push(Self::boolean_option(messages, t, "cheats", "checkbox-debug-menu.label", "Checkbox", false)),
                "Shroud" => {
                    out.push(Self::boolean_option(messages, t, "explored", "checkbox-explored-map.label", "ExploredMapCheckbox", false));
                    out.push(Self::boolean_option(messages, t, "fog", "checkbox-fog-of-war.label", "FogCheckbox", true));
                }
                "MapOptions" => {
                    out.push(Self::boolean_option(messages, t, "shortgame", "checkbox-short-game.label", "ShortGameCheckbox", true));
                    let tech: IndexMap<String, String> = player
                        .iter()
                        .filter(|p| p.name == "ProvidesTechPrerequisite")
                        .filter_map(|p| Some((field(p, "Id")?.to_string(), messages.get(field(p, "Name")?))))
                        .collect();
                    if !tech.is_empty() {
                        let label = field(t, "TechLevelDropdownLabel").unwrap_or("dropdown-tech-level.label");
                        out.push(
                            Self::dropdown_option(messages, 
                                "techlevel",
                                label,
                                tech,
                                field(t, "TechLevel").unwrap_or("unrestricted").to_string(),
                                field_bool(t, "TechLevelDropdownLocked", false),
                            )
                            .decorated(messages, t, "TechLevelDropdown", label),
                        );
                    }
                    let speeds = self.speed_names.iter().map(|(k, n)| (k.clone(), messages.get(n))).collect();
                    let label = field(t, "GameSpeedDropdownLabel").unwrap_or("dropdown-game-speed.label");
                    out.push(
                        Self::dropdown_option(messages, 
                            "gamespeed",
                            label,
                            speeds,
                            field(t, "GameSpeed").unwrap_or(&self.default_speed).to_string(),
                            field_bool(t, "GameSpeedDropdownLocked", false),
                        )
                        .decorated(messages, t, "GameSpeedDropdown", label),
                    );
                }
                "MapBuildRadius" => {
                    out.push(Self::boolean_option(messages, t, "allybuild", "checkbox-ally-build-radius.label", "AllyBuildRadiusCheckbox", true));
                    out.push(Self::boolean_option(messages, t, "buildradius", "checkbox-build-radius.label", "BuildRadiusCheckbox", true));
                }
                "CrateSpawner" => out.push(Self::boolean_option(messages, t, "crates", "checkbox-crates.label", "Checkbox", true)),
                "MapCreeps" => out.push(Self::boolean_option(messages, t, "creeps", "dropdown-map-creeps.label", "Checkbox", true)),
                "MapStartingLocations" => out.push(Self::boolean_option(
                    messages,
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
                                messages.get(field(w, "ClassName").unwrap_or_default()),
                            ))
                        })
                        .collect();
                    if !units.is_empty() {
                        let label = field(t, "DropdownLabel").unwrap_or("dropdown-starting-units.label");
                        out.push(
                            Self::dropdown_option(messages, 
                                "startingunits",
                                label,
                                units,
                                field(t, "StartingUnitsClass").unwrap_or("none").to_string(),
                                field_bool(t, "DropdownLocked", false),
                            )
                            .decorated(messages, t, "Dropdown", label),
                        );
                    }
                }
                "TimeLimitManager" => {
                    let values = field_ints(t, "TimeLimitOptions", &[0, 10, 20, 30, 40, 60, 90])
                        .into_iter()
                        .map(|m| (m.to_string(), if m == 0 { "No limit".to_string() } else { format!("{m} minutes") }))
                        .collect();
                    let label = field(t, "TimeLimitLabel").unwrap_or("dropdown-time-limit.label");
                    let mut option = Self::dropdown_option(
                            messages,
                            "timelimit",
                            label,
                            values,
                            field(t, "TimeLimitDefault").unwrap_or("0").trim().to_string(),
                            field_bool(t, "TimeLimitLocked", false),
                        )
                        .decorated(messages, t, "TimeLimit", label);
                    option.is_visible = field_bool(t, "TimeLimitDropdownVisible", true);
                    out.push(option);
                }
                "ScriptLobbyDropdown" => {
                    let values = t.yaml.nodes.iter().find(|n| n.key == "Values").map_or_else(IndexMap::new, |v| {
                        v.value
                            .nodes
                            .iter()
                            .map(|n| (n.key.clone(), messages.get(n.value.value_str())))
                            .collect()
                    });
                    let label = field(t, "Label").unwrap_or_default();
                    out.push(
                        Self::dropdown_option(messages, 
                            field(t, "ID").unwrap_or_default(),
                            label,
                            values,
                            field(t, "Default").unwrap_or_default().to_string(),
                            field_bool(t, "Locked", false),
                        )
                        .decorated(messages, t, "", label),
                    );
                }
                _ => {}
            }
        }
        out
    }
}

/// `Ruleset.DefinesUnsafeCustomRules` of a map.
fn defines_unsafe_custom_rules(map: &MapPackage, fs: &MapFs) -> bool {
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
        rustora_fs::ReadFs::try_open(fs, f)
            .and_then(|d| MiniYaml::from_bytes(&d, f, true).ok())
            .is_some_and(|nodes| flagged(&nodes))
    })
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
