// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Lobby state (`OpenRA.Game/Network/Session.cs`).

use indexmap::{IndexMap, IndexSet};
use rustora_core::field_enum;
use rustora_core::fieldloader::{Color, LoadError, load};
use rustora_core::miniyaml::MiniYaml;
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
