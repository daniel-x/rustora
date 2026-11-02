// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Placeholder until `Traits/Cloak.cs` is ported.

use rustora_engine::actor::ActorRef;
use rustora_engine::player::PlayerRef;

pub struct Cloak;

impl Cloak {
    pub fn is_trait_disabled(&self) -> bool {
        true
    }
    pub fn is_visible_to(&self, _self_: &ActorRef, _viewer: &PlayerRef) -> bool {
        true
    }
}
