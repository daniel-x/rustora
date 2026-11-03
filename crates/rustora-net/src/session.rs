// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Lobby state (`OpenRA.Game/Network/Session.cs`).

use indexmap::{IndexMap, IndexSet};
use rustora_core::field_enum;
use rustora_core::fieldloader::{Color, LoadError, load};
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode, nodes_to_string};
use rustora_derive::FieldLoad;

use crate::NetError;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClientState {
    NotReady,
    #[default]
    Invalid,
    Ready,
    Disconnected,
}
field_enum!(ClientState, [NotReady, Invalid, Ready, Disconnected]);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionQuality {
    #[default]
    Good,
    Moderate,
    Poor,
}
field_enum!(ConnectionQuality, [Good, Moderate, Poor]);

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct Client {
    pub index: i32,
    pub preferred_color: Color,
    pub color: Color,
    pub faction: Option<String>,
    pub spawn_point: i32,
    pub name: Option<String>,
    #[field(name = "AnonymizedIPAddress")]
    pub anonymized_ip_address: Option<String>,
    pub location: Option<String>,
    pub connection_quality: ConnectionQuality,
    pub state: ClientState,
    pub team: i32,
    pub handicap: i32,
    pub slot: Option<String>,
    pub bot: Option<String>,
    pub bot_controller_client_index: i32,
    pub is_admin: bool,
    pub fingerprint: Option<String>,
}

impl Client {
    pub fn is_observer(&self) -> bool {
        self.slot.is_none()
    }

