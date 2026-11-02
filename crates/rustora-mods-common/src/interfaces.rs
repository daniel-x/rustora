// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Interfaces of `OpenRA.Mods.Common/TraitsInterfaces.cs` (added as traits get ported).

use rustora_engine::actor::ActorRef;
use rustora_engine::damage::{AttackInfo, Damage};

pub trait INotifyDamage {
    fn damaged(&self, self_: &ActorRef, e: &AttackInfo);
}

pub trait INotifyDamageStateChanged {
    fn damage_state_changed(&self, self_: &ActorRef, e: &AttackInfo);
}

pub trait INotifyKilled {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo);
}

pub trait INotifyAppliedDamage {
    fn applied_damage(&self, self_: &ActorRef, damaged: &ActorRef, e: &AttackInfo);
}

pub trait IDamageModifier {
    fn get_damage_modifier(&self, attacker: Option<&ActorRef>, damage: &Damage) -> i32;
}
