// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The player references of a map (`PlayerReference`, `MapPlayers`) and how the lobby derives slots and client
//! settings from them. Client and server use them.

use indexmap::IndexMap;
use rustora_core::fieldloader::{Color, load};
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_core::world_types::CPos;
use rustora_derive::FieldLoad;
use rustora_net::session::{Client, Slot};

/// `PlayerReference` from map.yaml.
#[derive(Clone, Debug, FieldLoad)]
pub struct PlayerReference {
    pub name: String,
    pub palette: Option<String>,
    pub bot: Option<String>,
    pub starting_units_class: Option<String>,
    pub allow_bots: bool,
    pub playable: bool,
    pub required: bool,
    pub owns_world: bool,
    pub spectating: bool,
    pub non_combatant: bool,
    pub lock_faction: bool,
    pub faction: String,
    pub lock_color: bool,
    pub color: Color,
    pub home_location: CPos,
    pub lock_spawn: bool,
    pub spawn: i32,
    pub lock_team: bool,
    pub team: i32,
    pub lock_handicap: bool,
    pub handicap: i32,
    pub allies: Vec<String>,
    pub enemies: Vec<String>,
}

impl Default for PlayerReference {
    fn default() -> Self {
        PlayerReference {
            name: String::new(),
            palette: None,
            bot: None,
            starting_units_class: None,
            allow_bots: true,
            playable: false,
            required: false,
            owns_world: false,
            spectating: false,
            non_combatant: false,
            lock_faction: false,
            faction: String::new(),
            lock_color: false,
            color: Color::from_argb(255, 0xEE, 0xEE, 0xEE),
            home_location: CPos::ZERO,
            lock_spawn: false,
            spawn: 0,
            lock_team: false,
            team: 0,
            lock_handicap: false,
            handicap: 0,
            allies: Vec::new(),
            enemies: Vec::new(),
        }
    }
}

/// `LobbyCommands.MakeSlotFromPlayerReference`.
pub fn make_slot(pr: &PlayerReference) -> Option<Slot> {
    pr.playable.then(|| Slot {
        player_reference: pr.name.clone(),
        closed: false,
        allow_bots: pr.allow_bots,
        lock_faction: pr.lock_faction,
        lock_color: pr.lock_color,
        lock_team: pr.lock_team,
        lock_handicap: pr.lock_handicap,
        lock_spawn: pr.lock_spawn,
        required: pr.required,
    })
}

/// `Server.SyncClientToPlayerReference`.
pub fn sync_client_to_player_reference(c: &mut Client, pr: &PlayerReference) {
    if pr.lock_faction {
        c.faction = Some(pr.faction.clone());
    }
    if pr.lock_spawn {
        c.spawn_point = pr.spawn;
    }
    if pr.lock_team {
        c.team = pr.team;
    }
    if pr.lock_handicap {
        c.handicap = pr.handicap;
    }
    c.color = if pr.lock_color { pr.color } else { c.preferred_color };
}

/// `MapPlayers`: player references keyed by name, in definition order.
pub fn map_players(defs: &[MiniYamlNode]) -> IndexMap<String, PlayerReference> {
    let mut out = IndexMap::new();
    for d in defs {
        let pr: PlayerReference = load(&MiniYaml::new(Some(d.key.clone()), d.value.nodes.clone())).expect("valid player reference");
        out.insert(pr.name.clone(), pr);
    }
    out
}
