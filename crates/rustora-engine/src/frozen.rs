// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Frozen actors (`Traits/Player/FrozenActorLayer.cs`). Placeholder until the layer is ported.

use std::rc::Rc;

use rustora_core::world_types::WPos;

use crate::actor::ActorRef;

pub type FrozenActorRef = Rc<FrozenActor>;

pub struct FrozenActor {
    pub id: u32,
    actor: Option<ActorRef>,
    center: WPos,
}

impl FrozenActor {
    pub fn actor(&self) -> Option<&ActorRef> {
        self.actor.as_ref()
    }

    pub fn center_position(&self) -> WPos {
        self.center
    }

    pub fn targetable_positions(&self) -> Vec<WPos> {
        vec![self.center]
    }
}
