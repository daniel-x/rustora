// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Actor helpers that depend on mod interfaces registered by mod crates.

use rustora_core::world_types::WPos;

use crate::actor::ActorRef;

/// `Actor.GetTargetablePositions`: falls back to the center position.
pub fn targetable_positions(a: &ActorRef) -> Vec<WPos> {
    let providers = a.traits_implementing::<dyn ITargetablePositions>();
    let mut out = Vec::new();
    let mut any = false;
    for p in &providers {
        if p.is_trait_enabled() {
            any = true;
            out.extend(p.targetable_positions(a));
        }
    }
    if any { out } else { vec![a.center_position()] }
}

pub trait ITargetablePositions {
    fn targetable_positions(&self, self_: &ActorRef) -> Vec<WPos>;
    fn is_trait_enabled(&self) -> bool;
}
