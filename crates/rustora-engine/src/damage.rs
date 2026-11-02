// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Damage types and states (`TraitsInterfaces.cs`).

use crate::actor::ActorRef;
use crate::bitset::BitSet;

/// Bit flags of `DamageState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DamageState(pub i32);

impl DamageState {
    pub const UNDAMAGED: DamageState = DamageState(1);
    pub const LIGHT: DamageState = DamageState(2);
    pub const MEDIUM: DamageState = DamageState(4);
    pub const HEAVY: DamageState = DamageState(8);
    pub const CRITICAL: DamageState = DamageState(16);
    pub const DEAD: DamageState = DamageState(32);
}

pub struct DamageTypeTag;
pub type DamageTypes = BitSet<DamageTypeTag>;

#[derive(Clone, Debug)]
pub struct Damage {
    pub value: i32,
    pub damage_types: DamageTypes,
}

impl Damage {
    pub fn new(value: i32, damage_types: DamageTypes) -> Self {
        Damage { value, damage_types }
    }
}

#[derive(Clone)]
pub struct AttackInfo {
    pub damage: Damage,
    pub attacker: Option<ActorRef>,
    pub damage_state: DamageState,
    pub previous_damage_state: DamageState,
}
