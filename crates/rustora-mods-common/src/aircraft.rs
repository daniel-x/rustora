// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Air/Aircraft.cs`, `Traits/Buildings/Reservable.cs` and `Activities/Air`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_core::expression::BooleanExpression;
use rustora_core::world_types::{CPos, WAngle, WDist, WPos, WRot, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::activity::{LINE_GREEN, TargetLineNode, chain_target_line_nodes, with_line_color};
use rustora_engine::actor::{ActorRef, ConditionCache, INVALID_CONDITION_TOKEN};
use rustora_engine::actor_init::{CenterPositionInit, FacingInit, LocationInit};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::DamageTypes;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::subcell::{FULL_CELL, INVALID};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{activity_common, info_impl, load_info, register_traits};

use crate::activities::{Nudge, Turn, Wait, initial_last_visible, notify_blocker_cells};
use crate::actor_map::WorldExt;
use crate::actor_reference::SpawnedByMapInit;
use crate::attack_move::AttackMoveActivity;
use crate::interfaces::*;
use crate::locomotor::{BlockedByActor, movement_type};
use crate::mobile::{CreationActivityDelayInit, Mobile, RallyPointInit};
use crate::production::{RallyPoint, nearest_exit_or_default};
use crate::resupply::{Rearmable, RearmableInfo, Repairable, RepairableInfo, Resupply, has_damage};
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("Aircraft", |n| load_info!(AircraftInfo, n));
    r.register("Reservable", |n| load_info!(ReservableInfo, n));
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IdleBehaviorType {
    #[default]
    None,
    Land,
    ReturnToBase,
    LeaveMap,
    LeaveMapAtClosestEdge,
}

rustora_core::field_enum!(IdleBehaviorType, [None, Land, ReturnToBase, LeaveMap, LeaveMapAtClosestEdge]);

#[derive(Clone, Debug, FieldLoad)]
pub struct AircraftInfo {
    pub voice: String,
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub idle_behavior: IdleBehaviorType,
    pub cruise_altitude: WDist,
    pub repulsable: bool,
    pub ideal_separation: WDist,
    pub repulsion_speed: i32,
    pub initial_facing: WAngle,
    pub turn_speed: WAngle,
    pub idle_turn_speed: Option<WAngle>,
    pub turn_deadzone: WAngle,
    pub speed: i32,
    pub idle_speed: i32,
    pub pitch: WAngle,
    pub pitch_speed: WAngle,
    pub roll: WAngle,
    pub idle_roll: Option<WAngle>,
    pub roll_speed: WAngle,
    pub min_airborne_altitude: i32,
    pub landable_terrain_types: IndexSet<String>,
    pub move_into_shroud: bool,
    pub crushes: BitSet<CrushClass>,
    pub crush_damage_types: DamageTypes,
    pub airborne_condition: Option<String>,
    pub cruising_condition: Option<String>,
    pub can_hover: bool,
    pub can_slide: bool,
    #[field(name = "VTOL")]
    pub vtol: bool,
    pub turn_to_land: bool,
    pub take_off_on_resupply: bool,
    pub take_off_on_creation: bool,
    pub can_force_land: bool,
    pub cursor: String,
    pub blocked_cursor: String,
    pub enter_cursor: String,
    pub enter_blocked_cursor: String,
    pub land_altitude: WDist,
    pub land_range: WDist,
    pub maximum_pitch: WAngle,
    pub altitude_velocity: WDist,
    pub takeoff_sounds: Vec<String>,
    pub landing_sounds: Vec<String>,
    pub wait_distance_from_resupply_base: WDist,
    pub number_of_ticks_to_verify_available_airport: i32,
    pub require_force_move_condition: Option<BooleanExpression>,
}

impl Default for AircraftInfo {
    fn default() -> Self {
        AircraftInfo {
            voice: "Action".into(),
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            idle_behavior: IdleBehaviorType::None,
            cruise_altitude: WDist::new(1280),
            repulsable: true,
            ideal_separation: WDist::new(1706),
            repulsion_speed: -1,
            initial_facing: WAngle::ZERO,
            turn_speed: WAngle::new(512),
            idle_turn_speed: None,
            turn_deadzone: WAngle::new(2),
            speed: 1,
            idle_speed: -1,
            pitch: WAngle::ZERO,
            pitch_speed: WAngle::ZERO,
            roll: WAngle::ZERO,
            idle_roll: None,
            roll_speed: WAngle::ZERO,
            min_airborne_altitude: 1,
            landable_terrain_types: IndexSet::new(),
            move_into_shroud: true,
            crushes: BitSet::default(),
            crush_damage_types: DamageTypes::default(),
            airborne_condition: None,
            cruising_condition: None,
            can_hover: false,
            can_slide: false,
            vtol: false,
            turn_to_land: false,
            take_off_on_resupply: false,
            take_off_on_creation: true,
            can_force_land: true,
            cursor: "move".into(),
            blocked_cursor: "move-blocked".into(),
            enter_cursor: "enter".into(),
            enter_blocked_cursor: "enter-blocked".into(),
            land_altitude: WDist::ZERO,
            land_range: WDist::from_cells(5),
            maximum_pitch: WAngle::from_degrees(10),
            altitude_velocity: WDist::new(43),
            takeoff_sounds: Vec::new(),
            landing_sounds: Vec::new(),
            wait_distance_from_resupply_base: WDist::new(3072),
            number_of_ticks_to_verify_available_airport: 150,
            require_force_move_condition: None,
        }
    }
}

impl AircraftInfo {
    /// `IPositionableInfo.CanEnterCell`.
    pub fn can_enter_cell(&self, w: &World, cell: CPos, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> bool {
        if !w.map().contains(cell) || !self.landable_terrain_types.contains(&w.map().terrain_type_info(cell).ty) {
            return false;
        }
        check == BlockedByActor::None
            || !w
                .actor_map()
                .get_actors_at(cell)
                .iter()
                .any(|x| ignore_actor.is_none_or(|i| i.id != x.id))
    }
}

/// `Reservable.IsReserved`.
pub fn is_reserved(a: &ActorRef) -> bool {
    a.trait_or_default::<Reservable>().is_some_and(|r| {
        r.reserved_for_aircraft
            .borrow()
            .as_ref()
            .is_some_and(|f| !f.may_yield_reservation())
    })
}

/// `Reservable.IsAvailableFor`.
pub fn is_available_for(reservable: &ActorRef, for_actor: &ActorRef) -> bool {
    match reservable.trait_or_default::<Reservable>() {
        None => true,
        Some(r) => {
            let aircraft = r.reserved_for_aircraft.borrow();
            match aircraft.as_ref() {
                None => true,
                Some(f) => f.may_yield_reservation() || r.reserved_for.borrow().as_ref().is_some_and(|x| x.id == for_actor.id),
            }
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct Aircraft {
    pub info: Rc<AircraftInfo>,
    state: ConditionalState,
    self_: ActorRef,
    repairable: OnceCell<Option<Rc<Repairable>>>,
    rearmable: OnceCell<Option<Rc<Rearmable>>>,
    position_offsets: OnceCell<Vec<Rc<dyn IAircraftCenterPositionOffset>>>,
    override_aircraft_landing: OnceCell<Option<Rc<dyn IOverrideAircraftLanding>>>,
    reservation: RefCell<Option<Rc<Reservable>>>,
    speed_modifiers: OnceCell<Vec<Rc<dyn ISpeedModifier>>>,
    notify_moving: RefCell<Vec<Rc<dyn INotifyMoving>>>,
    notify_center_position_changed: RefCell<Option<Vec<Rc<dyn INotifyCenterPositionChanged>>>>,
    orientation: Cell<WRot>,
    center_position: Cell<WPos>,
    reserved_actor: RefCell<Option<ActorRef>>,
    may_yield_reservation: Cell<bool>,
    force_landing: Cell<bool>,
    landing_cells: RefCell<Vec<(CPos, u8)>>,
    require_force_move: Cell<bool>,
    creation_activity_delay: i32,
    creation_by_map: bool,
    creation_rally_point: Option<Vec<CPos>>,
    notify: Cell<bool>,
    airborne: Cell<bool>,
    cruising: Cell<bool>,
    airborne_token: Cell<i32>,
    cruising_token: Cell<i32>,
    movement_types: Cell<u8>,
    cached_position: Cell<WPos>,
    cached_facing: Cell<WAngle>,
}

register_traits!(
    Aircraft: ITick,
    crate::orders::IIssueOrder,
    ISync,
    IFacing,
    IOccupySpace,
    IPositionable,
    IMove,
    INotifyAddedToWorld,
    INotifyRemovedFromWorld,
    INotifyActorDisposing,
    INotifyBecomingIdle,
    ICreationActivity,
    IDeathActorInitModifier,
    IResolveOrder,
    rustora_engine::sound::IOrderVoice,
    INotifyCreated,
    IObservesVariables,
    IDisabledTrait
);
info_impl!(AircraftInfo => |info, init| {
    let w = init.world();
    let location = init.inits.get::<LocationInit>().map(|l| l.value);
    let center = init.inits.get::<CenterPositionInit>().map(|c| c.value);
    let pos = center.or_else(|| location.map(|l| w.map().center_of_cell(l)));
    let facing = init.inits.get::<FacingInit>().map_or(info.initial_facing, |f| f.value);
    Some(Rc::new(Aircraft {
        state: ConditionalState::new_pausable(&info.base),
        self_: init.self_.clone(),
        repairable: OnceCell::new(),
        rearmable: OnceCell::new(),
        position_offsets: OnceCell::new(),
        override_aircraft_landing: OnceCell::new(),
        reservation: RefCell::new(None),
        speed_modifiers: OnceCell::new(),
        notify_moving: RefCell::new(Vec::new()),
        notify_center_position_changed: RefCell::new(None),
        orientation: Cell::new(WRot::NONE.with_yaw(facing)),
        center_position: Cell::new(pos.unwrap_or(WPos::ZERO)),
        reserved_actor: RefCell::new(None),
        may_yield_reservation: Cell::new(false),
        force_landing: Cell::new(false),
        landing_cells: RefCell::new(Vec::new()),
        require_force_move: Cell::new(false),
        creation_activity_delay: init.inits.get::<CreationActivityDelayInit>().map_or(0, |d| d.value),
        creation_by_map: pos.is_some() && init.inits.contains::<SpawnedByMapInit>(),
        creation_rally_point: init.inits.get::<RallyPointInit>().map(|r| r.value.clone()),
        notify: Cell::new(true),
        airborne: Cell::new(false),
        cruising: Cell::new(false),
        airborne_token: Cell::new(INVALID_CONDITION_TOKEN),
        cruising_token: Cell::new(INVALID_CONDITION_TOKEN),
        movement_types: Cell::new(movement_type::NONE),
        cached_position: Cell::new(WPos::ZERO),
        cached_facing: Cell::new(WAngle::ZERO),
        info,
    }))
});

impl ConditionalHooks for Aircraft {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl ISync for Aircraft {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.facing().hash() ^ self.center_position.get().hash()
    }
}

impl IDisabledTrait for Aircraft {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl IObservesVariables for Aircraft {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        let mut out = conditional_observers(&self);
        if let Some(e) = &self.info.require_force_move_condition {
            let w = Rc::downgrade(&self);
            let e2 = e.clone();
            out.push(VariableObserver {
                notifier: Rc::new(move |_: &ActorRef, c: &ConditionCache| {
                    if let Some(m) = w.upgrade() {
                        m.require_force_move.set(e2.evaluate(c));
                    }
                }),
                variables: e.variables().iter().cloned().collect(),
            });
        }
        out
    }
}

impl Aircraft {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.state.is_trait_paused()
    }

    pub fn require_force_move(&self) -> bool {
        self.require_force_move.get()
    }

    pub fn force_landing(&self) -> bool {
        self.force_landing.get()
    }

    pub fn may_yield_reservation(&self) -> bool {
        self.may_yield_reservation.get()
    }

    pub fn reserved_actor(&self) -> Option<ActorRef> {
        self.reserved_actor.borrow().clone()
    }

    pub fn center_position(&self) -> WPos {
        self.center_position.get()
    }

    pub fn facing(&self) -> WAngle {
        self.orientation.get().yaw
    }

    pub fn set_facing(&self, f: WAngle) {
        self.orientation.set(self.orientation.get().with_yaw(f));
    }

    pub fn pitch(&self) -> WAngle {
        self.orientation.get().pitch
    }

    pub fn set_pitch(&self, v: WAngle) {
        self.orientation.set(self.orientation.get().with_pitch(v));
    }

    pub fn roll(&self) -> WAngle {
        self.orientation.get().roll
    }

    pub fn set_roll(&self, v: WAngle) {
        self.orientation.set(self.orientation.get().with_roll(v));
    }

    pub fn turn_speed(&self) -> WAngle {
        if self.is_trait_disabled() || self.is_trait_paused() {
            WAngle::ZERO
        } else {
            self.info.turn_speed
        }
    }

    pub fn idle_turn_speed(&self) -> Option<WAngle> {
        if self.is_trait_disabled() || self.is_trait_paused() {
            None
        } else {
            self.info.idle_turn_speed
        }
    }

    pub fn get_turn_speed(&self, is_idle_turn: bool) -> WAngle {
        if (is_idle_turn && self.idle_movement_speed() == 0) || self.movement_speed() == 0 {
            return WAngle::ZERO;
        }
        let turn_speed = if is_idle_turn {
            self.idle_turn_speed().unwrap_or_else(|| self.turn_speed())
        } else {
            self.turn_speed()
        };
        WAngle::new(self.apply_speed_modifiers(turn_speed.angle).clamp(1, 1024))
    }

    fn apply_speed_modifiers(&self, n: i32) -> i32 {
        let mods = self.speed_modifiers.get().map_or(&[][..], |v| &v[..]);
        util::apply_percentage_modifiers(n, mods.iter().map(|m| m.get_speed_modifier()))
    }

    pub fn movement_speed(&self) -> i32 {
        if !self.is_trait_disabled() && !self.is_trait_paused() {
            self.apply_speed_modifiers(self.info.speed)
        } else {
            0
        }
    }

    pub fn idle_movement_speed(&self) -> i32 {
        if self.info.idle_speed < 0 {
            self.movement_speed()
        } else if !self.is_trait_disabled() && !self.is_trait_paused() {
            self.apply_speed_modifiers(self.info.idle_speed)
        } else {
            0
        }
    }

    pub fn at_land_altitude(&self) -> bool {
        self.self_.world().map().distance_above_terrain(self.get_position()) == self.land_altitude()
    }

    fn position_offsets(&self) -> &[Rc<dyn IAircraftCenterPositionOffset>] {
        self.position_offsets.get().map(Vec::as_slice).unwrap_or_default()
    }

    pub fn land_altitude(&self) -> WDist {
        let mut alt = self.info.land_altitude;
        for o in self.position_offsets() {
            alt = alt - WDist::new(o.position_offset().z);
        }
        alt
    }

    pub fn get_position(&self) -> WPos {
        let mut pos = self.self_.center_position();
        for o in self.position_offsets() {
            pos = pos + o.position_offset();
        }
        pos
    }

    fn rearmable(&self) -> Option<&Rc<Rearmable>> {
        self.rearmable.get().and_then(|r| r.as_ref())
    }

    fn repairable(&self) -> Option<&Rc<Repairable>> {
        self.repairable.get().and_then(|r| r.as_ref())
    }

    pub fn repulse(&self, self_: &ActorRef) {
        let force = self.get_repulsion_force(self_);
        if force == WVec::ZERO {
            return;
        }
        let speed = if self.info.repulsion_speed != -1 {
            self.info.repulsion_speed
        } else {
            self.movement_speed()
        };
        self.notify.set(false);
        self.set_position(self_, self.center_position.get() + self.fly_step_speed(speed, force.yaw()));
        self.notify.set(true);
    }

    pub fn get_repulsion_force(&self, self_: &ActorRef) -> WVec {
        if !self.info.repulsable {
            return WVec::ZERO;
        }
        if self.reservation.borrow().is_some() {
            let reserved = self.reserved_actor().expect("reservation without actor");
            let distance = (reserved.center_position() - self_.center_position()).horizontal_length();
            if distance < self.info.wait_distance_from_resupply_base.length {
                return WVec::ZERO;
            }
        }
        if !self.cruising.get() {
            return WVec::ZERO;
        }
        let w = self_.world();
        let mut force = WVec::ZERO;
        for actor in w.find_actors_in_circle(self_.center_position(), self.info.ideal_separation) {
            if actor.is_dead() {
                continue;
            }
            let Some(ai) = actor.info.trait_info::<AircraftInfo>() else {
                continue;
            };
            if !ai.repulsable || ai.cruise_altitude != self.info.cruise_altitude {
                continue;
            }
            force = force + self.repulsion_force_from(self_, &actor);
        }
        if !w.map().contains(self_.location()) {
            let center = WPos::lerp(w.map().projected_top_left(), w.map().projected_bottom_right(), 1, 2);
            force = force + WVec::new(0, 1024, 0).rotate(&WRot::from_yaw((self_.center_position() - center).yaw()));
        }
        if self.info.can_slide {
            return force;
        }
        let current_dir = self.fly_step(self.facing());
        let length = current_dir.horizontal_length().wrapping_mul(force.horizontal_length());
        if length == 0 {
            return WVec::ZERO;
        }
        let dot = WVec::dot(current_dir, force) / length;
        if dot >= 0 { force } else { WVec::ZERO }
    }

    fn repulsion_force_from(&self, self_: &ActorRef, other: &ActorRef) -> WVec {
        if self_.id == other.id || other.center_position().z < self_.center_position().z {
            return WVec::ZERO;
        }
        let d = self_.center_position() - other.center_position();
        let dist_sq = d.horizontal_length_squared();
        if dist_sq > self.info.ideal_separation.length_squared() {
            return WVec::ZERO;
        }
        if dist_sq < 1 {
            let yaw = self_.world().shared_random.borrow_mut().next_range(0, 1023);
            let rot = WRot::new(WAngle::ZERO, WAngle::ZERO, WAngle::new(yaw));
            return WVec::new(1024, 0, 0).rotate(&rot);
        }
        d * 1024 * 8 / (dist_sq as i32)
    }

    pub fn get_actor_below(&self) -> Option<ActorRef> {
        let w = self.self_.world();
        if w.map().distance_above_terrain(self.center_position.get()) != self.land_altitude() {
            return None;
        }
        w.actor_map()
            .get_actors_at(self.self_.location())
            .into_iter()
            .find(|a| a.info.has_trait_info::<ReservableInfo>())
    }

    pub fn make_reservation(&self, self_: &ActorRef, target: &ActorRef) {
        self.un_reserve();
        if let Some(reservable) = target.trait_or_default::<Reservable>() {
            let me = self_.trait_::<Aircraft>();
            reservable.reserve(target, self_, me);
            *self.reservation.borrow_mut() = Some(reservable);
        }
        *self.reserved_actor.borrow_mut() = Some(target.clone());
    }

    pub fn allow_yielding_reservation(&self) {
        if self.reservation.borrow().is_some() {
            self.may_yield_reservation.set(true);
        }
    }

    pub fn un_reserve(&self) {
        let Some(r) = self.reservation.borrow_mut().take() else { return };
        r.dispose_reservation();
        *self.reserved_actor.borrow_mut() = None;
        self.may_yield_reservation.set(false);
    }

    /// `AircraftCanEnter`.
    fn aircraft_can_enter(&self, self_: &ActorRef, a: &ActorRef) -> bool {
        if util::appears_hostile_to(self_, a) {
            return false;
        }
        self.rearmable().is_some_and(|r| r.info.rearm_actors.contains(&a.info.name))
            || self.repairable().is_some_and(|r| r.info.repair_actors.contains(&a.info.name))
    }

    fn aircraft_can_resupply_at(&self, self_: &ActorRef, a: &ActorRef, allowed_to_force_enter: bool) -> bool {
        if util::appears_hostile_to(self_, a) {
            return false;
        }
        let rearm = self.rearmable().filter(|r| r.info.rearm_actors.contains(&a.info.name));
        let can_repair = self.repairable().is_some_and(|r| r.info.repair_actors.contains(&a.info.name));
        let enter_rearmer = rearm.is_some_and(|r| allowed_to_force_enter || r.rearmable_ammo_pools().iter().any(|p| !p.has_full_ammo()));
        let enter_repairer = can_repair && (allowed_to_force_enter || has_damage(self_));
        enter_rearmer || enter_repairer
    }

    pub fn fly_step(&self, facing: WAngle) -> WVec {
        self.fly_step_speed(self.movement_speed(), facing)
    }

    pub fn fly_step_speed(&self, speed: i32, facing: WAngle) -> WVec {
        WVec::new(0, -1024, 0).rotate(&WRot::from_yaw(facing)) * speed / 1024
    }

    pub fn find_landing_location(&self, target_cell: CPos, max_search_distance: WDist) -> Option<CPos> {
        if self.can_land(target_cell, None, false) {
            return Some(target_cell);
        }
        let w = self.self_.world();
        let cell_range = (max_search_distance.length + 1023) / 1024;
        let center = w.map().center_of_cell(target_cell);
        for c in w.map().find_tiles_in_circle(target_cell, cell_range, false) {
            if !self.can_land(c, None, false) {
                continue;
            }
            let delta = w.map().center_of_cell(c) - center;
            if delta.length_squared() < max_search_distance.length_squared() {
                return Some(c);
            }
        }
        None
    }

    pub fn can_land_cells(&self, cells: &[CPos], docking_actor: Option<&ActorRef>, blocked_by_mobile: bool) -> bool {
        cells.iter().all(|c| self.can_land(*c, docking_actor, blocked_by_mobile))
    }

    pub fn can_land(&self, cell: CPos, docking_actor: Option<&ActorRef>, blocked_by_mobile: bool) -> bool {
        let w = self.self_.world();
        if !w.map().contains(cell) {
            return false;
        }
        for other in w.actor_map().get_actors_at(cell) {
            if self.is_blocked_by(&other, docking_actor, blocked_by_mobile) {
                return false;
            }
        }
        if docking_actor.is_some() {
            return true;
        }
        let ty = &w.map().terrain_type_info(cell).ty;
        match self
            .override_aircraft_landing
            .get()
            .and_then(|o| o.as_ref())
            .and_then(|o| o.landable_terrain_types())
        {
            Some(types) => types.contains(ty),
            None => self.info.landable_terrain_types.contains(ty),
        }
    }

    fn is_blocked_by(&self, other: &ActorRef, ignore: Option<&ActorRef>, blocked_by_mobile: bool) -> bool {
        let self_ = &self.self_;
        if other.id == self_.id || ignore.is_some_and(|i| i.id == other.id) {
            return false;
        }
        if !blocked_by_mobile
            && self_.owner().relationship_with(Some(&other.owner())) == PlayerRelationship::ALLY
            && other.trait_or_default::<Mobile>().is_some()
            && other.current_activity().is_none()
        {
            return false;
        }
        if other
            .trait_or_default::<dyn crate::interfaces::ITemporaryBlocker>()
            .is_some_and(|t| t.can_remove_blockage(other, self_))
        {
            return false;
        }
        if self.info.crushes.is_empty() {
            return true;
        }
        for c in other.cache().crushables.clone() {
            if c.crushable_by_actor(other, self_, self.info.crushes) {
                return false;
            }
        }
        true
    }

    pub fn can_rearm_at(&self, host: &ActorRef) -> bool {
        self.rearmable()
            .is_some_and(|r| r.info.rearm_actors.contains(&host.info.name) && r.rearmable_ammo_pools().iter().any(|p| !p.has_full_ammo()))
    }

    pub fn can_repair_at(&self, self_: &ActorRef, host: &ActorRef) -> bool {
        self.repairable().is_some_and(|r| r.info.repair_actors.contains(&host.info.name)) && has_damage(self_)
    }

    pub fn finished_moving(&self, self_: &ActorRef) {
        if util::is_at_ground_level(self_) {
            self.crush_action(self_, true);
        }
    }

    pub fn entering_cell(&self, self_: &ActorRef) {
        self.crush_action(self_, false);
    }

    fn crush_action(&self, self_: &ActorRef, on_crush: bool) {
        let w = self_.world();
        for a in w.actor_map().get_actors_at(self.top_left()) {
            if a.id == self_.id {
                continue;
            }
            for c in a.cache().crushables.clone() {
                if c.crushable_by_actor(&a, self_, self.info.crushes) && util::is_at_ground_level(&a) {
                    for n in a.traits_implementing::<dyn INotifyCrushed>() {
                        if on_crush {
                            n.on_crush(&a, self_, self.info.crushes);
                        } else {
                            n.warn_crush(&a, self_, self.info.crushes);
                        }
                    }
                }
            }
        }
    }

    pub fn add_influence(&self, landing_cell: CPos) {
        assert!(
            !self.has_influence(),
            "Cannot AddInfluence until previous influence is removed with RemoveInfluence"
        );
        *self.landing_cells.borrow_mut() = vec![(landing_cell, FULL_CELL)];
        if self.self_.is_in_world() {
            self.self_.world().actor_map().add_influence(&self.self_, self);
        }
    }

    pub fn remove_influence(&self) {
        if self.self_.is_in_world() {
            self.self_.world().actor_map().remove_influence(&self.self_, self);
        }
        self.landing_cells.borrow_mut().clear();
    }

    pub fn has_influence(&self) -> bool {
        !self.landing_cells.borrow().is_empty()
    }

    fn on_airborne_altitude_reached(&self, self_: &ActorRef) {
        if self.airborne.replace(true) {
            return;
        }
        if self.airborne_token.get() == INVALID_CONDITION_TOKEN {
            self.airborne_token
                .set(self_.grant_condition(self.info.airborne_condition.as_deref().unwrap_or("")));
        }
    }

    fn on_airborne_altitude_left(&self, self_: &ActorRef) {
        if !self.airborne.replace(false) {
            return;
        }
        if self.airborne_token.get() != INVALID_CONDITION_TOKEN {
            self.airborne_token.set(self_.revoke_condition(self.airborne_token.get()));
        }
    }

    fn on_cruising_altitude_reached(&self, self_: &ActorRef) {
        if self.cruising.replace(true) {
            return;
        }
        if self.cruising_token.get() == INVALID_CONDITION_TOKEN {
            self.cruising_token
                .set(self_.grant_condition(self.info.cruising_condition.as_deref().unwrap_or("")));
        }
    }

    fn on_cruising_altitude_left(&self, self_: &ActorRef) {
        if !self.cruising.replace(false) {
            return;
        }
        if self.cruising_token.get() != INVALID_CONDITION_TOKEN {
            self.cruising_token.set(self_.revoke_condition(self.cruising_token.get()));
        }
    }
}

impl INotifyCreated for Aircraft {
    fn created(&self, self_: &ActorRef) {
        let _ = self.repairable.set(self_.trait_or_default::<Repairable>());
        let _ = self.rearmable.set(self_.trait_or_default::<Rearmable>());
        let _ = self
            .position_offsets
            .set(self_.traits_implementing::<dyn IAircraftCenterPositionOffset>());
        let _ = self
            .override_aircraft_landing
            .set(self_.trait_or_default::<dyn IOverrideAircraftLanding>());
        let _ = self.speed_modifiers.set(self_.traits_implementing::<dyn ISpeedModifier>());
        self.cached_position.set(self_.center_position());
        *self.notify_moving.borrow_mut() = self_.traits_implementing::<dyn INotifyMoving>();
        *self.notify_center_position_changed.borrow_mut() = Some(self_.traits_implementing::<dyn INotifyCenterPositionChanged>());
        conditional_created(self, self_);
    }
}

impl INotifyAddedToWorld for Aircraft {
    fn added_to_world(&self, self_: &ActorRef) {
        let w = self_.world();
        w.add_to_maps(self_, self);
        let altitude = w.map().distance_above_terrain(self.center_position.get());
        if altitude.length >= self.info.min_airborne_altitude {
            self.on_airborne_altitude_reached(self_);
        }
        if altitude == self.info.cruise_altitude {
            self.on_cruising_altitude_reached(self_);
        }
    }
}

impl INotifyRemovedFromWorld for Aircraft {
    fn removed_from_world(&self, self_: &ActorRef) {
        self.un_reserve();
        self_.world().remove_from_maps(self_, self);
        self.on_cruising_altitude_left(self_);
        self.on_airborne_altitude_left(self_);
    }
}

impl INotifyActorDisposing for Aircraft {
    fn disposing(&self, _self_: &ActorRef) {
        self.un_reserve();
    }
}

impl ITick for Aircraft {
    fn tick(&self, self_: &ActorRef) {
        let current = self_.current_activity().map(|a| a.type_name());
        if !self.force_landing.get()
            && self.is_trait_paused()
            && self.airborne.get()
            && self.can_land(self_.location(), None, true)
            && !matches!(current, Some("Land") | Some("Turn"))
        {
            self_.queue_activity_queued(false, Land::new(self_));
            self.force_landing.set(true);
        }
        if self.force_landing.get() && !self.is_trait_paused() && !self.cruising.get() && current != Some("TakeOff") {
            self.force_landing.set(false);
            if self.info.idle_behavior != IdleBehaviorType::Land {
                self_.queue_activity_queued(false, TakeOff::new(self_));
            }
        }
        let old_facing = self.cached_facing.replace(self.facing());
        let old_position = self.cached_position.replace(self_.center_position());
        let mut types = movement_type::NONE;
        if old_facing != self.facing() {
            types |= movement_type::TURN;
        }
        let delta = old_position - self.cached_position.get();
        if delta.horizontal_length_squared() != 0 {
            types |= movement_type::HORIZONTAL;
        }
        if delta.vertical_length_squared() != 0 {
            types |= movement_type::VERTICAL;
        }
        self.set_current_movement_types(types);
        if types & movement_type::HORIZONTAL == 0 {
            if self.info.roll != WAngle::ZERO && self.roll() != WAngle::ZERO {
                self.set_roll(util::tick_facing(self.roll(), WAngle::ZERO, self.info.roll_speed));
            }
            if self.info.pitch != WAngle::ZERO && self.pitch() != WAngle::ZERO {
                self.set_pitch(util::tick_facing(self.pitch(), WAngle::ZERO, self.info.pitch_speed));
            }
        }
        self.repulse(self_);
    }
}

impl IFacing for Aircraft {
    fn turn_speed(&self) -> WAngle {
        Aircraft::turn_speed(self)
    }

    fn facing(&self) -> WAngle {
        Aircraft::facing(self)
    }

    fn set_facing(&self, f: WAngle) {
        Aircraft::set_facing(self, f);
    }

    fn orientation(&self) -> WRot {
        self.orientation.get()
    }
}

impl IOccupySpace for Aircraft {
    fn center_position(&self) -> WPos {
        self.center_position.get()
    }

    fn top_left(&self) -> CPos {
        self.self_.world().map().cell_containing(self.center_position.get())
    }

    fn occupied_cells(&self) -> Vec<(CPos, u8)> {
        self.landing_cells.borrow().clone()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl IPositionable for Aircraft {
    fn can_exist_in_cell(&self, _location: CPos) -> bool {
        true
    }

    fn can_enter_cell(&self, _location: CPos, _ignore_actor: Option<&ActorRef>, _check: BlockedByActor) -> bool {
        true
    }

    fn get_valid_subcell(&self, _preferred: u8) -> u8 {
        INVALID
    }

    fn get_available_subcell(&self, _location: CPos, _preferred: u8, _ignore_actor: Option<&ActorRef>, _check: BlockedByActor) -> u8 {
        INVALID
    }

    fn set_position_cell(&self, self_: &ActorRef, cell: CPos, _sub: u8) {
        let c = self_.world().map().center_of_cell(cell);
        self.set_position(self_, c + WVec::new(0, 0, self.center_position.get().z));
    }

    fn set_position(&self, self_: &ActorRef, pos: WPos) {
        self.center_position.set(pos);
        if !self_.is_in_world() {
            return;
        }
        let w = self_.world();
        let altitude = w.map().distance_above_terrain(pos);
        if self.has_influence() && altitude.length <= self.info.min_airborne_altitude {
            let current = vec![(self.top_left(), FULL_CELL)];
            // C# compares with `SequenceEqual` here, so the influence only moves when it would not change.
            if *self.landing_cells.borrow() == current {
                w.actor_map().remove_influence(self_, self);
                *self.landing_cells.borrow_mut() = current;
                w.actor_map().add_influence(self_, self);
            }
        }
        w.update_maps(self_);
        let is_airborne = altitude.length >= self.info.min_airborne_altitude;
        if is_airborne && !self.airborne.get() {
            self.on_airborne_altitude_reached(self_);
        } else if !is_airborne && self.airborne.get() {
            self.on_airborne_altitude_left(self_);
        }
        let is_cruising = altitude == self.info.cruise_altitude;
        if is_cruising && !self.cruising.get() {
            self.on_cruising_altitude_reached(self_);
        } else if !is_cruising && self.cruising.get() {
            self.on_cruising_altitude_left(self_);
        }
        if self.notify.get() {
            let list = self.notify_center_position_changed.borrow().clone();
            for n in list.iter().flatten() {
                n.center_position_changed(self_, 0, 0);
            }
        }
        self.finished_moving(self_);
    }

    fn set_center_position(&self, self_: &ActorRef, pos: WPos) {
        self.set_position(self_, pos);
    }
}

impl IMove for Aircraft {
    fn move_to(&self, cell: CPos, near_enough: i32, _ignore_actor: Option<ActorRef>, _evaluate_nearest_movable_cell: bool) -> ActivityRef {
        let w = self.self_.world();
        Fly::full(
            &self.self_,
            Target::from_cell(&w, cell, FULL_CELL),
            FlyArgs {
                near_enough: WDist::from_cells(near_enough),
                ..Default::default()
            },
        )
    }

    fn move_within_range(&self, target: &Target, min_range: WDist, max_range: WDist, initial_target_position: Option<WPos>) -> ActivityRef {
        Fly::full(
            &self.self_,
            target.clone(),
            FlyArgs {
                min_range,
                max_range,
                initial: initial_target_position,
                ..Default::default()
            },
        )
    }

    fn move_follow(
        &self,
        self_: &ActorRef,
        target: &Target,
        min_range: WDist,
        max_range: WDist,
        initial_target_position: Option<WPos>,
    ) -> ActivityRef {
        FlyFollow::new(self_, target, min_range, max_range, initial_target_position)
    }

    fn move_to_target(&self, self_: &ActorRef, target: &Target, initial_target_position: Option<WPos>) -> Option<ActivityRef> {
        Some(Fly::full(
            self_,
            target.clone(),
            FlyArgs {
                initial: initial_target_position,
                ..Default::default()
            },
        ))
    }

    fn return_to_cell(&self, _self_: &ActorRef) -> Option<ActivityRef> {
        None
    }

    fn move_into_target(&self, self_: &ActorRef, target: &Target) -> Option<ActivityRef> {
        Some(Land::full(self_, target.clone(), LandArgs::default()))
    }

    fn move_onto_target(&self, self_: &ActorRef, target: &Target, offset: WVec, facing: Option<WAngle>) -> ActivityRef {
        Land::full(
            self_,
            target.clone(),
            LandArgs {
                land_range: WDist::ZERO,
                offset,
                facing,
                ..Default::default()
            },
        )
    }

    fn local_move(&self, self_: &ActorRef, from: WPos, to: WPos) -> ActivityRef {
        let a = self_.clone();
        let call = crate::activities::CallFunc::new(Box::new(move || {
            a.trait_::<Aircraft>().set_center_position(&a, from);
        }));
        call.base().queue(Some(Fly::new(self_, Target::from_pos(to))));
        call
    }

    fn estimated_move_duration(&self, _self_: &ActorRef, from: WPos, to: WPos) -> i32 {
        let speed = self.movement_speed();
        if speed > 0 { (to - from).length() / speed } else { 0 }
    }

    fn nearest_moveable_cell(&self, target: CPos) -> CPos {
        target
    }

    fn current_movement_types(&self) -> u8 {
        self.movement_types.get()
    }

    fn set_current_movement_types(&self, t: u8) {
        let old = self.movement_types.replace(t);
        if old != t {
            for n in self.notify_moving.borrow().iter() {
                n.movement_type_changed(&self.self_, t);
            }
        }
    }

    fn can_enter_target_now(&self, self_: &ActorRef, target: &Target) -> bool {
        let w = self_.world();
        let target_actor = target.actor().cloned();
        for p in target.positions() {
            let blocked = w
                .actor_map()
                .get_actors_at(w.map().cell_containing(p))
                .iter()
                .any(|a| a.id != self_.id && target_actor.as_ref().is_none_or(|t| t.id != a.id));
            if blocked {
                return false;
            }
        }
        if let Some(t) = &target_actor {
            self.make_reservation(self_, t);
        }
        true
    }
}

impl INotifyBecomingIdle for Aircraft {
    fn on_becoming_idle(&self, self_: &ActorRef) {
        let w = self_.world();
        match self.info.idle_behavior {
            IdleBehaviorType::LeaveMap => {
                self_.queue_activity(FlyOffMap::new(self_, None));
                self_.queue_activity(RemoveSelf::new());
                return;
            }
            IdleBehaviorType::LeaveMapAtClosestEdge => {
                let edge = w.map().choose_closest_edge_cell(self_.location());
                self_.queue_activity(FlyOffMap::new(self_, Some(Target::from_cell(&w, edge, FULL_CELL))));
                self_.queue_activity(RemoveSelf::new());
                return;
            }
            IdleBehaviorType::ReturnToBase if self.get_actor_below().is_none() => {
                self_.queue_activity(ReturnToBase::new(self_, None, !self.info.take_off_on_resupply));
                return;
            }
            _ => {}
        }
        let dat = w.map().distance_above_terrain(self.center_position.get());
        if dat == self.land_altitude() {
            if !self.can_land(self_.location(), None, true) && self.reserved_actor().is_none() {
                self_.queue_activity(TakeOff::new(self_));
            }
            return;
        }
        if self.info.idle_behavior == IdleBehaviorType::Land && !self.info.landable_terrain_types.is_empty() {
            self_.queue_activity(Land::new(self_));
        } else {
            self_.queue_activity(FlyIdle::new(self_, -1, true));
        }
    }
}

impl IDeathActorInitModifier for Aircraft {
    fn modify_death_actor_init(&self, _self_: &ActorRef, init: &mut rustora_engine::actor_init::ActorInits) {
        init.push(Rc::new(FacingInit::new(self.facing())));
    }
}

impl ICreationActivity for Aircraft {
    fn is_trait_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn get_creation_activity(&self) -> Option<ActivityRef> {
        if self.creation_rally_point.is_some() || self.creation_activity_delay > 0 || self.creation_by_map {
            return Some(Rc::new(AssociateWithAirfieldActivity {
                base: ActivityBase::default(),
                aircraft: self.self_.trait_::<Aircraft>(),
                delay: self.creation_activity_delay,
                rally_point: self.creation_rally_point.clone(),
                creation_by_map: self.creation_by_map,
            }));
        }
        None
    }
}

impl IResolveOrder for Aircraft {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if self.is_trait_disabled() {
            return;
        }
        let w = self_.world();
        let queued = order.queued;
        match order.order_string.as_str() {
            "Move" | "Land" => {
                let is_land = order.order_string == "Land";
                if !order.target.is_valid_for(self_) {
                    return;
                }
                let cell = w.map().clamp(w.map().cell_containing(order.target.center_position()));
                if !self.info.move_into_shroud && !self_.owner().shroud().is_explored(cell) {
                    return;
                }
                if !queued {
                    self.un_reserve();
                }
                let target = Target::from_cell(&w, cell, FULL_CELL);
                let activity = if is_land {
                    Land::full(self_, target, LandArgs::default())
                } else {
                    Fly::full(
                        self_,
                        target,
                        FlyArgs {
                            near_enough: WDist::from_cells(8),
                            ..Default::default()
                        },
                    )
                };
                self_.queue_activity_queued(queued, with_line_color(activity, LINE_GREEN));
                crate::target_lines::show_target_lines(self_);
            }
            "Enter" | "ForceEnter" | "Repair" => {
                if order.target.target_type() != TargetType::Actor {
                    return;
                }
                let target_actor = order.target.actor().unwrap().clone();
                let is_force_enter = order.order_string == "ForceEnter";
                let can_resupply_at =
                    self.aircraft_can_resupply_at(self_, &target_actor, is_force_enter || !self.info.take_off_on_resupply);
                if !can_resupply_at || !is_available_for(&target_actor, self_) {
                    return;
                }
                if !queued {
                    self.un_reserve();
                }
                let force_land = is_force_enter || !self.info.take_off_on_resupply;
                self_.queue_activity_queued(queued, ReturnToBase::new(self_, Some(target_actor), force_land));
            }
            "Stop" => {
                let current = self_.current_activity().map(|a| a.type_name());
                if current == Some("Resupply") || (current == Some("ReturnToBase") && self.get_actor_below().is_some()) {
                    return;
                }
                self_.cancel_activity();
                self.un_reserve();
            }
            "ReturnToBase" => {
                let current = self_.current_activity().map(|a| a.type_name());
                if self.rearmable().is_none_or(|r| r.info.rearm_actors.is_empty())
                    || current == Some("ReturnToBase")
                    || self.get_actor_below().is_some()
                {
                    return;
                }
                if !queued {
                    self.un_reserve();
                }
                self_.queue_activity_queued(queued, ReturnToBase::new(self_, None, !self.info.take_off_on_resupply));
            }
            "Scatter" => {
                self_.queue_activity_queued(queued, Nudge::new(self_));
            }
            _ => {}
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
struct AssociateWithAirfieldActivity {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
    delay: i32,
    rally_point: Option<Vec<CPos>>,
    creation_by_map: bool,
}

impl AssociateWithAirfieldActivity {
    fn try_dock(&self, self_: &ActorRef, cpos: CPos, pos: WPos) -> Option<WPos> {
        let aircraft = &self.aircraft;
        let w = self_.world();
        let host = aircraft.get_actor_below()?;
        let exit = nearest_exit_or_default(&host, pos, None, None);
        let mut p = host.center_position();
        p = WPos::new(p.x, p.y, p.z - w.map().distance_above_terrain(p).length);
        if let Some(e) = &exit {
            p = p + e.info.spawn_offset;
            if let Some(f) = e.info.facing {
                aircraft.set_facing(f);
            }
        }
        aircraft.add_influence(cpos);
        aircraft.set_position(self_, p);
        aircraft.make_reservation(self_, &host);
        aircraft.allow_yielding_reservation();
        Some(p)
    }
}

impl Activity for AssociateWithAirfieldActivity {
    activity_common!("AssociateWithAirfieldActivity");

    fn target_line_nodes(&self, self_: &ActorRef) -> Vec<TargetLineNode> {
        chain_target_line_nodes(self.base.child_activity(), self_)
    }

    fn on_first_run(&self, self_: &ActorRef) {
        let w = self_.world();
        let aircraft = &self.aircraft;
        let cpos = self_.location();
        let pos = self_.center_position();
        if self.creation_by_map {
            if self.try_dock(self_, cpos, pos).is_some() {
                return;
            }
            let pos = WPos::new(pos.x, pos.y, pos.z - w.map().distance_above_terrain(pos).length);
            if !aircraft.info.take_off_on_creation && aircraft.can_land(cpos, None, true) {
                aircraft.add_influence(cpos);
                aircraft.set_position(self_, pos);
            } else {
                aircraft.set_position(self_, WPos::new(pos.x, pos.y, pos.z + aircraft.info.cruise_altitude.length));
            }
        } else {
            self.try_dock(self_, cpos, pos);
        }
        if self.delay > 0 {
            self.base.queue_child(Wait::new(self.delay));
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if !self.aircraft.info.take_off_on_creation || self.base.is_canceling() {
            return true;
        }
        if let Some(rally) = &self.rally_point {
            for cell in rally {
                let (a, c) = (self.aircraft.clone(), *cell);
                self.base.queue_child(AttackMoveActivity::new(
                    self_,
                    crate::attack_move::MoveOrder::To {
                        mv: a,
                        cell: c,
                        near_enough: 1,
                        ignore_actor: None,
                        evaluate_nearest_movable_cell: true,
                        line_color: None,
                    },
                    false,
                ));
            }
        }
        if !self.creation_by_map {
            self.aircraft.un_reserve();
        }
        true
    }

    fn get_targets(&self, self_: &ActorRef) -> Vec<Target> {
        self.base.child_activity().map_or_else(Vec::new, |c| c.get_targets(self_))
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ReservableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct Reservable {
    reserved_for: RefCell<Option<ActorRef>>,
    reserved_for_aircraft: RefCell<Option<Rc<Aircraft>>>,
    rally_point: OnceCell<Option<Rc<RallyPoint>>>,
}

register_traits!(Reservable: ITick, INotifyOwnerChanged, INotifySold, INotifyActorDisposing, INotifyCreated);
info_impl!(ReservableInfo => |_info, _init| Some(Rc::new(Reservable {
    reserved_for: RefCell::new(None),
    reserved_for_aircraft: RefCell::new(None),
    rally_point: OnceCell::new(),
})));

impl Reservable {
    /// `Reserve`; the C# disposable returned to the aircraft becomes [`Reservable::dispose_reservation`].
    fn reserve(&self, self_: &ActorRef, for_actor: &ActorRef, for_aircraft: Rc<Aircraft>) {
        let yields = self
            .reserved_for_aircraft
            .borrow()
            .as_ref()
            .is_some_and(|a| a.may_yield_reservation());
        if yields {
            self.un_reserve(self_);
        }
        *self.reserved_for.borrow_mut() = Some(for_actor.clone());
        *self.reserved_for_aircraft.borrow_mut() = Some(for_aircraft);
    }

    fn dispose_reservation(&self) {
        *self.reserved_for.borrow_mut() = None;
        *self.reserved_for_aircraft.borrow_mut() = None;
    }

    fn un_reserve(&self, self_: &ActorRef) {
        let Some(aircraft) = self.reserved_for_aircraft.borrow().clone() else {
            return;
        };
        if aircraft.get_actor_below().is_some_and(|b| b.id == self_.id) {
            let reserved_for = self.reserved_for.borrow().clone().expect("reserved aircraft without actor");
            let path = self
                .rally_point
                .get()
                .and_then(|r| r.as_ref())
                .map(|r| r.path.borrow().clone())
                .unwrap_or_default();
            if !path.is_empty() {
                for cell in path {
                    let a = aircraft.clone();
                    let act = AttackMoveActivity::new(
                        &reserved_for,
                        crate::attack_move::MoveOrder::To {
                            mv: a,
                            cell,
                            near_enough: 1,
                            ignore_actor: None,
                            evaluate_nearest_movable_cell: false,
                            line_color: None,
                        },
                        false,
                    );
                    reserved_for.queue_activity(act);
                }
            } else {
                reserved_for.queue_activity(TakeOff::new(&reserved_for));
            }
        }
        aircraft.un_reserve();
    }
}

impl INotifyCreated for Reservable {
    fn created(&self, self_: &ActorRef) {
        let _ = self.rally_point.set(self_.trait_or_default::<RallyPoint>());
    }
}

impl ITick for Reservable {
    fn tick(&self, self_: &ActorRef) {
        let Some(reserved_for) = self.reserved_for.borrow().clone() else {
            return;
        };
        if !Target::from_actor(Some(&reserved_for)).is_valid_for(self_) {
            let aircraft = self.reserved_for_aircraft.borrow().clone();
            if let Some(a) = aircraft {
                a.un_reserve();
            }
            self.dispose_reservation();
        }
    }
}

impl INotifyActorDisposing for Reservable {
    fn disposing(&self, self_: &ActorRef) {
        self.un_reserve(self_);
    }
}

impl INotifyOwnerChanged for Reservable {
    fn on_owner_changed(&self, self_: &ActorRef, _old_owner: &PlayerRef, _new_owner: &PlayerRef) {
        self.un_reserve(self_);
    }
}

impl INotifySold for Reservable {
    fn selling(&self, self_: &ActorRef) {
        self.un_reserve(self_);
    }

    fn sold(&self, self_: &ActorRef) {
        self.un_reserve(self_);
    }
}

/// `Activities/RemoveSelf.cs`.
#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct RemoveSelf {
    base: ActivityBase,
}

impl RemoveSelf {
    pub fn new() -> ActivityRef {
        Rc::new(RemoveSelf {
            base: ActivityBase::default(),
        })
    }
}

impl Activity for RemoveSelf {
    activity_common!("RemoveSelf");

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        self_.dispose();
        self.cancel(self_, false);
        true
    }
}

/// `FlyTick`.
pub fn fly_tick(
    self_: &ActorRef,
    aircraft: &Aircraft,
    desired_facing: WAngle,
    desired_altitude: WDist,
    move_override: WVec,
    idle_turn: bool,
) {
    let dat = self_.world().map().distance_above_terrain(aircraft.center_position());
    let mut mv = if move_override != WVec::ZERO {
        move_override
    } else if aircraft.info.can_slide {
        aircraft.fly_step(desired_facing)
    } else {
        aircraft.fly_step(aircraft.facing())
    };
    let old_facing = aircraft.facing();
    aircraft.set_facing(util::tick_facing(
        aircraft.facing(),
        desired_facing,
        aircraft.get_turn_speed(idle_turn),
    ));
    let roll = if idle_turn {
        aircraft.info.idle_roll.unwrap_or(aircraft.info.roll)
    } else {
        aircraft.info.roll
    };
    if roll != WAngle::ZERO {
        let desired_roll = if aircraft.facing() == desired_facing {
            WAngle::ZERO
        } else {
            WAngle::new(roll.angle * util::get_turn_direction(aircraft.facing(), old_facing))
        };
        aircraft.set_roll(util::tick_facing(aircraft.roll(), desired_roll, aircraft.info.roll_speed));
    }
    if aircraft.info.pitch != WAngle::ZERO {
        aircraft.set_pitch(util::tick_facing(aircraft.pitch(), aircraft.info.pitch, aircraft.info.pitch_speed));
    }
    if dat != desired_altitude || mv.z != 0 {
        let max_delta = mv.horizontal_length() * aircraft.info.maximum_pitch.tan() / 1024;
        let move_z = if mv.z != 0 { mv.z } else { desired_altitude.length - dat.length };
        let delta_z = clamp_cs(move_z, -max_delta, max_delta);
        mv = WVec::new(mv.x, mv.y, delta_z);
    }
    aircraft.set_position(self_, aircraft.center_position() + mv);
}

/// `Exts.Clamp`, which unlike `Ord::clamp` accepts `min > max`: the lower bound is checked first.
fn clamp_cs(v: i32, min: i32, max: i32) -> i32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

/// `VerticalTakeOffOrLandTick`.
pub fn vertical_take_off_or_land_tick(
    self_: &ActorRef,
    aircraft: &Aircraft,
    desired_facing: WAngle,
    desired_altitude: WDist,
    idle_turn: bool,
) -> bool {
    let turn_speed = if idle_turn {
        aircraft.idle_turn_speed().unwrap_or_else(|| aircraft.turn_speed())
    } else {
        aircraft.turn_speed()
    };
    aircraft.set_facing(util::tick_facing(aircraft.facing(), desired_facing, turn_speed));
    let dat = self_.world().map().distance_above_terrain(aircraft.center_position());
    if dat == desired_altitude {
        return false;
    }
    let max_delta = aircraft.info.altitude_velocity.length;
    let delta_z = clamp_cs(desired_altitude.length - dat.length, -max_delta, max_delta);
    aircraft.set_position(self_, aircraft.center_position() + WVec::new(0, 0, delta_z));
    true
}

pub fn calculate_turn_radius(speed: i32, turn_speed: WAngle) -> i32 {
    if turn_speed.angle > 0 { 180 * speed / turn_speed.angle } else { 0 }
}

#[derive(Default)]
pub struct FlyArgs {
    pub near_enough: WDist,
    pub min_range: WDist,
    pub max_range: WDist,
    pub initial: Option<WPos>,
}

const PREVIOUS_POSITIONS_CAPACITY: usize = 5;

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct Fly {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
    max_range: WDist,
    min_range: WDist,
    near_enough: WDist,
    target: RefCell<Target>,
    last_visible_target: RefCell<Target>,
    use_last_visible_target: Cell<bool>,
    previous_positions: RefCell<std::collections::VecDeque<WPos>>,
}

impl Fly {
    pub fn new(self_: &ActorRef, target: Target) -> ActivityRef {
        Fly::full(self_, target, FlyArgs::default())
    }

    pub fn full(self_: &ActorRef, target: Target, args: FlyArgs) -> ActivityRef {
        Rc::new(Fly {
            base: ActivityBase::default(),
            aircraft: self_.trait_::<Aircraft>(),
            max_range: args.max_range,
            min_range: args.min_range,
            near_enough: args.near_enough,
            last_visible_target: RefCell::new(initial_last_visible(self_, &target, args.initial)),
            target: RefCell::new(target),
            use_last_visible_target: Cell::new(false),
            previous_positions: RefCell::new(std::collections::VecDeque::with_capacity(PREVIOUS_POSITIONS_CAPACITY)),
        })
    }
}

impl Activity for Fly {
    activity_common!("Fly");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        let t = if self.use_last_visible_target.get() {
            &self.last_visible_target
        } else {
            &self.target
        };
        Some(t.borrow().clone())
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let aircraft = &self.aircraft;
        if aircraft.force_landing() {
            self.cancel(self_, false);
        }
        let w = self_.world();
        let dat = w.map().distance_above_terrain(aircraft.center_position());
        let is_landed = dat <= aircraft.land_altitude();
        if is_landed && aircraft.is_trait_paused() {
            return false;
        }
        if self.base.is_canceling() {
            let land_when_idle = aircraft.info.idle_behavior == IdleBehaviorType::Land;
            let skip_height_adjustment = land_when_idle
                && self_
                    .current_activity()
                    .is_some_and(|c| c.base().is_canceling() && c.base().next_activity().is_none());
            if aircraft.info.can_hover && !skip_height_adjustment && dat != aircraft.info.cruise_altitude {
                if is_landed {
                    self.base.queue_child(TakeOff::new(self_));
                } else {
                    vertical_take_off_or_land_tick(self_, aircraft, aircraft.facing(), aircraft.info.cruise_altitude, false);
                }
                return false;
            }
            return true;
        } else if is_landed {
            self.base.queue_child(TakeOff::new(self_));
            return false;
        }
        let (target, target_is_hidden_actor) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = target.clone();
        if !target_is_hidden_actor && target.target_type() == TargetType::Actor {
            *self.last_visible_target.borrow_mut() = Target::from_target_positions(&target);
        }
        let use_last = target_is_hidden_actor || !target.is_valid_for(self_);
        self.use_last_visible_target.set(use_last);
        if use_last && !self.last_visible_target.borrow().is_valid_for(self_) {
            return true;
        }
        let check_target = if use_last {
            self.last_visible_target.borrow().clone()
        } else {
            target
        };
        let pos = aircraft.get_position();
        let delta = check_target.center_position() - pos;
        let inside_max_range = self.max_range.length > 0 && check_target.is_in_range(pos, self.max_range);
        let inside_min_range = self.min_range.length > 0 && check_target.is_in_range(pos, self.min_range);
        if inside_max_range && !inside_min_range {
            return true;
        }
        let is_slider = aircraft.info.can_slide;
        let mut desired_facing = aircraft.facing();
        if delta.horizontal_length_squared() != 0 {
            let facing = delta.yaw();
            let diff = (facing.angle - desired_facing.angle).abs();
            let deadzone = aircraft.info.turn_deadzone.angle;
            if diff > deadzone && diff < 1024 - deadzone {
                desired_facing = facing;
            }
        }
        let mv = if is_slider {
            aircraft.fly_step(desired_facing)
        } else {
            aircraft.fly_step(aircraft.facing())
        };
        if inside_min_range {
            if is_slider {
                fly_tick(self_, aircraft, desired_facing, aircraft.info.cruise_altitude, -mv, false);
            } else {
                fly_tick(
                    self_,
                    aircraft,
                    desired_facing + WAngle::new(512),
                    aircraft.info.cruise_altitude,
                    mv,
                    false,
                );
            }
            return false;
        }
        {
            let prev = self.previous_positions.borrow();
            if prev.len() == PREVIOUS_POSITIONS_CAPACITY
                && (*prev.front().unwrap() - *prev.back().unwrap()).length_squared() < 4096
                && delta.horizontal_length_squared() <= self.near_enough.length_squared()
            {
                return true;
            }
        }
        if delta.horizontal_length_squared() < mv.horizontal_length_squared() {
            if is_slider || aircraft.info.vtol {
                if delta.horizontal_length_squared() != 0 {
                    let delta_move = WVec::new(delta.x, delta.y, 0);
                    fly_tick(self_, aircraft, desired_facing, dat, delta_move, false);
                }
                if dat != aircraft.info.cruise_altitude {
                    vertical_take_off_or_land_tick(self_, aircraft, aircraft.facing(), aircraft.info.cruise_altitude, false);
                    return false;
                }
            }
            return true;
        }
        if !is_slider {
            let turn_radius = calculate_turn_radius(aircraft.movement_speed(), aircraft.turn_speed());
            let turn_center_facing = aircraft.facing() + WAngle::new(util::get_turn_direction(aircraft.facing(), desired_facing) * 256);
            let turn_center_dir = WVec::new(0, -1024, 0).rotate(&WRot::from_yaw(turn_center_facing)) * turn_radius / 1024;
            let turn_center = aircraft.center_position() + turn_center_dir;
            if (check_target.center_position() - turn_center).horizontal_length_squared() < (turn_radius.wrapping_mul(turn_radius)) as i64 {
                desired_facing = aircraft.facing();
            }
        }
        {
            let mut prev = self.previous_positions.borrow_mut();
            if prev.len() == PREVIOUS_POSITIONS_CAPACITY {
                prev.pop_front();
            }
            prev.push_back(self_.center_position());
        }
        fly_tick(self_, aircraft, desired_facing, aircraft.info.cruise_altitude, WVec::ZERO, false);
        false
    }

    fn get_targets(&self, _self_: &ActorRef) -> Vec<Target> {
        vec![self.target.borrow().clone()]
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct FlyForward {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
    cruise_altitude: WDist,
    fly_ticks: i32,
    remaining_distance: Cell<i32>,
    ticks: Cell<i32>,
}

impl FlyForward {
    pub fn new_ticks(self_: &ActorRef, ticks: i32) -> ActivityRef {
        Self::make(self_, ticks, 0)
    }

    pub fn new_distance(self_: &ActorRef, distance: WDist) -> ActivityRef {
        Self::make(self_, 0, distance.length)
    }

    fn make(self_: &ActorRef, ticks: i32, distance: i32) -> ActivityRef {
        let aircraft = self_.trait_::<Aircraft>();
        Rc::new(FlyForward {
            base: ActivityBase::default(),
            cruise_altitude: aircraft.info.cruise_altitude,
            aircraft,
            fly_ticks: ticks,
            remaining_distance: Cell::new(distance),
            ticks: Cell::new(0),
        })
    }
}

impl Activity for FlyForward {
    activity_common!("FlyForward");

    fn tick(&self, self_: &ActorRef) -> bool {
        let aircraft = &self.aircraft;
        if aircraft.force_landing() {
            self.cancel(self_, false);
            return true;
        }
        if self.base.is_canceling() || self.fly_ticks_elapsed() || (self.fly_ticks == 0 && self.remaining_distance.get() <= 0) {
            return true;
        }
        if self.remaining_distance.get() != 0 {
            self.remaining_distance
                .set(self.remaining_distance.get() - aircraft.fly_step(aircraft.facing()).horizontal_length());
        }
        fly_tick(self_, aircraft, aircraft.facing(), self.cruise_altitude, WVec::ZERO, false);
        false
    }
}

impl FlyForward {
    /// `flyTicks > 0 && ticks++ >= flyTicks`.
    fn fly_ticks_elapsed(&self) -> bool {
        if self.fly_ticks <= 0 {
            return false;
        }
        let t = self.ticks.get();
        self.ticks.set(t + 1);
        t >= self.fly_ticks
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct FlyIdle {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
    tick_idles: Option<Vec<Rc<dyn INotifyIdle>>>,
    idle_turn: bool,
    remaining_ticks: Cell<i32>,
    is_idle_turner: bool,
}

impl FlyIdle {
    pub fn new(self_: &ActorRef, ticks: i32, idle_turn: bool) -> ActivityRef {
        let aircraft = self_.trait_::<Aircraft>();
        let is_idle_turner = aircraft.info.idle_speed > 0 || (!aircraft.info.can_hover && aircraft.info.idle_speed < 0);
        Rc::new(FlyIdle {
            base: ActivityBase::default(),
            aircraft,
            tick_idles: idle_turn.then(|| self_.traits_implementing::<dyn INotifyIdle>()),
            idle_turn,
            remaining_ticks: Cell::new(ticks),
            is_idle_turner,
        })
    }
}

impl Activity for FlyIdle {
    activity_common!("FlyIdle");

    fn tick(&self, self_: &ActorRef) -> bool {
        let r = self.remaining_ticks.get();
        if r == 0 || (self.base.next_activity().is_some() && r < 0) {
            return true;
        }
        if self.aircraft.force_landing() || self.base.is_canceling() {
            return true;
        }
        if r > 0 {
            self.remaining_ticks.set(r - 1);
        }
        for t in self.tick_idles.iter().flatten() {
            t.tick_idle(self_);
        }
        if self.is_idle_turner {
            let aircraft = &self.aircraft;
            let mv = aircraft.fly_step_speed(aircraft.idle_movement_speed(), aircraft.facing());
            let desired = aircraft.facing() + WAngle::new(256);
            fly_tick(self_, aircraft, desired, aircraft.info.cruise_altitude, mv, self.idle_turn);
        }
        false
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct FlyOffMap {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
    target: Option<Target>,
    ending_delay: Cell<i32>,
}

impl FlyOffMap {
    pub fn new(self_: &ActorRef, target: Option<Target>) -> ActivityRef {
        let base = ActivityBase::default();
        base.child_has_priority.set(false);
        Rc::new(FlyOffMap {
            base,
            aircraft: self_.trait_::<Aircraft>(),
            target,
            ending_delay: Cell::new(25),
        })
    }
}

impl Activity for FlyOffMap {
    activity_common!("FlyOffMap");

    fn on_first_run(&self, self_: &ActorRef) {
        if let Some(t) = &self.target {
            self.base.queue_child(Fly::new(self_, t.clone()));
            self.base.queue_child(FlyForward::new_ticks(self_, -1));
            return;
        }
        let aircraft = &self.aircraft;
        if aircraft.info.vtol && self_.world().map().distance_above_terrain(aircraft.center_position()) != aircraft.info.cruise_altitude {
            self.base.queue_child(TakeOff::new(self_));
        }
        self.base.queue_child(FlyForward::new_ticks(self_, -1));
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.aircraft.force_landing() {
            self.cancel(self_, false);
        }
        if self.base.is_canceling() {
            return true;
        }
        if !self_.world().map().contains(self_.location()) {
            let d = self.ending_delay.get() - 1;
            self.ending_delay.set(d);
            if d < 0
                && let Some(c) = self.base.child_activity()
            {
                c.cancel(self_, false);
            }
        }
        self.base.tick_child(self_)
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct FlyFollow {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
    min_range: WDist,
    max_range: WDist,
    target: RefCell<Target>,
    last_visible_target: RefCell<Target>,
    was_moving_within_range: Cell<bool>,
}

impl FlyFollow {
    pub fn new(self_: &ActorRef, target: &Target, min_range: WDist, max_range: WDist, initial: Option<WPos>) -> ActivityRef {
        Rc::new(FlyFollow {
            base: ActivityBase::default(),
            aircraft: self_.trait_::<Aircraft>(),
            min_range,
            max_range,
            target: RefCell::new(target.clone()),
            last_visible_target: RefCell::new(initial_last_visible(self_, target, initial)),
            was_moving_within_range: Cell::new(false),
        })
    }
}

impl Activity for FlyFollow {
    activity_common!("FlyFollow");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        Some(self.target.borrow().clone())
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let aircraft = &self.aircraft;
        if aircraft.force_landing() {
            self.cancel(self_, false);
        }
        if self.base.is_canceling() {
            return true;
        }
        let (target, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = target.clone();
        if !hidden && target.target_type() == TargetType::Actor {
            *self.last_visible_target.borrow_mut() = Target::from_target_positions(&target);
        }
        let use_last = hidden || !target.is_valid_for(self_);
        if self.was_moving_within_range.get() && hidden {
            return true;
        }
        self.was_moving_within_range.set(false);
        if use_last && !self.last_visible_target.borrow().is_valid_for(self_) {
            return true;
        }
        let pos = self_.center_position();
        let check_target = if use_last {
            self.last_visible_target.borrow().clone()
        } else {
            target.clone()
        };
        if check_target.is_in_range(pos, self.max_range) && !check_target.is_in_range(pos, self.min_range) {
            if !aircraft.info.can_hover {
                fly_tick(self_, aircraft, aircraft.facing(), aircraft.info.cruise_altitude, WVec::ZERO, false);
            }
            return use_last;
        }
        self.was_moving_within_range.set(true);
        self.base
            .queue_child(aircraft.move_within_range(&target, self.min_range, self.max_range, Some(check_target.center_position())));
        false
    }
}

pub struct LandArgs {
    /// A negative range selects `AircraftInfo.LandRange`.
    pub land_range: WDist,
    pub offset: WVec,
    pub facing: Option<WAngle>,
    pub clear_cells: Vec<CPos>,
}

impl Default for LandArgs {
    fn default() -> Self {
        LandArgs {
            land_range: WDist::new(-1),
            offset: WVec::ZERO,
            facing: None,
            clear_cells: Vec::new(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct Land {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
    offset: WVec,
    desired_facing: Option<WAngle>,
    assign_target_on_first_run: bool,
    clear_cells: Vec<CPos>,
    land_range: WDist,
    target: RefCell<Target>,
    landing_initiated: Cell<bool>,
    finished_approach: Cell<bool>,
}

impl Land {
    pub fn new(self_: &ActorRef) -> ActivityRef {
        Self::make(self_, Target::Invalid, LandArgs::default(), true)
    }

    pub fn full(self_: &ActorRef, target: Target, args: LandArgs) -> ActivityRef {
        Self::make(self_, target, args, false)
    }

    fn make(self_: &ActorRef, target: Target, args: LandArgs, assign_target_on_first_run: bool) -> ActivityRef {
        let aircraft = self_.trait_::<Aircraft>();
        let land_range = if args.land_range.length >= 0 {
            args.land_range
        } else {
            aircraft.info.land_range
        };
        let desired_facing = if args.facing.is_none() && aircraft.info.turn_to_land {
            Some(aircraft.info.initial_facing)
        } else {
            args.facing
        };
        Rc::new(Land {
            base: ActivityBase::default(),
            aircraft,
            offset: args.offset,
            desired_facing,
            assign_target_on_first_run,
            clear_cells: args.clear_cells,
            land_range,
            target: RefCell::new(target),
            landing_initiated: Cell::new(false),
            finished_approach: Cell::new(false),
        })
    }
}

impl Activity for Land {
    activity_common!("Land");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        Some(self.target.borrow().clone())
    }

    fn on_first_run(&self, self_: &ActorRef) {
        if self.assign_target_on_first_run {
            let w = self_.world();
            *self.target.borrow_mut() = Target::from_cell(&w, self_.location(), FULL_CELL);
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let aircraft = &self.aircraft;
        let w = self_.world();
        if self.base.is_canceling() || self.target.borrow().target_type() == TargetType::Invalid {
            if self.landing_initiated.get() {
                let should_land = aircraft.info.idle_behavior == IdleBehaviorType::Land;
                let continue_landing = should_land
                    && self_
                        .current_activity()
                        .is_some_and(|c| c.base().is_canceling() && c.base().next_activity().is_none());
                if !continue_landing {
                    let dat = w.map().distance_above_terrain(aircraft.center_position());
                    if dat > aircraft.land_altitude() && dat < aircraft.info.cruise_altitude {
                        self.base.queue_child(TakeOff::new(self_));
                        return false;
                    }
                    aircraft.remove_influence();
                    return true;
                }
            } else {
                return true;
            }
        }
        let pos = aircraft.get_position();
        let mut target_position = self.target.borrow().center_position() + self.offset;
        let mut landing_cell = w.map().cell_containing(target_position);
        if (target_position - pos).length_squared() == 0 {
            return true;
        }
        if self.target.borrow().target_type() == TargetType::Terrain && !self.landing_initiated.get() {
            let Some(new_location) = aircraft.find_landing_location(landing_cell, self.land_range) else {
                self.base.queue_child(aircraft.move_to(landing_cell, 0, None, false));
                return true;
            };
            if new_location != landing_cell {
                *self.target.borrow_mut() = Target::from_cell(&w, new_location, FULL_CELL);
                target_position = self.target.borrow().center_position() + self.offset;
                landing_cell = w.map().cell_containing(target_position);
                if (target_position - pos).length_squared() == 0 {
                    return true;
                }
            }
        }
        if aircraft.info.vtol {
            if (pos - target_position).horizontal_length_squared() != 0 {
                self.base.queue_child(Fly::new(self_, Target::from_pos(target_position)));
                return false;
            }
            if let Some(f) = self.desired_facing
                && f != aircraft.facing()
            {
                self.base.queue_child(Turn::new(self_, f));
                return false;
            }
        }
        if !aircraft.info.vtol && !self.finished_approach.get() {
            self.queue_approach(self_, target_position);
            return false;
        }
        if !self.landing_initiated.get() {
            let mut blocking = self.clear_cells.clone();
            blocking.push(landing_cell);
            let docking = self.target.borrow().actor().cloned();
            if !aircraft.can_land_cells(&blocking, docking.as_ref(), true) {
                self.base.queue_child(FlyIdle::new(self_, 25, true));
                notify_blocker_cells(self_, &blocking);
                self.finished_approach.set(false);
                return false;
            }
            aircraft.add_influence(landing_cell);
            aircraft.entering_cell(self_);
            self.landing_initiated.set(true);
            rustora_engine::sound::play_random_at(
                &w,
                rustora_engine::sound::SoundType::World,
                &aircraft.info.landing_sounds,
                aircraft.center_position(),
                1.0,
            );
        }
        if aircraft.info.vtol {
            let land_altitude = w.map().distance_above_terrain(target_position) + aircraft.land_altitude();
            return !vertical_take_off_or_land_tick(self_, aircraft, aircraft.facing(), land_altitude, false);
        }
        let d = target_position - pos;
        let mv = aircraft.fly_step(aircraft.facing());
        if d.horizontal_length_squared() < mv.horizontal_length_squared() {
            let landing_alt_vec = WVec::from_dists(WDist::ZERO, WDist::ZERO, aircraft.land_altitude());
            aircraft.set_position(self_, target_position + landing_alt_vec);
            return true;
        }
        let landing_alt = w.map().distance_above_terrain(target_position) + aircraft.land_altitude();
        fly_tick(self_, aircraft, d.yaw(), landing_alt, WVec::ZERO, false);
        false
    }
}

impl Land {
    fn queue_approach(&self, self_: &ActorRef, target_position: WPos) {
        let aircraft = &self.aircraft;
        let altitude = aircraft.info.cruise_altitude.length;
        let land_distance = altitude * 1024 / aircraft.info.maximum_pitch.tan();
        let rotation = self.desired_facing.map_or(WRot::NONE, WRot::from_yaw);
        let approach_start = target_position + WVec::new(0, land_distance, altitude).rotate(&rotation);
        let speed = aircraft.movement_speed() * 32 / 35;
        let mut turn_radius = calculate_turn_radius(speed, aircraft.turn_speed());
        let angle = aircraft.facing();
        let fwd = -WVec::new(angle.sin(), angle.cos(), 0);
        let side = WVec::new(-fwd.y, fwd.x, fwd.z);
        let approach_delta = self_.center_position() - approach_start;
        let side_toward_base = if WVec::dot(-side, approach_delta) < WVec::dot(side, approach_delta) {
            -side
        } else {
            side
        };
        let cp = self_.center_position() + side_toward_base * turn_radius / 1024;
        let pos_center = WPos::new(cp.x, cp.y, altitude);
        let approach_center = approach_start + WVec::new(0, turn_radius * (self_.center_position().y - approach_start.y).signum(), 0);
        let tangent_direction = approach_center - pos_center;
        let tangent_length = tangent_direction.length();
        let mut tangent_offset = WVec::ZERO;
        if tangent_length != 0 {
            tangent_offset = WVec::new(-tangent_direction.y, tangent_direction.x, 0) * turn_radius / tangent_length;
        }
        if tangent_offset.x > 0 {
            tangent_offset = -tangent_offset;
        }
        let w1 = pos_center + tangent_offset;
        let w2 = approach_center + tangent_offset;
        let w3 = approach_start;
        turn_radius = calculate_turn_radius(aircraft.info.speed, aircraft.turn_speed());
        self.base.queue_child(Fly::full(
            self_,
            Target::from_pos(w1),
            FlyArgs {
                max_range: WDist::new(turn_radius * 3),
                ..Default::default()
            },
        ));
        self.base.queue_child(Fly::new(self_, Target::from_pos(w2)));
        self.base.queue_child(Fly::full(
            self_,
            Target::from_pos(w3),
            FlyArgs {
                max_range: WDist::new(turn_radius / 2),
                ..Default::default()
            },
        ));
        self.finished_approach.set(true);
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct TakeOff {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
}

impl TakeOff {
    pub fn new(self_: &ActorRef) -> ActivityRef {
        Rc::new(TakeOff {
            base: ActivityBase::default(),
            aircraft: self_.trait_::<Aircraft>(),
        })
    }
}

impl Activity for TakeOff {
    activity_common!("TakeOff");

    fn on_first_run(&self, self_: &ActorRef) {
        if self.aircraft.force_landing() || !self.aircraft.has_influence() {
            return;
        }
        self.aircraft.remove_influence();
        rustora_engine::sound::play_random_at(
            &self_.world(),
            rustora_engine::sound::SoundType::World,
            &self.aircraft.info.takeoff_sounds,
            self.aircraft.center_position(),
            1.0,
        );
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let aircraft = &self.aircraft;
        if aircraft.force_landing() {
            self.cancel(self_, false);
            return true;
        }
        let dat = self_.world().map().distance_above_terrain(aircraft.center_position());
        if dat < aircraft.info.cruise_altitude {
            if aircraft.info.vtol {
                vertical_take_off_or_land_tick(self_, aircraft, aircraft.facing(), aircraft.info.cruise_altitude, false);
                return false;
            }
            fly_tick(self_, aircraft, aircraft.facing(), aircraft.info.cruise_altitude, WVec::ZERO, false);
            return false;
        }
        true
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct ReturnToBase {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
    repairable_info: Option<Rc<RepairableInfo>>,
    rearmable: Option<Rc<Rearmable>>,
    always_land: bool,
    dest: RefCell<Option<ActorRef>>,
}

impl ReturnToBase {
    pub fn new(self_: &ActorRef, dest: Option<ActorRef>, always_land: bool) -> ActivityRef {
        Rc::new(ReturnToBase {
            base: ActivityBase::default(),
            aircraft: self_.trait_::<Aircraft>(),
            repairable_info: self_.info.trait_info::<RepairableInfo>(),
            rearmable: self_.trait_or_default::<Rearmable>(),
            always_land,
            dest: RefCell::new(dest),
        })
    }

    pub fn choose_resupplier(self_: &ActorRef, unreserved_only: bool) -> Option<ActorRef> {
        let rearm_info = self_.info.trait_info::<RearmableInfo>()?;
        let w = self_.world();
        let candidates = w.actors_having_trait::<Reservable>().into_iter().filter(|a| {
            !a.is_dead()
                && Rc::ptr_eq(&a.owner(), &self_.owner())
                && rearm_info.rearm_actors.contains(&a.info.name)
                && (!unreserved_only || is_available_for(a, self_))
        });
        crate::util::closest_to_with_path_from(candidates, self_)
    }

    fn should_land_at_building(&self, self_: &ActorRef, dest: &ActorRef) -> bool {
        if self.always_land {
            return true;
        }
        if self
            .repairable_info
            .as_ref()
            .is_some_and(|r| r.repair_actors.contains(&dest.info.name))
            && has_damage(self_)
        {
            return true;
        }
        self.rearmable
            .as_ref()
            .is_some_and(|r| r.info.rearm_actors.contains(&dest.info.name) && r.rearmable_ammo_pools().iter().any(|p| !p.has_full_ammo()))
    }
}

impl Activity for ReturnToBase {
    activity_common!("ReturnToBase");

    fn target_line_nodes(&self, self_: &ActorRef) -> Vec<TargetLineNode> {
        if self.base.child_activity().is_some() {
            return chain_target_line_nodes(self.base.child_activity(), self_);
        }
        self.dest
            .borrow()
            .iter()
            .map(|d| TargetLineNode {
                target: Target::from_actor(Some(d)),
                color: LINE_GREEN,
            })
            .collect()
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let aircraft = &self.aircraft;
        if aircraft.force_landing() || self.base.is_canceling() || self_.is_dead() {
            return true;
        }
        let w = self_.world();
        let dest_invalid = self
            .dest
            .borrow()
            .as_ref()
            .is_none_or(|d| d.is_dead() || !is_available_for(d, self_));
        if dest_invalid {
            *self.dest.borrow_mut() = ReturnToBase::choose_resupplier(self_, true);
        }
        let dest = self.dest.borrow().clone();
        let Some(dest) = dest else {
            if let Some(nearest) = ReturnToBase::choose_resupplier(self_, false) {
                if aircraft.info.can_hover {
                    let distance = (nearest.center_position() - self_.center_position()).horizontal_length();
                    let wait = aircraft.info.wait_distance_from_resupply_base;
                    if distance > wait.length {
                        let random_position = WVec::from_pdf(&mut w.shared_random.borrow_mut(), 2) * wait.length / 1024;
                        let target = Target::from_pos(nearest.center_position() + random_position);
                        self.base.queue_child(Fly::full(
                            self_,
                            target,
                            FlyArgs {
                                max_range: wait,
                                ..Default::default()
                            },
                        ));
                    }
                    return false;
                } else {
                    self.base.queue_child(Fly::full(
                        self_,
                        Target::from_actor(Some(&nearest)),
                        FlyArgs {
                            max_range: aircraft.info.wait_distance_from_resupply_base,
                            ..Default::default()
                        },
                    ));
                    self.base
                        .queue_child(FlyIdle::new(self_, aircraft.info.number_of_ticks_to_verify_available_airport, true));
                    return false;
                }
            }
            self.base
                .queue_child(FlyIdle::new(self_, aircraft.info.number_of_ticks_to_verify_available_airport, true));
            return true;
        };
        if self.should_land_at_building(self_, &dest) {
            let exit = nearest_exit_or_default(&dest, self_.center_position(), None, None);
            let mut offset = WVec::ZERO;
            let mut facing = None;
            if let Some(e) = &exit {
                offset = e.info.spawn_offset;
                facing = e.info.facing;
            }
            aircraft.make_reservation(self_, &dest);
            self.base
                .queue_child(aircraft.move_onto_target(self_, &Target::from_actor(Some(&dest)), offset, facing));
            self.base.queue_child(Resupply::new(self_, &dest, WDist::ZERO, self.always_land));
            return true;
        }
        self.base.queue_child(Fly::new(self_, Target::from_actor(Some(&dest))));
        true
    }
}

impl crate::orders::IIssueOrder for Aircraft {
    fn orders(&self, self_: &ActorRef) -> Vec<Box<dyn crate::orders::IOrderTargeter>> {
        use crate::orders::{TargetModifiers, UnitOrderTargeter};
        let info = &self.info;
        let has_building = |t: &ActorRef| t.info.has_trait_info::<crate::building::BuildingInfo>();
        let (a1, a2, a3, a4) = (self_.clone(), self_.clone(), self_.clone(), self_.clone());
        let can_force_land = info.can_force_land;
        let force_enter_allowed = !info.take_off_on_resupply;
        let require_force_move = self.require_force_move();
        vec![
            Box::new(UnitOrderTargeter::enter_allied(
                "ForceEnter",
                6,
                &info.enter_cursor,
                &info.enter_blocked_cursor,
                has_building,
                Box::new(move |t, m| {
                    can_force_land && m.has_modifier(TargetModifiers::FORCE_MOVE) && a1.trait_::<Aircraft>().aircraft_can_enter(&a1, t)
                }),
                Box::new(move |t| is_available_for(t, &a2) && a2.trait_::<Aircraft>().aircraft_can_resupply_at(&a2, t, true)),
            )),
            Box::new(UnitOrderTargeter::enter_allied(
                "Enter",
                5,
                &info.enter_cursor,
                &info.enter_blocked_cursor,
                has_building,
                Box::new(move |t, m| {
                    (!require_force_move || m.has_modifier(TargetModifiers::FORCE_MOVE))
                        && a3.trait_::<Aircraft>().aircraft_can_enter(&a3, t)
                }),
                Box::new(move |t| {
                    is_available_for(t, &a4) && a4.trait_::<Aircraft>().aircraft_can_resupply_at(&a4, t, force_enter_allowed)
                }),
            )),
            Box::new(AircraftMoveOrderTargeter {
                aircraft: self_.trait_::<Aircraft>(),
                land: Cell::new(false),
                is_queued: Cell::new(false),
            }),
        ]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn crate::orders::IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        let id = order.order_id();
        (!self.is_trait_disabled() && matches!(id, "Enter" | "Move" | "Land" | "ForceEnter"))
            .then(|| Order::new(id, Some(self_.clone()), target.clone(), queued))
    }
}

/// `Aircraft.AircraftMoveOrderTargeter`.
struct AircraftMoveOrderTargeter {
    aircraft: Rc<Aircraft>,
    land: Cell<bool>,
    is_queued: Cell<bool>,
}

impl crate::orders::IOrderTargeter for AircraftMoveOrderTargeter {
    fn order_id(&self) -> &str {
        if self.land.get() { "Land" } else { "Move" }
    }

    fn order_priority(&self) -> i32 {
        4
    }

    fn can_target(
        &self,
        self_: &ActorRef,
        target: &Target,
        modifiers: &mut crate::orders::TargetModifiers,
        cursor: &mut Option<String>,
    ) -> bool {
        use crate::orders::TargetModifiers;
        let aircraft = &self.aircraft;
        let force_move = modifiers.has_modifier(TargetModifiers::FORCE_MOVE);
        if target.target_type() != TargetType::Terrain || (aircraft.require_force_move() && !force_move) {
            return false;
        }
        let world = self_.world();
        let location = world.map().cell_containing(target.center_position());
        if force_move && aircraft.info.can_force_land {
            let building_at_location = world.actor_map().get_actors_at(location).iter().any(|a| {
                a.trait_or_default::<crate::building::Building>().is_some()
                    && a.trait_or_default::<crate::selection::Selectable>().is_some()
            });
            if !building_at_location || aircraft.can_land(location, None, false) {
                self.land.set(true);
            }
        }
        self.is_queued.set(modifiers.has_modifier(TargetModifiers::FORCE_QUEUE));
        let explored = self_.owner().shroud().is_explored(location);
        let ok = !aircraft.is_trait_paused() && (explored || aircraft.info.move_into_shroud) && world.map().contains(location);
        *cursor = Some(if ok {
            aircraft.info.cursor.clone()
        } else {
            aircraft.info.blocked_cursor.clone()
        });
        true
    }

    fn is_queued(&self) -> bool {
        self.is_queued.get()
    }

    fn target_overrides_selection(
        &self,
        self_: &ActorRef,
        target: &Target,
        _actors_at: &[ActorRef],
        _xy: CPos,
        modifiers: crate::orders::TargetModifiers,
    ) -> bool {
        if let Some(a) = target.actor().filter(|_| target.target_type() == TargetType::Actor) {
            let selected = crate::selection::Selection::of(&self_.world()).is_some_and(|s| s.contains(a));
            if !Rc::ptr_eq(&a.owner(), &self_.owner()) || selected {
                return true;
            }
        }
        modifiers.has_modifier(crate::orders::TargetModifiers::FORCE_MOVE)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl rustora_engine::sound::IOrderVoice for Aircraft {
    fn voice_phrase_for_order(&self, self_: &ActorRef, order: &Order) -> Option<String> {
        if self.is_trait_disabled() {
            return None;
        }
        let allowed = match order.order_string.as_str() {
            "Land" | "Move" => crate::util::voice_allows_target(self_, order, self.info.move_into_shroud),
            "Enter" | "ForceEnter" | "Stop" | "Scatter" => true,
            "ReturnToBase" => self.rearmable().is_some_and(|r| !r.info.rearm_actors.is_empty()),
            _ => false,
        };
        allowed.then(|| self.info.voice.clone())
    }
}

/// Registers the save game loaders of this module.
pub(crate) fn register_persist() {
    rustora_engine::persist_activities!(
        AssociateWithAirfieldActivity,
        RemoveSelf,
        Fly,
        FlyForward,
        FlyIdle,
        FlyOffMap,
        FlyFollow,
        Land,
        TakeOff,
        ReturnToBase
    );
}
