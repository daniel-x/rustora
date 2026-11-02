// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Interfaces of `OpenRA.Mods.Common/TraitsInterfaces.cs` (added as traits get ported).

use rustora_core::world_types::{CPos, WAngle, WDist, WPos, WVec};
use rustora_engine::activity::ActivityRef;
use rustora_engine::actor::ActorRef;
use rustora_engine::bitset::BitSet;
use rustora_engine::damage::{AttackInfo, Damage};
use rustora_engine::target::Target;
use rustora_engine::traits::CrushClass;

use crate::locomotor::BlockedByActor;

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

pub trait ITargetableCells {
    fn targetable_cells(&self) -> Vec<(rustora_core::world_types::CPos, u8)>;
}

pub trait INotifySold {
    fn selling(&self, self_: &ActorRef);
    fn sold(&self, self_: &ActorRef);
}

pub trait INotifyCustomLayerChanged {
    fn custom_layer_changed(&self, self_: &ActorRef, old_layer: u8, new_layer: u8);
}

pub trait INotifyCenterPositionChanged {
    fn center_position_changed(&self, self_: &ActorRef, old_layer: u8, new_layer: u8);
}

pub trait INotifyFinishedMoving {
    fn finished_moving(&self, self_: &ActorRef, old_layer: u8, new_layer: u8);
}

pub trait INotifyMoving {
    fn movement_type_changed(&self, self_: &ActorRef, ty: u8);
}

pub trait INotifyCrushed {
    fn on_crush(&self, self_: &ActorRef, crusher: &ActorRef, classes: BitSet<CrushClass>);
    fn warn_crush(&self, self_: &ActorRef, crusher: &ActorRef, classes: BitSet<CrushClass>);
}

pub trait INotifyBlockingMove {
    fn on_notify_blocking_move(&self, self_: &ActorRef, blocking: &ActorRef);
}

pub trait ISpeedModifier {
    fn get_speed_modifier(&self) -> i32;
}

pub trait IRevealsShroudModifier {
    fn get_reveals_shroud_modifier(&self) -> i32;
}

pub trait ICreatesShroudModifier {
    fn get_creates_shroud_modifier(&self) -> i32;
}

pub trait IWrapMove {
    fn is_trait_enabled(&self) -> bool;
    fn wrap_move(&self, inner: ActivityRef) -> ActivityRef;
}

pub trait INotifyParachute {
    fn on_parachute(&self, self_: &ActorRef);
    fn on_landed(&self, self_: &ActorRef);
}

pub trait IDeathActorInitModifier {
    fn modify_death_actor_init(&self, self_: &ActorRef, init: &mut rustora_engine::actor_init::ActorInits);
}

pub trait ITransformActorInitModifier {
    fn modify_transform_actor_init(&self, self_: &ActorRef, init: &mut rustora_engine::actor_init::ActorInits);
}

/// `IMove`: the positional parts of the movement traits that other activities use.
pub trait IMove {
    fn move_to(&self, cell: CPos, near_enough: i32, ignore_actor: Option<ActorRef>, evaluate_nearest_movable_cell: bool) -> ActivityRef;
    fn move_within_range(&self, target: &Target, min_range: WDist, max_range: WDist, initial_target_position: Option<WPos>) -> ActivityRef;
    fn move_follow(&self, self_: &ActorRef, target: &Target, min_range: WDist, max_range: WDist, initial_target_position: Option<WPos>) -> ActivityRef;
    fn move_to_target(&self, self_: &ActorRef, target: &Target, initial_target_position: Option<WPos>) -> Option<ActivityRef>;
    fn return_to_cell(&self, self_: &ActorRef) -> ActivityRef;
    fn move_into_target(&self, self_: &ActorRef, target: &Target) -> Option<ActivityRef>;
    fn move_onto_target(&self, self_: &ActorRef, target: &Target, offset: WVec, facing: Option<WAngle>) -> ActivityRef;
    fn local_move(&self, self_: &ActorRef, from: WPos, to: WPos) -> ActivityRef;
    fn estimated_move_duration(&self, self_: &ActorRef, from: WPos, to: WPos) -> i32;
    fn nearest_moveable_cell(&self, target: CPos) -> CPos;
    fn current_movement_types(&self) -> u8;
    fn set_current_movement_types(&self, t: u8);
    fn can_enter_target_now(&self, self_: &ActorRef, target: &Target) -> bool;
    fn as_mobile(&self) -> Option<&crate::mobile::Mobile> {
        None
    }
}

/// `IPositionable`.
pub trait IPositionable {
    fn can_exist_in_cell(&self, location: CPos) -> bool;
    fn can_enter_cell(&self, location: CPos, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> bool;
    fn get_valid_subcell(&self, preferred: u8) -> u8;
    fn get_available_subcell(&self, location: CPos, preferred: u8, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> u8;
    fn set_position_cell(&self, self_: &ActorRef, cell: CPos, sub: u8);
    fn set_position(&self, self_: &ActorRef, pos: WPos);
    fn set_center_position(&self, self_: &ActorRef, pos: WPos);
}

pub trait INotifyTransform {
    fn before_transform(&self, self_: &ActorRef);
    fn on_transform(&self, self_: &ActorRef);
    fn after_transform(&self, to_actor: &ActorRef);
}
