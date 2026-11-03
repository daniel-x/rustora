// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The description of a game in server lists (`OpenRA.Game/Network/GameServer.cs`).

use rustora_core::fieldloader::{Color, load};
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_derive::FieldLoad;

use crate::session::{Client, fmt_color};

/// `GameServer.ProtocolVersion`.
pub const PROTOCOL_VERSION: i32 = 2;

fn fmt_bool(b: bool) -> String {
    if b { "True" } else { "False" }.to_string()
}

fn node(key: &str, value: impl Into<String>) -> MiniYamlNode {
    MiniYamlNode::with_value_str(key, value)
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GameClient {
    pub name: Option<String>,
    pub fingerprint: Option<String>,
    pub color: Color,
    pub faction: Option<String>,
    pub team: i32,
    pub spawn_point: i32,
    pub is_admin: bool,
    pub is_spectator: bool,
    pub is_bot: bool,
}

impl GameClient {
    pub fn from_client(c: &Client) -> GameClient {
        GameClient {
            name: c.name.clone(),
            fingerprint: c.fingerprint.clone(),
            color: c.color,
            faction: c.faction.clone(),
            team: c.team,
            spawn_point: c.spawn_point,
            is_admin: c.is_admin,
            is_spectator: c.slot.is_none() && c.bot.is_none(),
            is_bot: c.bot.is_some(),
        }
    }

    fn save(&self) -> MiniYaml {
        let text = |s: &Option<String>| s.clone().unwrap_or_default();
        MiniYaml::new(
            None,
            vec![
                node("Name", text(&self.name)),
                node("Fingerprint", text(&self.fingerprint)),
                node("Color", fmt_color(self.color)),
                node("Faction", text(&self.faction)),
                node("Team", self.team.to_string()),
                node("SpawnPoint", self.spawn_point.to_string()),
                node("IsAdmin", fmt_bool(self.is_admin)),
                node("IsSpectator", fmt_bool(self.is_spectator)),
                node("IsBot", fmt_bool(self.is_bot)),
            ],
        )
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GameServer {
    /// Online game number, -1 for LAN games. LAN beacons send a GUID, which the receiver replaces.
    #[field(ignore)]
    pub id: i32,
    pub name: Option<String>,
    pub address: Option<String>,
    pub port: i32,
    /// 1 waiting for players, 2 playing, 3 shutting down.
    pub state: i32,
    pub max_players: i32,
    pub map: Option<String>,
    #[field(name = "Mod")]
    pub mod_id: Option<String>,
    pub version: Option<String>,
    pub mod_title: Option<String>,
    pub mod_website: Option<String>,
    pub mod_icon32: Option<String>,
    pub location: Option<String>,
    pub protected: bool,
    pub authentication: bool,
    pub started: Option<String>,
    pub players: i32,
    pub bots: i32,
    pub spectators: i32,
    pub play_time: i32,
    #[field(ignore)]
    pub clients: Vec<GameClient>,
    #[field(ignore)]
    pub disabled_spawn_points: Vec<i32>,
}

impl GameServer {
    /// `GameServer(MiniYaml)`: a game from a server list or a LAN beacon.
    pub fn from_yaml(yaml: &MiniYaml) -> Result<GameServer, String> {
        let mut game: GameServer = load(yaml).map_err(|e| e.to_string())?;
        let value = |key: &str| yaml.nodes.iter().find(|n| n.key == key).map(|n| n.value.clone());
        game.id = value("Id").and_then(|v| v.value_str().trim().parse().ok()).unwrap_or(-1);
        if game.mod_id.is_none() || game.version.is_none() {
            let mods = value("Mods");
            if let Some((m, v)) = mods.as_ref().and_then(|m| m.value_str().split_once('@')) {
                game.mod_id = Some(m.to_string());
                game.version = Some(v.to_string());
            }
        }
        if let Some(clients) = value("Clients") {
            for n in clients.nodes.iter().filter(|n| n.key.starts_with("Client@")) {
                game.clients.push(load(&n.value).map_err(|e| e.to_string())?);
            }
        }
        if let Some(spawns) = value("DisabledSpawnPoints") {
            game.disabled_spawn_points = spawns.value_str().split(',').filter_map(|s| s.trim().parse().ok()).collect();
        }
        Ok(game)
    }

    /// `ToPOSTData`: what a server advertises, online or in its LAN beacon with `lan_guid`.
    pub fn to_post_data(&self, lan_guid: Option<&str>) -> String {
        let text = |s: &Option<String>| s.clone().unwrap_or_default();
        let spawns: Vec<String> = self.disabled_spawn_points.iter().map(i32::to_string).collect();
        let mut root = vec![
            node("Protocol", PROTOCOL_VERSION.to_string()),
            node("Name", text(&self.name)),
            node("Address", text(&self.address)),
            node("Mod", text(&self.mod_id)),
            node("Version", text(&self.version)),
            node("ModTitle", text(&self.mod_title)),
            node("ModWebsite", text(&self.mod_website)),
            node("ModIcon32", text(&self.mod_icon32)),
            node("Map", text(&self.map)),
            node("State", self.state.to_string()),
            node("MaxPlayers", self.max_players.to_string()),
            node("Protected", fmt_bool(self.protected)),
            node("Authentication", fmt_bool(self.authentication)),
            node("DisabledSpawnPoints", spawns.join(", ")),
        ];
        if let Some(guid) = lan_guid {
            let count = |f: &dyn Fn(&GameClient) -> bool| self.clients.iter().filter(|c| f(c)).count().to_string();
            root.push(node("Id", guid));
            root.push(node("Players", count(&|c| !c.is_bot && !c.is_spectator)));
            root.push(node("Spectators", count(&|c| c.is_spectator)));
            root.push(node("Bots", count(&|c| c.is_bot)));
            root.push(node("Mods", format!("{}@{}", text(&self.mod_id), text(&self.version))));
        }
        let clients = self
            .clients
            .iter()
            .enumerate()
            .map(|(i, c)| MiniYamlNode::new(format!("Client@{i}"), c.save()))
            .collect();
        root.push(MiniYamlNode::new("Clients", MiniYaml::new(None, clients)));
        let mut lines = Vec::new();
        MiniYaml::new(None, root).to_lines("Game", None, &mut lines);
        lines.join("\n")
    }
}

/// Parses the answer of the master server; invalid entries are skipped like in `ServerListLogic`.
pub fn parse_game_list(text: &str) -> Result<Vec<GameServer>, String> {
    let nodes = MiniYaml::from_str(text, "server list", true).map_err(|e| e.to_string())?;
    Ok(nodes
        .iter()
        .filter_map(|n| GameServer::from_yaml(&n.value).ok())
        .filter(|g| g.address.is_some())
        .collect())
}

/// `ServerListLogic`: a game from a LAN beacon's data with the address of the host it answered from.
pub fn parse_lan_game(data: &str, host: std::net::IpAddr) -> Option<GameServer> {
    let nodes = MiniYaml::from_str(data, "LAN beacon", true).ok()?;
    let mut game = GameServer::from_yaml(&nodes.first()?.value).ok()?;
    game.id = -1;
    let port = game.address.as_deref()?.rsplit_once(':')?.1.to_string();
    game.address = Some(format!("{host}:{port}"));
    game.location = Some("Local Network".to_string());
    Some(game)
}

/// The query URL of `ServerListLogic.RefreshServerList`.
pub fn server_list_url(base: &str, engine_version: &str, mod_id: &str, mod_version: &str) -> String {
    format!("{base}?protocol={PROTOCOL_VERSION}&engine={engine_version}&mod={mod_id}&version={mod_version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> GameServer {
        GameServer {
            name: Some("Test".into()),
            address: Some("0.0.0.0:1234".into()),
            state: 1,
            max_players: 2,
            map: Some("abc".into()),
            mod_id: Some("ra".into()),
            version: Some("release-20250330".into()),
            protected: true,
            clients: vec![
                GameClient {
                    name: Some("Admin".into()),
                    is_admin: true,
                    is_spectator: true,
                    ..Default::default()
                },
                GameClient {
                    name: Some("Bot".into()),
                    is_bot: true,
                    ..Default::default()
                },
            ],
            disabled_spawn_points: vec![2, 3],
            ..Default::default()
        }
    }

    #[test]
    fn lan_data_round_trip() {
        let data = game().to_post_data(Some("guid"));
        let back = parse_lan_game(&data, "192.168.1.5".parse().unwrap()).unwrap();
        assert_eq!(back.address.as_deref(), Some("192.168.1.5:1234"));
        assert_eq!(back.id, -1);
        assert_eq!((back.spectators, back.bots, back.players), (1, 1, 0));
        assert_eq!(back.clients.len(), 2);
        assert!(back.clients[0].is_admin);
        assert_eq!(back.disabled_spawn_points, [2, 3]);
        assert!(back.protected);
        assert_eq!(back.location.as_deref(), Some("Local Network"));
    }

    #[test]
    fn online_data_leaves_out_the_lan_fields() {
        let data = game().to_post_data(None);
        assert!(data.starts_with("Game:\n\tProtocol: 2\n\tName: Test\n"));
        assert!(!data.contains("Mods:"));
        assert!(!data.contains("\tId:"));
    }

    #[test]
    fn game_list_skips_invalid_games_and_reads_old_mods_field() {
        let text = "Game@1:\n\tId: 7\n\tAddress: 1.2.3.4:1234\n\tMods: ra@release-1\nGame@2:\n\tName: no address\n";
        let list = parse_game_list(text).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, 7);
        assert_eq!(list[0].mod_id.as_deref(), Some("ra"));
        assert_eq!(list[0].version.as_deref(), Some("release-1"));
    }
}
