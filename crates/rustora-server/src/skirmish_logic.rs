// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `SkirmishLogic.cs`: a skirmish restores the last setup from `skirmish.<mod>.yaml` in the support dir.

use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode, nodes_to_string};
use rustora_mods_common::world::sync_client_to_player_reference;
use rustora_net::session::{Client, ClientState, fmt_color};

use crate::server::{Server, ServerType};

/// `SkirmishSlot` as yaml: the slot is the value, the rest are child nodes.
fn slot_yaml(client: &Client) -> MiniYaml {
    let field = |key: &str, value: String| MiniYamlNode::with_value_str(key, value);
    MiniYaml::new(
        client.slot.clone(),
        vec![
            field("Color", fmt_color(client.color)),
            field("Faction", client.faction.clone().unwrap_or_default()),
            field("SpawnPoint", client.spawn_point.to_string()),
            field("Team", client.team.to_string()),
            field("Handicap", client.handicap.to_string()),
        ],
    )
}

/// `SkirmishSlot.DeserializeToClient`.
fn apply_slot(yaml: &MiniYaml, client: &mut Client) {
    let field = |key: &str| yaml.nodes.iter().find(|n| n.key == key).map(|n| n.value.value_str().trim());
    let number = |key: &str| field(key).and_then(|v| v.parse().ok()).unwrap_or(0);
    client.slot = yaml.value.clone().filter(|s| !s.is_empty());
    if let Some(color) = field("Color").and_then(Color::parse) {
        client.color = color;
        client.preferred_color = color;
    }
    client.faction = field("Faction").filter(|f| !f.is_empty()).map(str::to_string);
    client.spawn_point = number("SpawnPoint");
    client.team = number("Team");
    client.handicap = number("Handicap");
}

impl Server {
    fn skirmish_file(&self) -> std::path::PathBuf {
        self.settings
            .support_dir
            .join(format!("skirmish.{}.yaml", self.game_mod.data.manifest.id))
    }

    /// `TryInitializeFromFile`.
    fn try_initialize_skirmish(&mut self, conn: i32) -> bool {
        let Some(nodes) = std::fs::read(self.skirmish_file())
            .ok()
            .and_then(|data| MiniYaml::from_bytes(&data, "skirmish", true).ok())
        else {
            return false;
        };
        let section = |key: &str| nodes.iter().find(|n| n.key == key).map(|n| &n.value);
        let Some(map_uid) = section("Map").map(|m| m.value_str().to_string()) else {
            return false;
        };
        if self.lobby.global_settings.map.as_deref() != Some(&map_uid)
            && (!self.game_mod.map_exists(&map_uid) || !self.interpret_command(&format!("map {map_uid}"), conn))
        {
            return false;
        }
        let map = self.map.clone();
        if let Some(options) = section("Options") {
            for node in &options.nodes {
                let value = node.value.value_str();
                if let Some(option) = map.options.iter().find(|o| o.id == node.key)
                    && !option.is_locked
                    && option.values.contains_key(value)
                    && let Some(state) = self.lobby.global_settings.lobby_options.get_mut(&option.id)
                {
                    state.value = Some(value.to_string());
                    state.preferred_value = Some(value.to_string());
                }
            }
        }
        let sanitize_faction = |faction: Option<String>| {
            faction
                .filter(|f| map.selectable_factions.contains(f))
                .or_else(|| Some("Random".to_string()))
        };
        if let Some(player) = section("Player")
            && let Some(mut client) = self.lobby.client_with_index(conn).cloned()
        {
            apply_slot(player, &mut client);
            client.color = self.sanitize_player_color(client.color, client.index, None);
            client.faction = sanitize_faction(client.faction);
            if let Some(target) = self.lobby.client_with_index_mut(conn) {
                *target = client;
            }
        }
        if let Some(bots) = section("Bots") {
            let controller = self.lobby.clients.iter().find(|c| c.is_admin).map_or(0, |c| c.index);
            for node in &bots.nodes {
                let Some(info) = map.bots.iter().find(|b| b.bot_type == node.key) else {
                    continue;
                };
                let mut client = Client {
                    index: self.choose_free_player_index(),
                    name: Some(info.name.clone()),
                    bot: Some(info.bot_type.clone()),
                    state: ClientState::NotReady,
                    bot_controller_client_index: controller,
                    ..Default::default()
                };
                apply_slot(&node.value, &mut client);
                let slot = client.slot.clone().unwrap_or_default();
                if !self.lobby.slots.get(&slot).is_some_and(|s| s.lock_color) {
                    client.color = self.sanitize_player_color(client.color, client.index, None);
                }
                client.faction = sanitize_faction(client.faction);
                if let Some(reference) = map.players.get(&slot) {
                    sync_client_to_player_reference(&mut client, reference);
                }
                self.lobby.clients.push(client);
            }
        }
        true
    }

    /// `IClientJoined.ClientJoined`.
    pub(crate) fn skirmish_client_joined(&mut self, conn: i32) {
        if self.settings.server_type != ServerType::Skirmish || self.try_initialize_skirmish(conn) {
            return;
        }
        let slot = self
            .lobby
            .slots
            .iter()
            .find(|(key, s)| !s.closed && s.allow_bots && self.lobby.client_in_slot(key).is_none())
            .map(|(key, _)| key.clone());
        let bot = self.map.bots.first().map(|b| b.bot_type.clone());
        let controller = self.lobby.clients.iter().find(|c| c.is_admin).map(|c| c.index);
        if let (Some(slot), Some(bot), Some(controller)) = (slot, bot, controller) {
            self.interpret_command(&format!("slot_bot {slot} {controller} {bot}"), conn);
        }
    }

    /// `INotifySyncLobbyInfo.LobbyInfoSynced`.
    pub(crate) fn skirmish_lobby_info_synced(&self) {
        if self.settings.server_type != ServerType::Skirmish {
            return;
        }
        let Some(player) = self.lobby.non_bot_clients().next() else {
            return;
        };
        let global = &self.lobby.global_settings;
        let options = global
            .lobby_options
            .iter()
            .map(|(k, v)| MiniYamlNode::with_value_str(k.clone(), v.value.clone().unwrap_or_default()))
            .collect();
        let bots = self
            .lobby
            .clients
            .iter()
            .filter(|c| c.is_bot())
            .map(|b| MiniYamlNode::new(b.bot.clone().unwrap_or_default(), slot_yaml(b)))
            .collect();
        let nodes = vec![
            MiniYamlNode::with_value_str("Map", global.map.clone().unwrap_or_default()),
            MiniYamlNode::new("Options", MiniYaml::new(None, options)),
            MiniYamlNode::new("Player", slot_yaml(player)),
            MiniYamlNode::new("Bots", MiniYaml::new(None, bots)),
        ];
        if let Err(e) = std::fs::write(self.skirmish_file(), nodes_to_string(&nodes)) {
            println!("Cannot write the skirmish settings: {e}");
        }
    }
}
