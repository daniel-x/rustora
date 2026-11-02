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

impl rustora_core::fieldloader::FieldValue for DamageState {
    const TYPE_NAME: &'static str = "DamageState";

    fn parse(field: &str, value: Option<&str>, _yaml: Option<&rustora_core::miniyaml::MiniYaml>) -> Result<Self, rustora_core::fieldloader::LoadError> {
        let mut flags = 0;
        for part in value.unwrap_or("").split(',').map(str::trim).filter(|p| !p.is_empty()) {
            flags |= match part {
                "Undamaged" => 1,
                "Light" => 2,
                "Medium" => 4,
                "Heavy" => 8,
                "Critical" => 16,
                "Dead" => 32,
                _ => return rustora_core::fieldloader::invalid(value, "DamageState", field),
            };
        }
        Ok(DamageState(flags))
    }
}

impl rustora_core::fieldloader::ArrayAware for DamageState {}

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
