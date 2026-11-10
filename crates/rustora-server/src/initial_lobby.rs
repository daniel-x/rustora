// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The server part of `SkirmishLogic.cs`: the lobby setup that the client of a single player server chose is
//! applied when that client joins. The client reads and writes the skirmish file itself.

use rustora_game::players::sync_client_to_player_reference;
use rustora_net::lobby_setup::{InitialLobby, LobbySetup, SlotSetup};
use rustora_net::session::{Client, ClientState};

use crate::server::Server;

/// `SkirmishSlot.DeserializeToClient`.
fn apply_slot(setup: &SlotSetup, client: &mut Client) {
    client.slot = setup.slot.clone().filter(|s| !s.is_empty());
    if let Some(color) = setup.color {
        client.color = color;
        client.preferred_color = color;
    }
    client.faction = setup.faction.clone().filter(|f| !f.is_empty());
    client.spawn_point = setup.spawn_point;
    client.team = setup.team;
    client.handicap = setup.handicap;
}

impl Server {
    /// `TryInitializeFromFile` with the setup that the client read from the skirmish file.
    fn try_apply_setup(&mut self, setup: &LobbySetup, conn: i32) -> bool {
        if self.lobby.global_settings.map.as_deref() != Some(&setup.map)
            && (!self.game_mod.map_exists(&setup.map) || !self.interpret_command(&format!("map {}", setup.map), conn))
        {
            return false;
        }
        let map = self.map.clone();
        for (id, value) in &setup.options {
            if let Some(option) = map.options.iter().find(|o| &o.id == id)
                && !option.is_locked
                && option.values.contains_key(value)
                && let Some(state) = self.lobby.global_settings.lobby_options.get_mut(&option.id)
            {
                state.value = Some(value.clone());
                state.preferred_value = Some(value.clone());
            }
        }
        let sanitize_faction = |faction: Option<String>| {
            faction
                .filter(|f| map.selectable_factions.contains(f))
                .or_else(|| Some("Random".to_string()))
        };
        if let Some(player) = &setup.player
            && let Some(mut client) = self.lobby.client_with_index(conn).cloned()
        {
            apply_slot(player, &mut client);
            client.color = self.sanitize_player_color(client.color, client.index, None);
            client.faction = sanitize_faction(client.faction);
            if let Some(target) = self.lobby.client_with_index_mut(conn) {
                *target = client;
            }
        }
        let controller = self.lobby.clients.iter().find(|c| c.is_admin).map_or(0, |c| c.index);
        for bot in &setup.bots {
            let Some(info) = map.bots.iter().find(|b| b.bot_type == bot.bot_type) else {
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
            apply_slot(&bot.slot, &mut client);
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
        true
    }

    /// `SkirmishLogic.ClientJoined`: applies the initial lobby of the settings once, when the host joins.
    pub(crate) fn apply_initial_lobby(&mut self, conn: i32) {
        let InitialLobby::Skirmish(setup) = std::mem::take(&mut self.settings.initial_lobby) else {
            return;
        };
        if setup.is_some_and(|s| self.try_apply_setup(&s, conn)) {
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
}
