// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Placeholder until `Traits/Attack/AttackBase.cs` is ported.

use rustora_core::world_types::WPos;
use rustora_engine::actor::ActorRef;
use rustora_engine::target::Target;

pub struct AttackBase;

impl AttackBase {
    pub fn is_aiming(&self) -> bool {
        false
    }
    pub fn on_stop_order(&self, _self_: &ActorRef) {}
    pub fn is_trait_disabled(&self) -> bool {
        false
    }
    pub fn is_trait_paused(&self) -> bool {
        false
    }
    pub fn is_turreted_with(&self, _turret: &str) -> bool {
        false
    }
    pub fn get_target_position(&self, _pos: WPos, target: &Target) -> WPos {
        target.center_position()
    }
}
