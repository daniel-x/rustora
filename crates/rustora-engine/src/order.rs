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
            RawTarget::FrozenActor { .. } => Target::Invalid,
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
