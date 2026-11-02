// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Player.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::CPos;

use crate::actor::{Actor, ActorRef};
use crate::actor_init::{ActorInits, OwnerInit};
use crate::world::World;

pub type PlayerRef = Rc<Player>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinState {
    Undefined,
    Won,
    Lost,
}

/// Bit flags of `PlayerRelationship`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerRelationship(pub i32);

impl PlayerRelationship {
    pub const NONE: PlayerRelationship = PlayerRelationship(0);
    pub const ENEMY: PlayerRelationship = PlayerRelationship(1);
    pub const NEUTRAL: PlayerRelationship = PlayerRelationship(2);
    pub const ALLY: PlayerRelationship = PlayerRelationship(4);

    pub fn has_relationship(self, r: PlayerRelationship) -> bool {
        self.0 & r.0 == r.0
    }
}

/// Player properties resolved before the player actor is created.
pub struct PlayerSetup {
    pub internal_name: String,
    pub player_name: String,
    pub faction: String,
    pub display_faction: String,
    pub non_combatant: bool,
    pub playable: bool,
    pub client_index: i32,
    pub home_location: CPos,
    pub handicap: i32,
    pub bot_type: Option<String>,
    pub spectating: bool,
    pub color: Color,
    pub spawn_point: i32,
    pub display_spawn_point: i32,
    pub in_mission_map: bool,
}

pub struct Player {
    pub internal_name: String,
    pub player_name: String,
    pub faction: String,
    pub display_faction: String,
    pub non_combatant: bool,
    pub playable: bool,
    pub client_index: i32,
    pub home_location: CPos,
    pub handicap: i32,
    pub bot_type: Option<String>,
    pub is_bot: bool,
    pub color: Color,
    pub spawn_point: i32,
    pub display_spawn_point: i32,
    spectating: bool,
    in_mission_map: bool,
    pub win_state: Cell<WinState>,
    pub has_objectives: Cell<bool>,
    /// Bit of this player in `LongBitSet<PlayerBitMask>`; 0 for spectators.
    pub player_mask: u64,
    pub allied_players_mask: Cell<u64>,
    pub enemy_players_mask: Cell<u64>,
    player_actor: OnceCell<ActorRef>,
    unlock_render_player: RefCell<Vec<Rc<dyn crate::traits::IUnlocksRenderPlayer>>>,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.player_name, self.client_index)
    }
}

impl Player {
    /// Creates the player and its player actor, like the C# constructor.
    pub fn create(world: &Rc<World>, s: PlayerSetup) -> PlayerRef {
        let player_mask = if s.spectating {
            0
        } else {
            world.allocate_player_bit(&s.internal_name)
        };
        let p = Rc::new(Player {
            is_bot: s.bot_type.is_some(),
            internal_name: s.internal_name,
            player_name: s.player_name,
            faction: s.faction,
            display_faction: s.display_faction,
            non_combatant: s.non_combatant,
            playable: s.playable,
            client_index: s.client_index,
            home_location: s.home_location,
            handicap: s.handicap,
            bot_type: s.bot_type,
            color: s.color,
            spawn_point: s.spawn_point,
            display_spawn_point: s.display_spawn_point,
            spectating: s.spectating,
            in_mission_map: s.in_mission_map,
            win_state: Cell::new(WinState::Undefined),
            has_objectives: Cell::new(false),
            player_mask,
            allied_players_mask: Cell::new(0),
            enemy_players_mask: Cell::new(0),
            player_actor: OnceCell::new(),
            unlock_render_player: RefCell::new(Vec::new()),
        });
        let actor = Actor::new(world, "player", ActorInits::new().with(OwnerInit::from_player(&p)));
        let _ = p.player_actor.set(actor.clone());
        actor.initialize(true);
        *p.unlock_render_player.borrow_mut() = actor.traits_implementing::<dyn crate::traits::IUnlocksRenderPlayer>();
        p
    }

    pub fn player_actor(&self) -> &ActorRef {
        self.player_actor.get().expect("player actor created")
    }

    pub fn spectating(&self) -> bool {
        !self.in_mission_map && (self.spectating || self.win_state.get() != WinState::Undefined)
    }

    pub fn unlocked_render_player(&self) -> bool {
        if self.unlock_render_player.borrow().iter().any(|x| x.render_player_unlocked()) {
            return true;
        }
        self.win_state.get() != WinState::Undefined && !self.in_mission_map
    }

    pub fn relationship_with(&self, other: Option<&Player>) -> PlayerRelationship {
        let Some(other) = other else {
            return if self.non_combatant {
                PlayerRelationship::NEUTRAL
            } else {
                PlayerRelationship::ALLY
            };
        };
        if std::ptr::eq(self, other) {
            return PlayerRelationship::ALLY;
        }
        if other.spectating() {
            return if self.non_combatant {
                PlayerRelationship::NEUTRAL
            } else {
                PlayerRelationship::ALLY
            };
        }
        if self.allied_players_mask.get() & other.player_mask != 0 {
            return PlayerRelationship::ALLY;
        }
        if self.enemy_players_mask.get() & other.player_mask != 0 {
            return PlayerRelationship::ENEMY;
        }
        PlayerRelationship::NEUTRAL
    }

    pub fn is_allied_with(&self, other: Option<&Player>) -> bool {
        self.relationship_with(other) == PlayerRelationship::ALLY
    }

    /// `Sync.HashPlayer`.
    pub fn sync_hash(&self) -> i32 {
        ((self.player_actor().id << 16) as i32).wrapping_mul(0x567)
    }
}

/// `Sync.HashPlayer` for optional players.
pub fn hash_player(p: Option<&PlayerRef>) -> i32 {
    p.map_or(0, |p| p.sync_hash())
}
