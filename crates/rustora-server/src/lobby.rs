// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Lobby commands of connected clients (`OpenRA.Mods.Common/ServerTraits/LobbyCommands.cs`), and what the
//! `LobbyCommands` and `LobbySettingsNotification` traits do when a client joins or the server becomes empty.

use rustora_core::fieldloader::Color;
use rustora_mods_common::world::{make_slot, sync_client_to_player_reference};
use rustora_net::session::{Client, ClientState, LobbyOptionState, Session};

use crate::server::{Server, ServerState, WHITE, sanitized_player_name};

fn parse_int(s: &str) -> Option<i32> {
    s.trim().parse().ok()
}

/// `bool.TryParse`.
fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_state(s: &str) -> Option<ClientState> {
    match s {
        "NotReady" | "0" => Some(ClientState::NotReady),
        "Invalid" | "1" => Some(ClientState::Invalid),
        "Ready" | "2" => Some(ClientState::Ready),
        "Disconnected" | "1000" => Some(ClientState::Disconnected),
        _ => None,
    }
}

impl Server {
    /// `InterpretCommand`: false if `command` is unknown or invalid for the client `conn`.
    pub fn interpret_command(&mut self, command: &str, conn: i32) -> bool {
        let Some(client) = self.lobby.client_with_index(conn).cloned() else {
            return false;
        };
        if !self.validate_command(conn, &client, command) {
            return false;
        }
        let (name, value) = command.split_once(' ').unwrap_or((command, ""));
        match name {
            "state" => self.cmd_state(conn, value),
            "startgame" => self.cmd_start_game(conn, &client),
            "slot" => self.cmd_slot(conn, &client, value),
            "allow_spectators" => self.cmd_allow_spectators(conn, value),
            "spectate" => self.cmd_spectate(&client),
            "slot_close" => self.cmd_slot_close(conn, &client, value),
            "slot_open" => self.cmd_slot_open(conn, &client, value),
            "slot_bot" => self.cmd_slot_bot(conn, &client, value),
            "map" => self.cmd_map(conn, &client, value),
            "option" => self.cmd_option(conn, &client, value),
            "reset_options" => self.cmd_reset_options(conn, &client),
            "assignteams" => self.cmd_assign_teams(conn, &client, value),
            "kick" => self.cmd_kick(conn, &client, value),
            "vote_kick" => self.cmd_vote_kick(conn, &client, value),
            "make_admin" => self.cmd_make_admin(conn, &client, value),
            "make_spectator" => self.cmd_make_spectator(conn, &client, value),
            "name" => self.cmd_name(&client, value),
            "faction" => self.cmd_faction(conn, &client, value),
            "team" => self.cmd_team(&client, value),
            "handicap" => self.cmd_handicap(&client, value),
            "spawn" => self.cmd_spawn(conn, &client, value),
            "clear_spawn" => self.cmd_clear_spawn(conn, &client, value),
            "color" => self.cmd_color(conn, &client, value),
            "sync_lobby" => self.cmd_sync_lobby(conn, &client, value),
            _ => false,
        }
    }

    fn validate_command(&mut self, conn: i32, client: &Client, command: &str) -> bool {
        if command.starts_with("kick ") || command.starts_with("vote_kick ") {
            return true;
        }
        if self.state == ServerState::GameStarted {
            self.send_fluent_message_to(conn, "notification-state-unchanged-game-started", &[("command", command.into())]);
            return false;
        }
        if client.state == ClientState::Ready && !(command.starts_with("state") || command == "startgame") {
            self.send_fluent_message_to(conn, "notification-state-unchanged-ready", &[]);
            return false;
        }
        true
    }

