// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Resolved orders (`Network/Order.cs` with actor IDs looked up in the world).

use rustora_core::world_types::CPos;
use rustora_net::order::{RawOrder, RawTarget};

use crate::actor::ActorRef;
use crate::player::PlayerRef;
use crate::target::Target;
use crate::world::World;

#[derive(Clone)]
pub struct Order {
    pub order_string: String,
    pub subject: Option<ActorRef>,
    pub target: Target,
    pub target_string: Option<String>,
    pub queued: bool,
    pub extra_actors: Option<Vec<Option<ActorRef>>>,
    pub extra_location: CPos,
    pub extra_data: u32,
    pub grouped_actors: Option<Vec<Option<ActorRef>>>,
    pub is_immediate: bool,
}

impl Order {
    /// `new Order(orderString, subject, target, queued)`.
    pub fn new(order_string: &str, subject: Option<ActorRef>, target: Target, queued: bool) -> Order {
        Order {
            order_string: order_string.to_string(),
            subject,
            target,
            target_string: None,
            queued,
            extra_actors: None,
            extra_location: CPos::ZERO,
            extra_data: 0,
            grouped_actors: None,
            is_immediate: false,
        }
    }

    /// `Order.FromGroupedOrder`.
    pub fn from_grouped(&self, subject: Option<ActorRef>) -> Order {
        Order {
            subject,
            grouped_actors: None,
            ..self.clone()
        }
    }

    pub fn player(&self) -> Option<PlayerRef> {
        self.subject.as_ref().and_then(|s| s.owner_opt())
    }

    /// Port of `Order.Deserialize(world, ...)`: returns `None` if the subject no longer exists.
    pub fn resolve(world: &World, raw: &RawOrder) -> Option<Order> {
        let lookup = |id: u32| -> Result<Option<ActorRef>, ()> {
            if id == u32::MAX {
                return Ok(None);
            }
            world.get_actor_by_id(id).map(Some).ok_or(())
        };
        let subject = match raw.subject {
            Some(id) => Some(lookup(id).ok()??),
            None => None,
        };
        let target = match &raw.target {
            RawTarget::Invalid => Target::Invalid,
            RawTarget::Actor { id, generation } => match lookup(*id) {
                Ok(a) => Target::from_serialized_actor(a, *generation),
                Err(()) => Target::Invalid,
            },
            RawTarget::FrozenActor {
                player_actor_id,
                frozen_id,
            } => world
                .get_actor_by_id(*player_actor_id)
                .and_then(|a| a.owner().frozen_actor_layer())
                .and_then(|l| l.from_id(*frozen_id))
                .map_or(Target::Invalid, Target::from_frozen_actor),
            RawTarget::Cell { cell, subcell } => Target::from_cell(world, *cell, *subcell),
            RawTarget::Pos { pos, terrain_positions } => match terrain_positions {
                None => Target::from_pos(*pos),
                Some(p) => Target::from_positions(*pos, p.clone()),
            },
        };
        let ids = |v: &Option<Vec<u32>>| v.as_ref().map(|v| v.iter().map(|id| world.get_actor_by_id(*id)).collect());
        Some(Order {
            order_string: raw.order_string.clone(),
            subject,
            target,
            target_string: raw.target_string.clone(),
            queued: raw.queued,
            extra_actors: ids(&raw.extra_actors),
            extra_location: raw.extra_location,
            extra_data: raw.extra_data,
            grouped_actors: ids(&raw.grouped_actors),
            is_immediate: raw.is_immediate,
        })
    }
}

impl Order {
    /// `Order.Serialize` for field orders, without the byte encoding: actor references become ids.
    pub fn to_raw(&self) -> RawOrder {
        let id = |a: &Option<ActorRef>| a.as_ref().map_or(u32::MAX, |a| a.id);
        let ids = |v: &Option<Vec<Option<ActorRef>>>| v.as_ref().map(|v| v.iter().map(id).collect());
        let target = match &self.target {
            Target::Invalid => RawTarget::Invalid,
            Target::Actor { actor, generation } => RawTarget::Actor {
                id: actor.id,
                generation: *generation,
            },
            Target::FrozenActor(fa) => RawTarget::FrozenActor {
                player_actor_id: fa.viewer.player_actor().id,
                frozen_id: fa.id(),
            },
            Target::Terrain {
                center,
                positions,
                cell,
                subcell,
            } => match cell {
                Some(cell) => RawTarget::Cell {
                    cell: *cell,
                    subcell: subcell.unwrap_or(0),
                },
                None => RawTarget::Pos {
                    pos: *center,
                    terrain_positions: if positions.len() == 1 && positions[0] == *center {
                        None
                    } else {
                        Some(positions.to_vec())
                    },
                },
            },
        };
        RawOrder {
            subject: self.subject.as_ref().map(|a| a.id),
            target,
            target_string: self.target_string.clone(),
            queued: self.queued,
            extra_actors: ids(&self.extra_actors),
            extra_location: self.extra_location,
            extra_data: self.extra_data,
            grouped_actors: ids(&self.grouped_actors),
            is_immediate: self.is_immediate,
            ..RawOrder::new(&self.order_string)
        }
    }
}
