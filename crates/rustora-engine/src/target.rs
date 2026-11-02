// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Target.cs`.

use std::rc::Rc;

use rustora_core::world_types::{CPos, WDist, WPos};

use crate::actor::ActorRef;
use crate::frozen::FrozenActorRef;
use crate::world::World;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetType {
    Invalid,
    Actor,
    Terrain,
    FrozenActor,
}

#[derive(Clone, Default)]
pub enum Target {
    #[default]
    Invalid,
    Actor {
        actor: ActorRef,
        generation: i32,
    },
    FrozenActor(FrozenActorRef),
    Terrain {
        center: WPos,
        positions: Rc<[WPos]>,
        cell: Option<CPos>,
        subcell: Option<u8>,
    },
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.target_type() {
            TargetType::Actor => write!(f, "{:?}", self.actor().unwrap()),
            TargetType::FrozenActor => write!(f, "FrozenActor"),
            TargetType::Terrain => write!(f, "{}", self.center_position()),
            TargetType::Invalid => write!(f, "Invalid"),
        }
    }
}

impl Target {
    pub fn from_pos(p: WPos) -> Self {
        Target::Terrain {
            center: p,
            positions: Rc::from(vec![p]),
            cell: None,
            subcell: None,
        }
    }

    pub fn from_positions(center: WPos, positions: Vec<WPos>) -> Self {
        Target::Terrain {
            center,
            positions: Rc::from(positions),
            cell: None,
            subcell: None,
        }
    }

    pub fn from_cell(world: &World, c: CPos, subcell: u8) -> Self {
        let center = world.map().center_of_subcell(c, subcell);
        Target::Terrain {
            center,
            positions: Rc::from(vec![center]),
            cell: Some(c),
            subcell: Some(subcell),
        }
    }

    pub fn from_actor(a: Option<&ActorRef>) -> Self {
        match a {
            Some(a) => Target::Actor {
                actor: a.clone(),
                generation: a.generation.get(),
            },
            None => Target::Invalid,
        }
    }

    pub fn from_serialized_actor(a: Option<ActorRef>, generation: i32) -> Self {
        match a {
            Some(actor) => Target::Actor { actor, generation },
            None => Target::Invalid,
        }
    }

    pub fn from_frozen_actor(fa: FrozenActorRef) -> Self {
        Target::FrozenActor(fa)
    }

    /// `Target.Type`: actor targets become invalid when the actor died or changed generation.
    pub fn target_type(&self) -> TargetType {
        match self {
            Target::Invalid => TargetType::Invalid,
            Target::Actor { actor, generation } => {
                if !actor.is_in_world() || actor.is_dead() || actor.generation.get() != *generation {
                    TargetType::Invalid
                } else {
                    TargetType::Actor
                }
            }
            Target::FrozenActor(_) => TargetType::FrozenActor,
            Target::Terrain { .. } => TargetType::Terrain,
        }
    }

    /// The raw type ignoring actor validity (the private `type` field in C#).
    pub fn raw_type(&self) -> TargetType {
        match self {
            Target::Invalid => TargetType::Invalid,
            Target::Actor { .. } => TargetType::Actor,
            Target::FrozenActor(_) => TargetType::FrozenActor,
            Target::Terrain { .. } => TargetType::Terrain,
        }
    }

    pub fn actor(&self) -> Option<&ActorRef> {
        match self {
            Target::Actor { actor, .. } => Some(actor),
            _ => None,
        }
    }

    pub fn frozen_actor(&self) -> Option<&FrozenActorRef> {
        match self {
            Target::FrozenActor(f) => Some(f),
            _ => None,
        }
    }

    pub fn center_position(&self) -> WPos {
        match self.target_type() {
            TargetType::Actor => self.actor().unwrap().center_position(),
            TargetType::FrozenActor => self.frozen_actor().unwrap().center_position(),
            TargetType::Terrain => match self {
                Target::Terrain { center, .. } => *center,
                _ => unreachable!(),
            },
            TargetType::Invalid => panic!("Attempting to query the position of an invalid Target"),
        }
    }

    pub fn positions(&self) -> Vec<WPos> {
        match self.target_type() {
            TargetType::Actor => crate::actor_ext::targetable_positions(self.actor().unwrap()),
            TargetType::FrozenActor => self.frozen_actor().unwrap().targetable_positions(),
            TargetType::Terrain => match self {
                Target::Terrain { positions, .. } => positions.to_vec(),
                _ => unreachable!(),
            },
            TargetType::Invalid => Vec::new(),
        }
    }

    pub fn is_in_range(&self, origin: WPos, range: WDist) -> bool {
        if self.target_type() == TargetType::Invalid {
            return false;
        }
        self.positions()
            .iter()
            .any(|t| (*t - origin).horizontal_length_squared() <= range.length_squared())
    }

    /// `Sync.HashTarget`.
    pub fn sync_hash(&self) -> i32 {
        match self.target_type() {
            TargetType::Actor => (self.actor().unwrap().id << 16).wrapping_mul(0x567) as i32,
            TargetType::FrozenActor => match self.frozen_actor().unwrap().actor() {
                Some(a) => (a.id << 16).wrapping_mul(0x567) as i32,
                None => 0,
            },
            TargetType::Terrain => self.center_position().hash(),
            TargetType::Invalid => 0,
        }
    }

    /// C# `==` on targets (terrain positions compare by reference).
    pub fn same_as(&self, o: &Target) -> bool {
        match (self, o) {
            (
                Target::Terrain {
                    center: c1,
                    positions: p1,
                    cell: l1,
                    subcell: s1,
                },
                Target::Terrain {
                    center: c2,
                    positions: p2,
                    cell: l2,
                    subcell: s2,
                },
            ) => c1 == c2 && Rc::ptr_eq(p1, p2) && l1 == l2 && s1 == s2,
            (Target::Actor { actor: a1, generation: g1 }, Target::Actor { actor: a2, generation: g2 }) => a1.id == a2.id && g1 == g2,
            (Target::FrozenActor(f1), Target::FrozenActor(f2)) => Rc::ptr_eq(f1, f2),
            _ => false,
        }
    }
}