    fn validate_slot_command(&mut self, conn: i32, client: &Client, slot: &str) -> bool {
        if !self.lobby.slots.contains_key(slot) {
            println!("Invalid slot: {slot}");
            return false;
        }
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-requires-host", &[]);
            return false;
        }
        true
    }

    fn malformed(&mut self, conn: i32, command: &str) -> bool {
        self.send_fluent_message_to(conn, "notification-malformed-command", &[("command", command.into())]);
        true
    }

    fn required_slot_empty(&self) -> bool {
        self.lobby
            .slots
            .iter()
            .any(|(k, s)| s.required && self.lobby.client_in_slot(k).is_none())
    }

    fn no_players(&self) -> bool {
        self.lobby.slots.keys().all(|k| self.lobby.client_in_slot(k).is_none())
    }

    fn check_auto_start(&mut self) {
        let lobby = &self.lobby;
        if lobby.non_bot_players().any(|c| c.state != ClientState::Ready)
            || lobby
                .clients
                .iter()
                .find(|c| c.is_admin)
                .is_none_or(|c| c.state != ClientState::Ready)
            || (!lobby.global_settings.enable_singleplayer && lobby.non_bot_players().count() < 2)
            || self.required_slot_empty()
            || self.no_players()
            || self.map.insufficient_enabled_spawn_points(lobby)
        {
            return;
        }
        self.start_game();
    }

    fn cmd_state(&mut self, conn: i32, s: &str) -> bool {
        let Some(state) = parse_state(s) else {
            return self.malformed(conn, "state");
        };
        if let Some(c) = self.lobby.client_with_index_mut(conn) {
            c.state = state;
        }
        self.sync_lobby_clients();
        self.check_auto_start();
        true
    }

    fn cmd_start_game(&mut self, conn: i32, client: &Client) -> bool {
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-start-game", &[]);
        } else if self.required_slot_empty() {
            self.send_fluent_message_to(conn, "notification-no-start-until-required-slots-full", &[]);
        } else if self.no_players() {
            self.send_order_to(conn, "Message", "notification-no-start-without-players");
        } else if !self.lobby.global_settings.enable_singleplayer && self.lobby.non_bot_players().count() < 2 {
            self.send_fluent_message_to(conn, "notification-two-humans-required", &[]);
        } else if self.map.insufficient_enabled_spawn_points(&self.lobby) {
            self.send_fluent_message_to(conn, "notification-insufficient-enabled-spawn-points", &[]);
        } else {
            self.start_game();
        }
        true
    }

    fn cmd_slot(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        let Some(slot) = self.lobby.slots.get(s) else {
            println!("Invalid slot: {s}");
            return false;
        };
        if slot.closed || self.lobby.client_in_slot(s).is_some() {
            return false;
        }
        let lock_color = slot.lock_color;
        let old_slot_locks_spawn = client
            .slot
            .as_ref()
            .and_then(|o| self.lobby.slots.get(o))
            .is_some_and(|o| o.lock_spawn);
        let map = self.map.clone();
        if let Some(c) = self.lobby.client_with_index_mut(client.index) {
            if old_slot_locks_spawn {
                c.spawn_point = 0;
            }
            c.slot = Some(s.to_string());
            if let Some(pr) = map.players.get(s) {
                sync_client_to_player_reference(c, pr);
            }
        }
        if !lock_color {
            let asked = self.lobby.client_with_index(client.index).map_or(client.color, |c| c.color);
            let color = self.sanitize_player_color(asked, client.index, Some(conn));
            if let Some(c) = self.lobby.client_with_index_mut(client.index) {
                c.preferred_color = color;
                c.color = color;
            }
        }
        self.sync_lobby_clients();
        self.check_auto_start();
        true
    }

    fn cmd_allow_spectators(&mut self, conn: i32, s: &str) -> bool {
        match parse_bool(s) {
            Some(v) => {
                self.lobby.global_settings.allow_spectators = v;
                self.sync_lobby_global_settings();
                true
            }
            None => self.malformed(conn, "allow_spectate"),
        }
    }

    fn make_spectator(c: &mut Client) {
        c.slot = None;
        c.spawn_point = 0;
        c.team = 0;
        c.handicap = 0;
        c.color = WHITE;
    }

    fn cmd_spectate(&mut self, client: &Client) -> bool {
        if !self.lobby.global_settings.allow_spectators && !client.is_admin {
            return false;
        }
        if let Some(c) = self.lobby.client_with_index_mut(client.index) {
            Self::make_spectator(c);
        }
        self.sync_lobby_clients();
        self.check_auto_start();
        true
    }

    fn cmd_slot_close(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        if !self.validate_slot_command(conn, client, s) {
            return false;
        }
        if let Some(occupant) = self.lobby.client_in_slot(s).cloned() {
            if occupant.is_bot() {
                self.lobby.clients.retain(|c| c.index != occupant.index);
                self.sync_lobby_clients();
            } else if self.conn(occupant.index).is_some() {
                self.send_order_to(conn, "ServerError", "notification-slot-closed");
                self.drop_client(occupant.index);
            }
        }
        if let Some(slot) = self.lobby.slots.get_mut(s) {
            slot.closed = true;
        }
        self.sync_lobby_slots();
        true
    }

    fn cmd_slot_open(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        if !self.validate_slot_command(conn, client, s) {
            return false;
        }
        if let Some(slot) = self.lobby.slots.get_mut(s) {
            slot.closed = false;
        }
        self.sync_lobby_slots();
        if let Some(index) = self.lobby.client_in_slot(s).filter(|c| c.is_bot()).map(|c| c.index) {
            self.lobby.clients.retain(|c| c.index != index);
        }
        self.sync_lobby_clients();
        true
    }

    fn cmd_slot_bot(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        let parts: Vec<&str> = s.split(' ').collect();
        if parts.len() < 3 {
            return self.malformed(conn, "slot_bot");
        }
        let (slot_name, bot_type) = (parts[0], parts[2]);
        if !self.validate_slot_command(conn, client, slot_name) {
            return false;
        }
        let Some(controller) = parse_int(parts[1]) else {
            println!("Invalid bot controller client index: {}", parts[1]);
            return false;
        };
        let existing = self.lobby.client_in_slot(slot_name).cloned();
        if existing.as_ref().is_some_and(|b| !b.is_bot()) {
            self.send_fluent_message_to(conn, "notification-invalid-bot-slot", &[]);
            return true;
        }
        let map = self.map.clone();
        let Some(bot_info) = map.bots.iter().find(|b| b.bot_type == bot_type) else {
            self.send_fluent_message_to(conn, "notification-invalid-bot-type", &[]);
            return true;
        };
        if let Some(slot) = self.lobby.slots.get_mut(slot_name) {
            slot.closed = false;
        }
        let index = match existing {
            None => {
                let index = self.choose_free_player_index();
                let players = self.player_colors(None);
                let color = map
                    .color_picker
                    .random_preset_color(&mut self.random, &map.restricted_colors, &players);
                self.lobby.clients.push(Client {
                    index,
                    name: Some(bot_info.name.clone()),
                    bot: Some(bot_type.to_string()),
                    slot: Some(slot_name.to_string()),
                    faction: Some("Random".to_string()),
                    state: ClientState::NotReady,
                    bot_controller_client_index: controller,
                    color,
                    preferred_color: color,
                    ..Default::default()
                });
                index
            }
            Some(b) => {
                if let Some(c) = self.lobby.client_with_index_mut(b.index) {
                    c.name = Some(bot_info.name.clone());
                    c.bot = Some(bot_type.to_string());
                }
                b.index
            }
        };
        if let (Some(c), Some(pr)) = (self.lobby.client_with_index_mut(index), map.players.get(slot_name)) {
            sync_client_to_player_reference(c, pr);
        }
        self.sync_lobby_clients();
        self.sync_lobby_slots();
        true
    }

    fn cmd_map(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-change-map", &[]);
            return true;
        }
        if !self.map_pool.contains(s) || !self.game_mod.map_exists(s) {
            self.send_fluent_message_to(conn, "notification-unknown-map", &[]);
            return true;
        }
        match self.game_mod.map_info(s) {
            Ok(map) => self.select_map(conn, client, map),
            Err(e) => {
                println!("Failed to load map {s}: {e}");
                self.send_fluent_message_to(conn, "notification-unknown-map", &[]);
            }
        }
        true
    }

    fn select_map(&mut self, conn: i32, client: &Client, map: std::rc::Rc<crate::mod_info::MapInfo>) {
        let old_slots: Vec<String> = self.lobby.slots.keys().cloned().collect();
        self.map = map.clone();
        self.lobby.global_settings.map = Some(map.uid.clone());
        self.lobby.global_settings.map_status = map.status;
        self.reset_slots();
        map.load_map_settings(&mut self.lobby);

        for c in &mut self.lobby.clients {
            let valid = c.faction.as_ref().is_some_and(|f| map.selectable_factions.contains(f));
            if !valid {
                c.faction = Some("Random".to_string());
            }
            c.state = ClientState::Invalid;
        }

        let slots: Vec<String> = self.lobby.slots.keys().cloned().collect();
        let mut next_slot = slots.iter();
        for old in &old_slots {
            let Some(index) = self.lobby.client_in_slot(old).map(|c| c.index) else {
                continue;
            };
            let new_slot = next_slot.next().cloned();
            let Some(c) = self.lobby.client_with_index_mut(index) else {
                continue;
            };
            c.spawn_point = 0;
            c.slot = new_slot.clone();
            let bot = c.bot.clone();
            match new_slot.as_ref().and_then(|n| map.players.get(n)) {
                Some(pr) => {
                    sync_client_to_player_reference(c, pr);
                    let allowed = pr.allow_bots && map.bots.iter().any(|b| Some(&b.bot_type) == bot.as_ref());
                    if bot.is_some() && !allowed {
                        self.lobby.clients.retain(|x| x.index != index);
                    }
                }
                None if bot.is_some() => self.lobby.clients.retain(|x| x.index != index),
                None => c.color = WHITE,
            }
        }

        let color_clients: Vec<(i32, Color)> = self
            .lobby
            .clients
            .iter()
            .filter(|c| c.slot.as_ref().and_then(|s| self.lobby.slots.get(s)).is_some_and(|s| !s.lock_color))
            .map(|c| (c.index, c.color))
            .collect();
        for (index, asked) in color_clients {
            let color = self.sanitize_player_color(asked, index, Some(conn));
            if let Some(c) = self.lobby.client_with_index_mut(index) {
                c.color = color;
                c.preferred_color = color;
            }
        }

        self.lobby.disabled_spawn_points.clear();
        self.sync_lobby_info();
        let admin = client.name.clone().unwrap_or_default();
        self.send_fluent_message(
            "notification-changed-map",
            &[("player", (&admin).into()), ("map", (&map.title).into())],
        );
        if self.lobby.global_settings.map_status & crate::mod_info::MAP_STATUS_UNSAFE_CUSTOM_RULES != 0 {
            self.send_fluent_message("notification-custom-rules", &[]);
        }
        if !self.lobby.global_settings.enable_singleplayer {
            self.send_fluent_message("notification-two-humans-required", &[]);
        } else if map.players.values().filter(|p| p.playable).all(|p| !p.allow_bots) {
            self.send_fluent_message("notification-map-bots-disabled", &[]);
        }
        if let Some(briefing) = &map.briefing {
            self.send_message(briefing);
        }
    }

    fn reset_slots(&mut self) {
        self.lobby.slots = self
            .map
            .players
            .values()
            .filter_map(make_slot)
            .map(|s| (s.player_reference.clone(), s))
            .collect();
    }

    fn set_all_not_ready(&mut self) {
        for c in &mut self.lobby.clients {
            c.state = ClientState::NotReady;
        }
    }

    fn cmd_option(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-change-configuration", &[]);
            return true;
        }
        let map = self.map.clone();
        let split: Vec<&str> = s.split(' ').collect();
        let option = split.first().and_then(|id| map.options.iter().rev().find(|o| o.id == *id));
        let (Some(option), Some(value)) = (option, split.get(1).filter(|_| split.len() >= 2)) else {
            self.send_fluent_message_to(conn, "notification-invalid-configuration-command", &[]);
            return true;
        };
        if !option.values.contains_key(*value) {
            self.send_fluent_message_to(conn, "notification-invalid-configuration-command", &[]);
            return true;
        }
        if option.is_locked {
            self.send_fluent_message_to(conn, "notification-option-locked", &[("option", (&option.name).into())]);
            return true;
        }
        let Some(state) = self.lobby.global_settings.lobby_options.get_mut(&option.id) else {
            return true;
        };
        if state.value.as_deref() == Some(*value) {
            return true;
        }
        state.value = Some(value.to_string());
        state.preferred_value = Some(value.to_string());
        self.sync_lobby_global_settings();
        let player = client.name.clone().unwrap_or_default();
        self.send_fluent_message(
            "notification-option-changed",
            &[
                ("player", (&player).into()),
                ("name", (&option.name).into()),
                ("value", option.label(value).as_str().into()),
            ],
        );
        self.set_all_not_ready();
        self.sync_lobby_clients();
        true
    }

    fn cmd_reset_options(&mut self, conn: i32, client: &Client) -> bool {
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-change-configuration", &[]);
            return true;
        }
        let map = self.map.clone();
        let player = client.name.clone().unwrap_or_default();
        let mut options = indexmap::IndexMap::new();
        for o in &map.options {
            let current = self.lobby.global_settings.lobby_options.get(&o.id).and_then(|s| s.value.clone());
            if current.as_deref() != Some(o.default_value.as_str()) {
                self.send_fluent_message(
                    "notification-option-changed",
                    &[
                        ("player", (&player).into()),
                        ("name", (&o.name).into()),
                        ("value", o.label(&o.default_value).as_str().into()),
                    ],
                );
            }
            options.insert(
                o.id.clone(),
                LobbyOptionState {
                    value: Some(o.default_value.clone()),
                    preferred_value: Some(o.default_value.clone()),
                    is_locked: o.is_locked,
                },
            );
        }
        self.lobby.global_settings.lobby_options = options;
        self.sync_lobby_global_settings();
        self.set_all_not_ready();
        self.sync_lobby_clients();
        true
    }

    fn cmd_assign_teams(&mut self, conn: i32, client: &Client, raw: &str) -> bool {
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-option", &[]);
            return true;
        }
        let Some(team_count) = parse_int(raw) else {
            self.send_fluent_message_to(conn, "notification-error-number-teams", &[("raw", raw.into())]);
            return true;
        };
        let max_teams = (self.lobby.clients.iter().filter(|c| c.slot.is_some()).count() as i32 + 1) / 2;
        let team_count = team_count.clamp(0, max_teams);
        let indices: Vec<i32> = self
            .lobby
            .slots
            .iter()
            .filter(|(_, s)| !s.lock_team)
            .filter_map(|(k, _)| self.lobby.client_in_slot(k).map(|c| c.index))
            .collect();
        let client_count = indices.len() as i32;
        for (assigned, index) in indices.into_iter().enumerate() {
            if let Some(c) = self.lobby.client_with_index_mut(index) {
                c.team = match team_count {
                    0 => 0,
                    1 => {
                        if c.is_bot() {
                            2
                        } else {
                            1
                        }
                    }
                    _ => assigned as i32 * team_count / client_count + 1,
                };
            }
        }
        self.sync_lobby_clients();
        true
    }

    /// The connected client with the index in `s`.
    fn connected_client(&self, s: &str) -> Option<Client> {
        let index = parse_int(s)?;
        self.conn(index)?;
        self.lobby.client_with_index(index).cloned()
    }

    fn cmd_kick(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-kick", &[]);
            return true;
        }
        let split: Vec<&str> = s.split(' ').collect();
        if split.len() < 2 {
            return self.malformed(conn, "kick");
        }
        let Some(target) = self.connected_client(split[0]) else {
            self.send_fluent_message_to(conn, "notification-kick-none", &[]);
            return true;
        };
        if target.index == client.index {
            self.send_fluent_message_to(conn, "notification-kick-self", &[]);
            return true;
        }
        if self.state == ServerState::GameStarted && !target.is_observer() && !self.has_client_won_or_lost(target.index) {
            self.send_fluent_message_to(conn, "notification-no-kick-game-started", &[]);
            return true;
        }
        let admin = client.name.clone().unwrap_or_default();
        let player = target.name.clone().unwrap_or_default();
        let address = self.conn(target.index).map(|c| c.end_point.ip().to_canonical().to_string());
        self.send_fluent_message(
            "notification-admin-kicked",
            &[("admin", (&admin).into()), ("player", (&player).into())],
        );
        self.send_order_to(target.index, "ServerError", "notification-you-were-kicked");
        self.drop_client(target.index);
        if parse_bool(split[1]) == Some(true) {
            self.send_fluent_message("notification-temp-ban", &[("admin", (&admin).into()), ("player", (&player).into())]);
            self.temp_bans.extend(address);
        }
        self.sync_lobby_clients();
        self.sync_lobby_slots();
        true
    }

    fn cmd_vote_kick(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        let split: Vec<&str> = s.split(' ').collect();
        if split.len() != 2 {
            return self.malformed(conn, "vote_kick");
        }
        if !self.settings.enable_vote_kick {
            self.send_fluent_message_to(conn, "notification-vote-kick-disabled", &[]);
            return true;
        }
        let Some(target) = self.connected_client(split[0]) else {
            self.send_fluent_message_to(conn, "notification-kick-none", &[]);
            return true;
        };
        if target.index == client.index {
            self.send_fluent_message_to(conn, "notification-kick-self", &[]);
            return true;
        }
        let Some(vote) = parse_bool(split[1]) else {
            return self.malformed(conn, "vote_kick");
        };
        if self.vote_kick(conn, client, &target, vote) {
            let name = target.name.clone().unwrap_or_default();
            self.send_fluent_message("notification-kicked", &[("player", (&name).into())]);
            self.send_order_to(target.index, "ServerError", "notification-you-were-kicked");
            self.drop_client(target.index);
            self.sync_lobby_clients();
            self.sync_lobby_slots();
        }
        true
    }

    fn cmd_make_admin(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-transfer-admin", &[]);
            return true;
        }
        let Some(target) = self.connected_client(s) else {
            self.send_fluent_message_to(conn, "notification-empty-slot", &[]);
            return true;
        };
        if let Some(c) = self.lobby.client_with_index_mut(client.index) {
            c.is_admin = false;
        }
        if let Some(c) = self.lobby.client_with_index_mut(target.index) {
            c.is_admin = true;
        }
        let slots: Vec<String> = self.lobby.slots.keys().cloned().collect();
        for slot in slots {
            if let Some(b) = self.lobby.clients.iter_mut().find(|c| c.slot.as_ref() == Some(&slot) && c.is_bot()) {
                b.bot_controller_client_index = target.index;
            }
        }
        let name = target.name.clone().unwrap_or_default();
        self.send_fluent_message("notification-new-admin", &[("player", (&name).into())]);
        self.sync_lobby_clients();
        true
    }

    fn cmd_make_spectator(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-move-spectators", &[]);
            return true;
        }
        let Some(target) = self.connected_client(s) else {
            self.send_fluent_message_to(conn, "notification-empty-slot", &[]);
            return true;
        };
        if let Some(c) = self.lobby.client_with_index_mut(target.index) {
            Self::make_spectator(c);
            c.state = ClientState::NotReady;
        }
        let admin = client.name.clone().unwrap_or_default();
        let player = target.name.clone().unwrap_or_default();
        self.send_fluent_message(
            "notification-move-spectators",
            &[("admin", (&admin).into()), ("player", (&player).into())],
        );
        self.sync_lobby_clients();
        self.check_auto_start();
        true
    }

    fn cmd_name(&mut self, client: &Client, s: &str) -> bool {
        let clean = sanitized_player_name(s);
        if client.name.as_deref() == Some(clean.as_str()) {
            return true;
        }
        let old = client.name.clone().unwrap_or_default();
        self.send_fluent_message("notification-nick-changed", &[("player", (&old).into()), ("name", (&clean).into())]);
        if let Some(c) = self.lobby.client_with_index_mut(client.index) {
            c.name = Some(clean);
        }
        self.sync_lobby_clients();
        true
    }

    /// The client a host or the client itself may change, with the second argument of the command.
    /// C# throws on malformed arguments, which ends the server thread; this ignores them instead.
    fn changeable_target<'a>(&self, client: &Client, s: &'a str) -> Option<(Client, &'a str)> {
        let (index, value) = s.split_once(' ')?;
        let target = self.lobby.client_with_index(parse_int(index)?)?;
        (target.index == client.index || client.is_admin).then(|| (target.clone(), value.split(' ').next().unwrap_or("")))
    }

    fn target_slot(&self, target: &Client) -> Option<&rustora_net::session::Slot> {
        self.lobby.slots.get(target.slot.as_ref()?)
    }

    /// Checks the faction of the issuing client like C# does, not the requested one.
    fn cmd_faction(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        let Some((target, faction)) = self.changeable_target(client, s) else {
            return true;
        };
        if self.target_slot(&target).is_none_or(|t| t.lock_faction) {
            return true;
        }
        let issuer_valid = client.faction.as_ref().is_some_and(|f| self.map.selectable_factions.contains(f));
        if !issuer_valid {
            self.send_fluent_message_to(conn, "notification-invalid-faction-selected", &[("faction", faction.into())]);
            return true;
        }
        if let Some(c) = self.lobby.client_with_index_mut(target.index) {
            c.faction = Some(faction.to_string());
        }
        self.sync_lobby_clients();
        true
    }

    fn cmd_team(&mut self, client: &Client, s: &str) -> bool {
        let Some((target, team)) = self.changeable_target(client, s) else {
            return true;
        };
        if self.target_slot(&target).is_none_or(|t| t.lock_team) {
            return true;
        }
        let Some(team) = parse_int(team) else {
            println!("Invalid team: {s}");
            return false;
        };
        if let Some(c) = self.lobby.client_with_index_mut(target.index) {
            c.team = team;
        }
        self.sync_lobby_clients();
        true
    }

    fn cmd_handicap(&mut self, client: &Client, s: &str) -> bool {
        let Some((target, handicap)) = self.changeable_target(client, s) else {
            return true;
        };
        if self.target_slot(&target).is_none_or(|t| t.lock_handicap) {
            return true;
        }
        let Some(handicap) = parse_int(handicap).filter(|h| (0..100).contains(h) && h % 5 == 0) else {
            println!("Invalid handicap: {s}");
            return false;
        };
        if let Some(c) = self.lobby.client_with_index_mut(target.index) {
            c.handicap = handicap;
        }
        self.sync_lobby_clients();
        true
    }

    fn cmd_clear_spawn(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        let spawn = match parse_int(s) {
            Some(0) | None => return true,
            Some(v) => v,
        };
        let existing = self.lobby.clients.iter().find(|c| c.spawn_point == spawn).cloned();
        if existing.as_ref().is_none_or(|e| e.index != client.index) && !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-clear-spawn", &[]);
            return true;
        }
        if let Some(e) = existing {
            if self.target_slot(&e).is_some_and(|t| t.lock_spawn) {
                return true;
            }
            if let Some(c) = self.lobby.client_with_index_mut(e.index) {
                c.spawn_point = 0;
                if c.state == ClientState::Ready {
                    c.state = ClientState::NotReady;
                }
            }
            self.sync_lobby_clients();
            return true;
        }
        if !self.lobby.disabled_spawn_points.insert(spawn) {
            self.lobby.disabled_spawn_points.shift_remove(&spawn);
        }
        self.sync_lobby_info();
        true
    }

    fn cmd_spawn(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        let Some((target, spawn)) = self.changeable_target(client, s) else {
            return true;
        };
        if self.target_slot(&target).is_none_or(|t| t.lock_spawn) {
            return true;
        }
        let Some(spawn) = parse_int(spawn).filter(|v| *v >= 0 && *v <= self.map.spawn_points as i32) else {
            println!("Invalid spawn point: {spawn}");
            return true;
        };
        if self
            .lobby
            .clients
            .iter()
            .any(|c| c.index != client.index && c.spawn_point == spawn && c.spawn_point != 0)
        {
            self.send_fluent_message_to(conn, "notification-spawn-occupied", &[]);
            return true;
        }
        let locked = spawn > 0
            && self
                .lobby
                .slots
                .values()
                .filter(|slot| slot.lock_spawn)
                .any(|slot| self.map.players.get(&slot.player_reference).is_some_and(|pr| pr.spawn == spawn));
        if locked {
            self.send_fluent_message_to(conn, "notification-spawn-locked", &[]);
            return true;
        }
        if let Some(c) = self.lobby.client_with_index_mut(target.index) {
            c.spawn_point = spawn;
        }
        self.sync_lobby_clients();
        true
    }

    fn cmd_color(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        let Some((target, color)) = self.changeable_target(client, s) else {
            return true;
        };
        if self.target_slot(&target).is_none_or(|t| t.lock_color) {
            return true;
        }
        let Some(asked) = Color::parse(color) else {
            return true;
        };
        let color = self.sanitize_player_color(asked, target.index, Some(conn));
        if let Some(c) = self.lobby.client_with_index_mut(target.index) {
            c.color = color;
            if asked == color {
                c.preferred_color = color;
            }
        }
        self.sync_lobby_clients();
        true
    }

    fn cmd_sync_lobby(&mut self, conn: i32, client: &Client, s: &str) -> bool {
        if !client.is_admin {
            self.send_fluent_message_to(conn, "notification-admin-lobby-info", &[]);
            return true;
        }
        match Session::deserialize(s, "SyncLobby") {
            Ok(lobby) => {
                self.lobby = lobby;
                self.sync_lobby_info();
            }
            Err(_) => self.send_fluent_message_to(conn, "notification-invalid-lobby-info", &[]),
        }
        true
    }

    /// The colors other players use or the map reserves; `except` is the client asking for a color.
    fn player_colors(&self, except: Option<i32>) -> Vec<Color> {
        self.lobby
            .clients
            .iter()
            .filter(|c| Some(c.index) != except)
            .map(|c| c.color)
            .chain(self.map.players.values().map(|p| p.color))
            .collect()
    }

    /// `SanitizePlayerColor`: reports problems to `echo`.
    fn sanitize_player_color(&mut self, asked: Color, player_index: i32, echo: Option<i32>) -> Color {
        let map = self.map.clone();
        let players = self.player_colors(Some(player_index));
        let (color, error) = map
            .color_picker
            .make_valid(asked, &mut self.random, &map.restricted_colors, &players);
        if let (Some(conn), Some(error)) = (echo, error) {
            self.send_fluent_message_to(conn, error, &[]);
        }
        color
    }

    /// `IClientJoined` of `LobbyCommands` and `LobbySettingsNotification`.
    pub fn client_joined(&mut self, conn: i32) {
        let pool: Vec<&str> = self.map_pool.iter().map(String::as_str).collect();
        let pool = pool.join(", ");
        self.send_order_to(conn, "SyncMapPool", &pool);
        let map = self.map.clone();
        if let Some(client) = self.lobby.client_with_index(conn).cloned()
            && self.target_slot(&client).is_some_and(|s| !s.lock_color)
        {
            let color = self.sanitize_player_color(client.color, client.index, None);
            if let Some(c) = self.lobby.client_with_index_mut(conn) {
                c.color = color;
            }
        }
        if let Some(briefing) = &map.briefing {
            self.send_order_to(conn, "Message", briefing);
        }
        let mut defaults = Session::default();
        map.load_map_settings(&mut defaults);
        let changed: Vec<(String, String)> = self
            .lobby
            .global_settings
            .lobby_options
            .iter()
            .filter(|(k, v)| defaults.global_settings.lobby_options.get(*k).is_none_or(|d| d.value != v.value))
            .map(|(k, v)| (k.clone(), v.value.clone().unwrap_or_default()))
            .collect();
        for (id, value) in changed {
            if let Some(o) = map.options.iter().rev().find(|o| o.id == id) {
                let text = o.values.get(&value).cloned().unwrap_or_default();
                self.send_fluent_message_to(
                    conn,
                    "notification-lobby-option",
                    &[("name", (&o.name).into()), ("value", (&text).into())],
                );
            }
        }
    }

    /// `INotifyServerEmpty`.
    pub fn server_empty(&mut self) {
        self.temp_bans.clear();
        self.lobby.global_settings.allow_spectators = true;
        self.reset_slots();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_are_case_sensitive() {
        assert_eq!(parse_state("Ready"), Some(ClientState::Ready));
        assert_eq!(parse_state("ready"), None);
    }

    #[test]
    fn bools_like_try_parse() {
        assert_eq!(parse_bool("True"), Some(true));
        assert_eq!(parse_bool(" false "), Some(false));
        assert_eq!(parse_bool("1"), None);
    }
}
