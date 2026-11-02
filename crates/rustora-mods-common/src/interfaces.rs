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
    fn return_to_cell(&self, self_: &ActorRef) -> Option<ActivityRef>;
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

pub trait ITechTreePrerequisite {
    fn provides_prerequisites(&self) -> Vec<String>;
}

pub trait ITechTreeElement {
    fn prerequisites_available(&self, key: &str);
    fn prerequisites_unavailable(&self, key: &str);
    fn prerequisites_item_hidden(&self, key: &str);
    fn prerequisites_item_visible(&self, key: &str);
}

pub trait INotifyProduction {
    fn unit_produced(&self, self_: &ActorRef, other: &ActorRef, exit: CPos);
}

pub trait INotifyOtherProduction {
    fn unit_produced_by_other(&self, self_: &ActorRef, producer: &ActorRef, produced: &ActorRef, production_type: &str, init: &rustora_engine::actor_init::ActorInits);
}

pub trait INotifyBuildingPlaced {
    fn building_placed(&self, self_: &ActorRef);
}

pub trait INotifyPowerLevelChanged {
    fn power_level_changed(&self, self_: &ActorRef);
}

pub trait IPowerModifier {
    fn get_power_modifier(&self) -> i32;
}

pub trait IProductionCostModifierInfo {
    fn get_production_cost_modifier(&self, tech_tree: &crate::production::TechTree, queue: &str) -> i32;
}

pub trait IProductionTimeModifierInfo {
    fn get_production_time_modifier(&self, tech_tree: &crate::production::TechTree, queue: &str) -> i32;
}

pub trait ISeedableResource {
    fn seed(&self, self_: &ActorRef);
}

pub trait INotifyLineBuildSegmentsChanged {
    fn segment_added(&self, self_: &ActorRef, segment: &ActorRef);
    fn segment_removed(&self, self_: &ActorRef, segment: &ActorRef);
}

pub trait INotifyDeployTriggered {
    fn deploy(&self, self_: &ActorRef, skip_make_anim: bool);
    fn undeploy(&self, self_: &ActorRef, skip_make_anim: bool);
}

pub trait INotifyDeployComplete {
    fn finished_deploy(&self, self_: &ActorRef);
    fn finished_undeploy(&self, self_: &ActorRef);
}

pub trait IDockClientBody {
    fn play_dock_animation(&self, self_: &ActorRef, after: std::rc::Rc<dyn Fn()>);
    fn play_reverse_dock_animation(&self, self_: &ActorRef, after: std::rc::Rc<dyn Fn()>);
}

pub trait INotifyHarvestAction {
    fn harvested(&self, self_: &ActorRef, resource_type: &str);
    fn moving_to_resources(&self, self_: &ActorRef, target_cell: CPos);
    fn movement_cancelled(&self, self_: &ActorRef);
}

pub trait INotifyCapture {
    fn on_capture(&self, self_: &ActorRef, captor: &ActorRef, old_owner: &rustora_engine::player::PlayerRef, new_owner: &rustora_engine::player::PlayerRef, types: BitSet<crate::capture::CaptureType>);
}

pub trait INotifyEnteredCargo {
    fn on_entered_cargo(&self, self_: &ActorRef, cargo: &ActorRef);
}

pub trait INotifyExitedCargo {
    fn on_exited_cargo(&self, self_: &ActorRef, cargo: &ActorRef);
}

pub trait INotifyPassengerEntered {
    fn on_passenger_entered(&self, self_: &ActorRef, passenger: &ActorRef);
}

pub trait INotifyPassengerExited {
    fn on_passenger_exited(&self, self_: &ActorRef, passenger: &ActorRef);
}

pub trait INotifyLoadCargo {
    fn loading(&self, self_: &ActorRef);
}

pub trait INotifyUnloadCargo {
    fn unloading(&self, self_: &ActorRef);
}

pub trait INotifyDockClient {
    fn docked(&self, self_: &ActorRef, host: &ActorRef);
    fn undocked(&self, self_: &ActorRef, host: &ActorRef);
}

pub trait INotifyDockHost {
    fn docked(&self, self_: &ActorRef, client: &ActorRef);
    fn undocked(&self, self_: &ActorRef, client: &ActorRef);
}
