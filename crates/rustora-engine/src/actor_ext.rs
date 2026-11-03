// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Actor helpers that depend on mod interfaces registered by mod crates.

use rustora_core::world_types::WPos;

use crate::actor::ActorRef;
use crate::bitset::BitSet;
use crate::terrain::TargetableType;

/// `Actor.GetTargetablePositions`: falls back to the center position.
pub fn targetable_positions(a: &ActorRef) -> Vec<WPos> {
    let providers = a.cache_opt().map_or_else(
        || a.traits_implementing::<dyn ITargetablePositions>(),
        |c| c.targetable_positions.clone(),
    );
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

/// `Actor.GetEnabledTargetTypes`.
pub fn enabled_target_types(a: &ActorRef) -> BitSet<TargetableType> {
    let mut t = BitSet::default();
    for tg in targetables(a) {
        if tg.is_trait_enabled() {
            t = t.union(&tg.target_types());
        }
    }
    t
}

/// `Actor.GetAllTargetTypes`.
pub fn all_target_types(a: &ActorRef) -> BitSet<TargetableType> {
    let mut t = BitSet::default();
    for tg in targetables(a) {
        t = t.union(&tg.target_types());
    }
    t
}

/// `Actor.IsTargetableBy`.
pub fn is_targetable_by(a: &ActorRef, by: &ActorRef) -> bool {
    targetables(a).iter().any(|t| t.targetable_by(a, by))
}

pub trait ITargetable {
    fn target_types(&self) -> BitSet<TargetableType>;
    fn targetable_by(&self, self_: &ActorRef, by_actor: &ActorRef) -> bool;
    fn requires_force_fire(&self) -> bool;
    fn is_trait_enabled(&self) -> bool;
}

/// `Actor.Targetables`, cached at creation like the C# array.
fn targetables(a: &ActorRef) -> Vec<std::rc::Rc<dyn ITargetable>> {
    a.cache_opt()
        .map_or_else(|| a.traits_implementing::<dyn ITargetable>(), |c| c.targetables.clone())
}
