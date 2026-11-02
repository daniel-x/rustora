// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `OpenRA.Mods.Common`.

pub mod actor_reference;
pub mod world;

use rustora_engine::rules::TraitRegistry;

/// Registers all ported traits of this crate.
pub fn register(r: &mut TraitRegistry) {
    world::register(r);
}