    pub fn is_bot(&self) -> bool {
        self.bot.is_some()
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct Slot {
    pub player_reference: String,
    pub closed: bool,
    pub allow_bots: bool,
    pub lock_faction: bool,
    pub lock_color: bool,
    pub lock_team: bool,
    pub lock_handicap: bool,
    pub lock_spawn: bool,
    pub required: bool,
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct LobbyOptionState {
    pub value: Option<String>,
    pub preferred_value: Option<String>,
    pub is_locked: bool,
}

impl LobbyOptionState {
    pub fn is_enabled(&self) -> bool {
        self.value.as_deref() == Some("True")
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct Global {
    pub server_name: Option<String>,
    pub map: Option<String>,
    #[field(ignore)]
    pub map_status: i32,
    pub random_seed: i32,
    pub allow_spectators: bool,
    pub game_uid: Option<String>,
    pub enable_singleplayer: bool,
    pub enable_sync_reports: bool,
    pub dedicated: bool,
    pub game_saves_enabled: bool,
    pub net_frame_interval: i32,
    #[field(ignore)]
    pub lobby_options: IndexMap<String, LobbyOptionState>,
}

impl Default for Global {
    fn default() -> Self {
        Global {
            server_name: None,
            map: None,
            map_status: 0,
            random_seed: 0,
            allow_spectators: true,
            game_uid: None,
            enable_singleplayer: false,
            enable_sync_reports: false,
            dedicated: false,
            game_saves_enabled: false,
            net_frame_interval: 3,
            lobby_options: IndexMap::new(),
        }
    }
}

impl Global {
    pub fn option_bool(&self, id: &str, def: bool) -> bool {
        self.lobby_options.get(id).map_or(def, |o| o.is_enabled())
    }

    pub fn option_str<'a>(&'a self, id: &str, def: &'a str) -> &'a str {
        self.lobby_options.get(id).map_or(def, |o| o.value.as_deref().unwrap_or(""))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Session {
    pub clients: Vec<Client>,
    pub slots: IndexMap<String, Slot>,
    pub disabled_spawn_points: IndexSet<i32>,
    pub global_settings: Global,
}

impl Session {
    pub fn deserialize(data: &str, name: &str) -> Result<Session, NetError> {
        let err = |e: LoadError| NetError::Invalid(format!("Session deserialized invalid MiniYaml: {e}"));
        let nodes = MiniYaml::from_str(data, name, true).map_err(|e| err(e.into()))?;
        let mut s = Session::default();
        for n in &nodes {
            match n.key.split('@').next().unwrap_or("") {
                "Client" => s.clients.push(load(&n.value).map_err(err)?),
                "GlobalSettings" => {
                    let mut g: Global = load(&n.value).map_err(err)?;
                    if let Some(o) = n.value.nodes.iter().find(|c| c.key == "Options") {
                        for opt in &o.value.nodes {
                            g.lobby_options.insert(opt.key.clone(), load(&opt.value).map_err(err)?);
                        }
                    }
                    s.global_settings = g;
                }
                "Slot" => {
                    let slot: Slot = load(&n.value).map_err(err)?;
                    s.slots.insert(slot.player_reference.clone(), slot);
                }
                "DisabledSpawnPoints" => {
                    s.disabled_spawn_points =
                        rustora_core::fieldloader::get_value("DisabledSpawnPoints", n.value.value.as_deref()).map_err(err)?;
                }
                _ => {}
            }
        }
        Ok(s)
    }

    pub fn client_with_index(&self, index: i32) -> Option<&Client> {
        self.clients.iter().find(|c| c.index == index)
    }

    pub fn client_in_slot(&self, slot: &str) -> Option<&Client> {
        self.clients.iter().find(|c| c.slot.as_deref() == Some(slot))
    }
}

/// `FieldSaver.FormatValue` of the session field types.
fn fmt_bool(b: bool) -> String {
    if b { "True" } else { "False" }.to_string()
}

fn fmt_color(c: Color) -> String {
    if c.a() == 255 {
        format!("{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
    } else {
        format!("{:02X}{:02X}{:02X}{:02X}", c.r(), c.g(), c.b(), c.a())
    }
}

fn fmt_opt(s: &Option<String>) -> String {
    s.clone().unwrap_or_default()
}

/// The `Session.MapStatus` flags as the C# enum converter writes them.
pub fn fmt_map_status(status: i32) -> String {
    const NAMES: [(i32, &str); 4] = [(1, "Validating"), (2, "Playable"), (4, "Incompatible"), (8, "UnsafeCustomRules")];
    let set: Vec<&str> = NAMES.iter().filter(|(b, _)| status & b != 0).map(|(_, n)| *n).collect();
    if set.is_empty() { "Unknown".to_string() } else { set.join(", ") }
}

fn yaml(fields: Vec<(&str, String)>) -> MiniYaml {
    MiniYaml::new(None, fields.into_iter().map(|(k, v)| MiniYamlNode::with_value_str(k, v)).collect())
}

impl ConnectionQuality {
    pub fn name(self) -> &'static str {
        match self {
            ConnectionQuality::Good => "Good",
            ConnectionQuality::Moderate => "Moderate",
            ConnectionQuality::Poor => "Poor",
        }
    }
}

impl ClientState {
    pub fn name(self) -> &'static str {
        match self {
            ClientState::NotReady => "NotReady",
            ClientState::Invalid => "Invalid",
            ClientState::Ready => "Ready",
            ClientState::Disconnected => "Disconnected",
        }
    }
}

impl Client {
    /// `Session.Client.Serialize`: `Client@<index>` with all fields.
    pub fn serialize(&self) -> MiniYamlNode {
        MiniYamlNode::new(
            format!("Client@{}", self.index),
            yaml(vec![
                ("Index", self.index.to_string()),
                ("PreferredColor", fmt_color(self.preferred_color)),
                ("Color", fmt_color(self.color)),
                ("Faction", fmt_opt(&self.faction)),
                ("SpawnPoint", self.spawn_point.to_string()),
                ("Name", fmt_opt(&self.name)),
                ("AnonymizedIPAddress", fmt_opt(&self.anonymized_ip_address)),
                ("Location", fmt_opt(&self.location)),
                ("ConnectionQuality", self.connection_quality.name().to_string()),
                ("State", self.state.name().to_string()),
                ("Team", self.team.to_string()),
                ("Handicap", self.handicap.to_string()),
                ("Slot", fmt_opt(&self.slot)),
                ("Bot", fmt_opt(&self.bot)),
                ("BotControllerClientIndex", self.bot_controller_client_index.to_string()),
                ("IsAdmin", fmt_bool(self.is_admin)),
                ("Fingerprint", fmt_opt(&self.fingerprint)),
            ]),
        )
    }
}

impl Slot {
    /// `Session.Slot.Serialize`: `Slot@<player reference>`.
    pub fn serialize(&self) -> MiniYamlNode {
        MiniYamlNode::new(
            format!("Slot@{}", self.player_reference),
            yaml(vec![
                ("PlayerReference", self.player_reference.clone()),
                ("Closed", fmt_bool(self.closed)),
                ("AllowBots", fmt_bool(self.allow_bots)),
                ("LockFaction", fmt_bool(self.lock_faction)),
                ("LockColor", fmt_bool(self.lock_color)),
                ("LockTeam", fmt_bool(self.lock_team)),
                ("LockHandicap", fmt_bool(self.lock_handicap)),
                ("LockSpawn", fmt_bool(self.lock_spawn)),
                ("Required", fmt_bool(self.required)),
            ]),
        )
    }
}

impl Global {
    /// `Session.Global.Serialize`: `GlobalSettings` with the lobby options under `Options`.
    pub fn serialize(&self) -> MiniYamlNode {
        let mut y = yaml(vec![
            ("ServerName", fmt_opt(&self.server_name)),
            ("Map", fmt_opt(&self.map)),
            ("MapStatus", fmt_map_status(self.map_status)),
            ("RandomSeed", self.random_seed.to_string()),
            ("AllowSpectators", fmt_bool(self.allow_spectators)),
            ("GameUid", fmt_opt(&self.game_uid)),
            ("EnableSingleplayer", fmt_bool(self.enable_singleplayer)),
            ("EnableSyncReports", fmt_bool(self.enable_sync_reports)),
            ("Dedicated", fmt_bool(self.dedicated)),
            ("GameSavesEnabled", fmt_bool(self.game_saves_enabled)),
            ("NetFrameInterval", self.net_frame_interval.to_string()),
        ]);
        let options = self
            .lobby_options
            .iter()
            .map(|(k, o)| {
                MiniYamlNode::new(
                    k.clone(),
                    yaml(vec![
                        ("Value", fmt_opt(&o.value)),
                        ("PreferredValue", fmt_opt(&o.preferred_value)),
                        ("IsLocked", fmt_bool(o.is_locked)),
                    ]),
                )
            })
            .collect();
        y.nodes.push(MiniYamlNode::new("Options", MiniYaml::new(None, options)));
        MiniYamlNode::new("GlobalSettings", y)
    }
}

impl Session {
    /// `Session.Serialize`, the payload of the `SyncInfo` server order.
    pub fn serialize(&self) -> String {
        let spawns: Vec<String> = self.disabled_spawn_points.iter().map(i32::to_string).collect();
        let mut nodes = vec![MiniYamlNode::with_value_str("DisabledSpawnPoints", spawns.join(", "))];
        nodes.extend(self.clients.iter().map(Client::serialize));
        nodes.extend(self.slots.values().map(Slot::serialize));
        nodes.push(self.global_settings.serialize());
        nodes_to_string(&nodes)
    }
}
