// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Impact by weapon name.

use rustora_engine::actor::ActorRef;
use rustora_engine::target::Target;

pub fn weapon_impact(weapon: &str, target: &Target, firer: &ActorRef) {
    let w = firer.world().rules().weapon(&weapon.to_lowercase()).unwrap_or_else(|| panic!("unknown weapon {weapon}"));
    rustora_engine::weapon::weapon_impact(&w, target, firer);
}
