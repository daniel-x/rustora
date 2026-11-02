// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `OpenRA.Mods.Common`.

pub mod activities;
pub mod actor_map;
pub mod actor_reference;
pub mod aircraft;
pub mod attack;
pub mod attack_move;
pub mod building;
pub mod cloak;
pub mod health;
pub mod hit_shape;
pub mod hpf;
pub mod interfaces;
pub mod locomotor;
pub mod mobile;
pub mod parachutable;
pub mod path_finder_trait;
pub mod pathfinder;
pub mod player;
pub mod render;
pub mod resources;
pub mod smudge;
pub mod targetable;
pub mod turreted;
pub mod util;
pub mod visibility;
pub mod world;

use rustora_engine::rules::TraitRegistry;

/// Registers all ported traits of this crate.
pub fn register(r: &mut TraitRegistry) {
    actor_map::register(r);
    building::register(r);
    health::register(r);
    hit_shape::register(r);
    locomotor::register(r);
    mobile::register(r);
    parachutable::register(r);
    path_finder_trait::register(r);
    resources::register(r);
    targetable::register(r);
    turreted::register(r);
    visibility::register(r);
    player::register(r);
    render::register(r);
    world::register(r);
}
