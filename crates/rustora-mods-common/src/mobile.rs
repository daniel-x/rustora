// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Mobile.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use rustora_core::expression::BooleanExpression;
use rustora_core::world_types::{CPos, CVec, WAngle, WDist, WPos, WRot, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::actor::{ActorRef, ConditionCache};
use rustora_engine::actor_init::{CenterPositionInit, FacingInit, LocationInit, SubCellInit};
use rustora_engine::conditional::*;
use rustora_engine::order::Order;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::subcell::{ANY, FULL_CELL, INVALID};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{activity_common, info_impl, load_info, register_traits, value_init};

use crate::activities::*;
use crate::actor_map::WorldExt;
use crate::interfaces::*;
use crate::locomotor::*;
use crate::path_finder_trait::PathFinder;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("Mobile", |n| load_info!(MobileInfo, n));
}

value_init!(CreationActivityDelayInit, i32);
value_init!(RallyPointInit, Vec<CPos>);
value_init!(HuskSpeedInit, i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveResult {
    InProgress,
    CompleteCanceled,
    CompleteDestinationReached,
    CompleteDestinationBlocked,
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MobileInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    #[field(require)]
    pub locomotor: String,
    pub initial_facing: WAngle,
    pub turn_speed: WAngle,
    pub speed: i32,
    pub always_turn_in_place: bool,
    pub turns_while_moving: bool,
    pub can_move_backward: bool,
    pub backward_duration: i32,
    pub max_backward_cells: i32,
    pub require_force_move_condition: Option<BooleanExpression>,
    pub immovable_condition: Option<BooleanExpression>,
    pub terrain_orientation_adjustment_margin: WDist,
    #[field(ignore)]
    pub locomotor_info: OnceCell<Rc<LocomotorInfo>>,
}

impl Default for MobileInfo {
    fn default() -> Self {
        MobileInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            locomotor: String::new(),
            initial_facing: WAngle::ZERO,
            turn_speed: WAngle::new(512),
            speed: 1,
            always_turn_in_place: false,
            turns_while_moving: false,
            can_move_backward: false,
            backward_duration: 40,
            max_backward_cells: 15,
            require_force_move_condition: None,
            immovable_condition: None,
            terrain_orientation_adjustment_margin: WDist::new(-1),
            locomotor_info: OnceCell::new(),
        }
    }
}

impl MobileInfo {
    /// `LocomotorInfo`, resolved from the world actor's rules.
    pub fn locomotor_info(&self, w: &World) -> Rc<LocomotorInfo> {
        self.locomotor_info
            .get_or_init(|| {
                w.rules()
                    .actor("world")
                    .expect("world")
                    .trait_infos::<LocomotorInfo>()
                    .into_iter()
                    .find(|l| l.name == self.locomotor)
                    .unwrap_or_else(|| panic!("A locomotor named '{}' doesn't exist.", self.locomotor))
            })
            .clone()
    }

    /// `MobileInfo.CanEnterCell` (for use without a Mobile instance).
    pub fn can_enter_cell(&self, w: &World, self_: Option<&ActorRef>, cell: CPos, sub: u8, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> bool {
        locomotor_by_name(w, &self.locomotor).movement_cost_to_enter_cell(self_, cell, check, ignore_actor, false, sub) != MOVEMENT_COST_FOR_UNREACHABLE_CELL
    }
}

pub struct Mobile {
    pub info: Rc<MobileInfo>,
    state: ConditionalState,
    self_: ActorRef,
    me: Weak<Mobile>,
    locomotor_info: Rc<LocomotorInfo>,
    speed_modifiers: OnceCell<Vec<Rc<dyn ISpeedModifier>>>,
    return_to_cell_on_creation: bool,
    return_to_cell_on_creation_recalculate_subcell: bool,
    creation_activity_delay: i32,
    creation_rallypoint: Option<Vec<CPos>>,
    movement_types: Cell<u8>,
    old_facing: Cell<WAngle>,
    orientation: Cell<WRot>,
    old_pos: Cell<WPos>,
    pub from_subcell: Cell<u8>,
    pub to_subcell: Cell<u8>,
    notify_center_position_changed: RefCell<Option<Vec<Rc<dyn INotifyCenterPositionChanged>>>>,
    notify_moving: RefCell<Vec<Rc<dyn INotifyMoving>>>,
    notify_finished_moving: RefCell<Vec<Rc<dyn INotifyFinishedMoving>>>,
    move_wrappers: RefCell<Vec<Rc<dyn IWrapMove>>>,
    require_force_move: Cell<bool>,
    is_immovable: Cell<bool>,
    pub turn_to_move: Cell<bool>,
    is_blocking: Cell<bool>,
    pub move_result: Cell<MoveResult>,
    from_cell: Cell<CPos>,
    to_cell: Cell<CPos>,
    center_position: Cell<WPos>,
    locomotor: OnceCell<Rc<Locomotor>>,
    path_finder: OnceCell<Rc<PathFinder>>,
}

register_traits!(
    Mobile: IResolveOrder,
    IOccupySpace,
    IPositionable,
    IMove,
    ITick,
    ICreationActivity,
    IFacing,
    IDeathActorInitModifier,
    INotifyAddedToWorld,
    INotifyRemovedFromWorld,
    INotifyBlockingMove,
    INotifyBecomingIdle,
    INotifyCreated,
    IObservesVariables,
    IDisabledTrait,
    ISync
);
info_impl!(MobileInfo => |info, init| {
    let w = init.world();
    let locomotor_info = info.locomotor_info(&w);
    let mut sub = if locomotor_info.shares_cell { w.map().default_subcell() } else { FULL_CELL };
    let mut recalc = true;
    if let Some(s) = init.inits.get::<SubCellInit>() {
        sub = s.value;
        recalc = false;
    }
    let state = ConditionalState::new_pausable(&info.base);
    let facing = init.inits.get::<FacingInit>().map_or(info.initial_facing, |f| f.value);
    let m = Rc::new_cyclic(|me| Mobile {
        state,
        self_: init.self_.clone(),
        me: me.clone(),
        locomotor_info: locomotor_info.clone(),
        speed_modifiers: OnceCell::new(),
        return_to_cell_on_creation: init.inits.contains::<CenterPositionInit>(),
        return_to_cell_on_creation_recalculate_subcell: recalc,
        creation_activity_delay: init.inits.get::<CreationActivityDelayInit>().map_or(0, |d| d.value),
        creation_rallypoint: init.inits.get::<RallyPointInit>().map(|r| r.value.clone()),
        movement_types: Cell::new(0),
        old_facing: Cell::new(facing),
        orientation: Cell::new(WRot::NONE.with_yaw(facing)),
        old_pos: Cell::new(WPos::ZERO),
        from_subcell: Cell::new(sub),
        to_subcell: Cell::new(sub),
        notify_center_position_changed: RefCell::new(None),
        notify_moving: RefCell::new(Vec::new()),
        notify_finished_moving: RefCell::new(Vec::new()),
        move_wrappers: RefCell::new(Vec::new()),
        require_force_move: Cell::new(false),
        is_immovable: Cell::new(false),
        turn_to_move: Cell::new(false),
        is_blocking: Cell::new(false),
        move_result: Cell::new(MoveResult::InProgress),
        from_cell: Cell::new(CPos::ZERO),
        to_cell: Cell::new(CPos::ZERO),
        center_position: Cell::new(WPos::ZERO),
        locomotor: OnceCell::new(),
        path_finder: OnceCell::new(),
        info,
    });
    if let Some(l) = init.inits.get::<LocationInit>() {
        m.from_cell.set(l.value);
        m.to_cell.set(l.value);
        m.center_position.set(w.map().center_of_subcell(l.value, sub));
    }
    if let Some(c) = init.inits.get::<CenterPositionInit>() {
        m.old_pos.set(c.value);
        m.center_position.set(c.value);
    }
    Some(m)
});

impl ConditionalHooks for Mobile {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, self_: &ActorRef) {
        self.update_occupied(self_);
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        self.update_occupied(self_);
    }
    fn trait_resumed(&self, self_: &ActorRef) {
        self.update_occupied(self_);
    }
    fn trait_paused(&self, self_: &ActorRef) {
        self.update_occupied(self_);
    }
}

impl ISync for Mobile {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.facing().hash() ^ self.from_cell.get().hash() ^ self.to_cell.get().hash() ^ self.center_position.get().hash()
    }
}

impl IDisabledTrait for Mobile {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl IObservesVariables for Mobile {
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
        if let Some(e) = &self.info.immovable_condition {
            let w = Rc::downgrade(&self);
            let e2 = e.clone();
            out.push(VariableObserver {
                notifier: Rc::new(move |self_: &ActorRef, c: &ConditionCache| {
                    if let Some(m) = w.upgrade() {
                        let was = m.is_immovable.get();
                        m.is_immovable.set(e2.evaluate(c));
                        if was != m.is_immovable.get() {
                            m.update_occupied(self_);
                        }
                    }
                }),
                variables: e.variables().iter().cloned().collect(),
            });
        }
        out
    }
}

impl INotifyCreated for Mobile {
    fn created(&self, self_: &ActorRef) {
        *self.notify_center_position_changed.borrow_mut() = Some(self_.traits_implementing::<dyn INotifyCenterPositionChanged>());
        *self.notify_moving.borrow_mut() = self_.traits_implementing::<dyn INotifyMoving>();
        *self.notify_finished_moving.borrow_mut() = self_.traits_implementing::<dyn INotifyFinishedMoving>();
        *self.move_wrappers.borrow_mut() = self_.traits_implementing::<dyn IWrapMove>();
        let w = self_.world();
        let _ = self.path_finder.set(w.world_actor().trait_::<PathFinder>());
        let _ = self.locomotor.set(locomotor_by_name(&w, &self.info.locomotor));
        conditional_created(self, self_);
    }
}

impl ITick for Mobile {
    fn tick(&self, _self_: &ActorRef) {
        self.update_movement();
    }
}

impl IOccupySpace for Mobile {
    fn center_position(&self) -> WPos {
        self.center_position.get()
    }

    fn top_left(&self) -> CPos {
        self.to_cell.get()
    }

    fn occupied_cells(&self) -> Vec<(CPos, u8)> {
        let (f, t) = (self.from_cell.get(), self.to_cell.get());
        if f == t {
            return vec![(f, self.from_subcell.get())];
        }
        if self.locomotor_info.shares_cell {
            return vec![(t, self.to_subcell.get())];
        }
        vec![(f, self.from_subcell.get()), (t, self.to_subcell.get())]
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_leaving_cell(&self, location: CPos, sub: u8) -> bool {
        self.to_cell.get() != location
            && self.from_cell.get() == location
            && (sub == ANY || self.from_subcell.get() == sub || sub == FULL_CELL || self.from_subcell.get() == FULL_CELL)
    }
}

impl IFacing for Mobile {
    fn turn_speed(&self) -> WAngle {
        self.info.turn_speed
    }

    fn facing(&self) -> WAngle {
        self.orientation.get().yaw
    }

    fn set_facing(&self, f: WAngle) {
        self.orientation.set(self.orientation.get().with_yaw(f));
    }

    fn orientation(&self) -> WRot {
        self.orientation.get()
    }
}

impl INotifyAddedToWorld for Mobile {
    fn added_to_world(&self, self_: &ActorRef) {
        self_.world().add_to_maps(self_, self);
    }
}

impl INotifyRemovedFromWorld for Mobile {
    fn removed_from_world(&self, self_: &ActorRef) {
        self_.world().remove_from_maps(self_, self);
    }
}

impl ICreationActivity for Mobile {
    fn is_trait_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn get_creation_activity(&self) -> Option<ActivityRef> {
        if self.return_to_cell_on_creation || self.creation_rallypoint.is_some() || self.creation_activity_delay > 0 {
            let rc = self
                .return_to_cell_on_creation
                .then(|| ReturnToCellActivity::new(self.me(), self.creation_activity_delay, self.return_to_cell_on_creation_recalculate_subcell));
            return Some(Rc::new(LeaveProductionActivity {
                base: ActivityBase::default(),
                mobile: self.me(),
                delay: self.creation_activity_delay,
                rally_point: self.creation_rallypoint.clone(),
                return_to_cell: rc,
            }));
        }
        None
    }
}

impl INotifyBecomingIdle for Mobile {
    fn on_becoming_idle(&self, self_: &ActorRef) {
        if !self.locomotor().can_stay_in_cell(self_.location()) {
            self_.queue_activity(self.move_to(self_.location(), 0, None, true));
        }
    }
}

impl INotifyBlockingMove for Mobile {
    fn on_notify_blocking_move(&self, self_: &ActorRef, blocking: &ActorRef) {
        if !util::appears_friendly_to(self_, blocking) {
            return;
        }
        if self_.is_idle() {
            self_.queue_activity_queued(false, Nudge::new(blocking));
            return;
        }
        self.is_blocking.set(true);
    }
}

impl IDeathActorInitModifier for Mobile {
    fn modify_death_actor_init(&self, self_: &ActorRef, init: &mut rustora_engine::actor_init::ActorInits) {
        init.push(Rc::new(FacingInit::new(self.facing())));
        if self.can_enter_cell(self_.location(), Some(self_), BlockedByActor::Stationary) {
            init.push(Rc::new(HuskSpeedInit::new(self.movement_speed_for_cell(self_.location()))));
        }
    }
}

impl IResolveOrder for Mobile {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if self.state.is_trait_disabled() {
            return;
        }
        match order.order_string.as_str() {
            "Move" => {
                if !order.target.is_valid_for(self_) {
                    return;
                }
                let w = self_.world();
                let cell = w.map().clamp(w.map().cell_containing(order.target.center_position()));
                if !self.locomotor_info.move_into_shroud && !self_.owner().shroud().is_explored(cell) {
                    return;
                }
                self_.queue_activity_queued(order.queued, self.wrap_move(Move::new_near(self_, cell, WDist::from_cells(8), None, true)));
            }
            "Stop" => self_.cancel_activity(),
            "Scatter" => self_.queue_activity_queued(order.queued, Nudge::new(self_)),
            _ => {}
        }
    }
}

impl IPositionable for Mobile {
    fn can_exist_in_cell(&self, cell: CPos) -> bool {
        self.locomotor().movement_cost_for_cell(cell) != MOVEMENT_COST_FOR_UNREACHABLE_CELL
    }

    fn can_enter_cell(&self, cell: CPos, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> bool {
        self.locomotor()
            .movement_cost_to_enter_cell(Some(&self.self_), cell, check, ignore_actor, false, self.to_subcell.get())
            != MOVEMENT_COST_FOR_UNREACHABLE_CELL
    }

    fn get_valid_subcell(&self, preferred: u8) -> u8 {
        let preferred = if preferred == ANY { self.from_subcell.get() } else { preferred };
        if self.locomotor_info.shares_cell {
            if preferred <= FULL_CELL {
                return self.self_.world().map().default_subcell();
            }
        } else if preferred != FULL_CELL {
            return FULL_CELL;
        }
        preferred
    }

    fn get_available_subcell(&self, a: CPos, preferred: u8, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> u8 {
        self.locomotor().get_available_subcell(&self.self_, a, check, preferred, ignore_actor)
    }

    fn set_position_cell(&self, self_: &ActorRef, cell: CPos, sub: u8) {
        let sub = self.get_valid_subcell(sub);
        self.set_location(cell, sub, cell, sub);
        let w = self_.world();
        let mut position = w.map().center_of_cell(cell);
        position = position + w.map().offset_of_subcell(sub);
        position = position - WVec::new(0, 0, w.map().distance_above_terrain(position).length);
        self.set_center_position(self_, position);
        self.finished_moving(self_);
    }

    fn set_position(&self, self_: &ActorRef, pos: WPos) {
        let w = self_.world();
        let cell = w.map().cell_containing(pos);
        let sub = self.from_subcell.get();
        self.set_location(cell, sub, cell, sub);
        self.set_center_position(self_, w.map().center_of_subcell(cell, sub) + WVec::new(0, 0, w.map().distance_above_terrain(pos).length));
        self.finished_moving(self_);
    }

    fn set_center_position(&self, self_: &ActorRef, pos: WPos) {
        self.center_position.set(pos);
        self_.world().update_maps(self_);
        let n = self.notify_center_position_changed.borrow().clone();
        if let Some(n) = n {
            for x in n {
                x.center_position_changed(self_, 0, 0);
            }
        }
    }
}

impl IMove for Mobile {
    fn move_to(&self, cell: CPos, near_enough: i32, ignore_actor: Option<ActorRef>, evaluate_nearest_movable_cell: bool) -> ActivityRef {
        self.wrap_move(Move::new_near(
            &self.self_,
            cell,
            WDist::from_cells(near_enough),
            ignore_actor,
            evaluate_nearest_movable_cell,
        ))
    }

    fn move_within_range(&self, target: &Target, min_range: WDist, max_range: WDist, initial: Option<WPos>) -> ActivityRef {
        self.wrap_move(MoveAdjacentTo::new_within_range(&self.self_, target, min_range, max_range, initial))
    }

    fn move_follow(&self, self_: &ActorRef, target: &Target, min_range: WDist, max_range: WDist, initial: Option<WPos>) -> ActivityRef {
        self.wrap_move(Follow::new(self_, target, min_range, max_range, initial))
    }

    fn move_to_target(&self, self_: &ActorRef, target: &Target, initial: Option<WPos>) -> Option<ActivityRef> {
        if target.target_type() == TargetType::Invalid {
            return None;
        }
        Some(self.wrap_move(MoveAdjacentTo::new_adjacent(self_, target, initial)))
    }

    fn return_to_cell(&self, _self_: &ActorRef) -> Option<ActivityRef> {
        Some(ReturnToCellActivity::new(self.me(), 0, false))
    }

    fn move_into_target(&self, self_: &ActorRef, target: &Target) -> Option<ActivityRef> {
        if target.target_type() == TargetType::Invalid {
            return None;
        }
        Some(self.wrap_move(LocalMoveIntoTarget::new(self_, target, WDist::new(512))))
    }

    fn move_onto_target(&self, self_: &ActorRef, target: &Target, offset: WVec, facing: Option<WAngle>) -> ActivityRef {
        self.wrap_move(MoveAdjacentTo::new_onto_and_turn(self_, target, offset, facing))
    }

    fn local_move(&self, self_: &ActorRef, from: WPos, to: WPos) -> ActivityRef {
        self.wrap_move(self.local_move_inner(self_, from, to, self_.location()))
    }

    fn estimated_move_duration(&self, self_: &ActorRef, from: WPos, to: WPos) -> i32 {
        let speed = self.movement_speed_for_cell(self_.location());
        if speed > 0 { (to - from).length() / speed } else { 0 }
    }

    fn nearest_moveable_cell(&self, target: CPos) -> CPos {
        self.nearest_moveable_cell_range(target, 1, 10)
    }

    fn current_movement_types(&self) -> u8 {
        self.movement_types.get()
    }

    fn set_current_movement_types(&self, value: u8) {
        let old = self.movement_types.get();
        self.movement_types.set(value);
        if value != old {
            let self_ = &self.self_;
            self.update_occupied(self_);
            let n = self.notify_moving.borrow().clone();
            for x in n {
                x.movement_type_changed(self_, value);
            }
        }
    }

    fn can_enter_target_now(&self, self_: &ActorRef, target: &Target) -> bool {
        if let Some(f) = target.frozen_actor() {
            if target.target_type() == TargetType::FrozenActor && !f.is_valid() {
                return false;
            }
        }
        let w = self_.world();
        self_.location() == w.map().cell_containing(target.center_position()) || util::adjacent_cells(&w, target).contains(&self_.location())
    }

    fn as_mobile(&self) -> Option<&Mobile> {
        Some(self)
    }
}

impl Mobile {
    pub fn me(&self) -> Rc<Mobile> {
        self.me.upgrade().expect("mobile alive")
    }

    pub fn locomotor(&self) -> Rc<Locomotor> {
        self.locomotor.get().expect("mobile created").clone()
    }

    pub fn path_finder(&self) -> Rc<PathFinder> {
        self.path_finder.get().expect("mobile created").clone()
    }

    pub fn locomotor_info(&self) -> &Rc<LocomotorInfo> {
        &self.locomotor_info
    }

    pub fn from_cell(&self) -> CPos {
        self.from_cell.get()
    }

    pub fn to_cell(&self) -> CPos {
        self.to_cell.get()
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.state.is_trait_paused()
    }

    pub fn is_immovable(&self) -> bool {
        self.is_immovable.get()
    }

    pub fn is_blocking(&self) -> bool {
        self.is_blocking.get()
    }

    pub fn require_force_move(&self) -> bool {
        self.require_force_move.get()
    }

    /// Not disabled, not paused and not immovable.
    pub fn is_movable(&self) -> bool {
        !self.state.is_trait_disabled() && !self.state.is_trait_paused() && !self.is_immovable.get()
    }

    pub fn is_moving_between_cells(&self) -> bool {
        self.from_cell.get() != self.to_cell.get()
    }

    fn update_occupied(&self, self_: &ActorRef) {
        self_.world().actor_map().update_occupied_cells(self);
    }

    pub fn update_movement(&self) {
        let mut t = movement_type::NONE;
        let (old, cur) = (self.old_pos.get(), self.center_position.get());
        if (old - cur).horizontal_length_squared() != 0 {
            t |= movement_type::HORIZONTAL;
        }
        if old.z != cur.z {
            t |= movement_type::VERTICAL;
        }
        if self.old_facing.get() != self.facing() {
            t |= movement_type::TURN;
        }
        self.set_current_movement_types(t);
        self.old_pos.set(self.center_position.get());
        self.old_facing.set(self.facing());
    }

    pub fn get_adjacent_cell(&self, next_cell: CPos, prefer_to_avoid: Option<&dyn Fn(CPos) -> bool>) -> Option<CPos> {
        let w = self.self_.world();
        let mut avail = Vec::new();
        let mut not_stupid = Vec::new();
        for d in CVec::DIRECTIONS {
            let p = self.to_cell.get() + d;
            if self.can_enter_cell(p, None, BlockedByActor::All) && self.can_stay_in_cell(p) && prefer_to_avoid.is_none_or(|f| !f(p)) {
                avail.push(p);
            } else if p != next_cell && p != self.to_cell.get() {
                not_stupid.push(p);
            }
        }
        if !avail.is_empty() {
            let i = w.shared_random.borrow_mut().next_max(avail.len() as i32) as usize;
            return Some(avail[i]);
        }
        let mut infos: Vec<CPos> = Vec::new();
        let am = w.actor_map();
        for c in not_stupid {
            for a in am.get_actors_at(c) {
                if is_movable_actor(&a) {
                    infos.push(c);
                }
            }
        }
        if infos.is_empty() {
            return None;
        }
        let i = w.shared_random.borrow_mut().next_max(infos.len() as i32) as usize;
        Some(infos[i])
    }

    pub fn is_leaving(&self) -> bool {
        let t = self.movement_types.get();
        if t & movement_type::HORIZONTAL != 0 {
            return true;
        }
        if t & movement_type::TURN != 0 {
            return self.turn_to_move.get();
        }
        false
    }

    pub fn can_interact_with_ground_layer(&self, _self_: &ActorRef) -> bool {
        true
    }

    pub fn can_stay_in_cell(&self, cell: CPos) -> bool {
        self.locomotor().can_stay_in_cell(cell)
    }

    pub fn set_location(&self, from: CPos, from_sub: u8, to: CPos, to_sub: u8) {
        if self.from_cell.get() == from && self.to_cell.get() == to && self.from_subcell.get() == from_sub && self.to_subcell.get() == to_sub {
            return;
        }
        self.remove_influence();
        self.from_cell.set(from);
        self.to_cell.set(to);
        self.from_subcell.set(from_sub);
        self.to_subcell.set(to_sub);
        self.add_influence();
        self.is_blocking.set(false);
    }

    pub fn finished_moving(&self, self_: &ActorRef) {
        let n = self.notify_finished_moving.borrow().clone();
        for x in n {
            x.finished_moving(self_, 0, 0);
        }
        if !util::is_at_ground_level(self_) {
            return;
        }
        self.crush_action(self_, true);
    }

    fn crush_action(&self, self_: &ActorRef, on_crush: bool) {
        let w = self_.world();
        let crushes = self.locomotor_info.crushes;
        for a in w.actor_map().get_actors_at_sub(self.to_cell.get(), self.to_subcell.get()) {
            if a.id == self_.id {
                continue;
            }
            for c in a.cache().crushables.clone() {
                if c.crushable_by_actor(&a, self_, crushes) && util::is_at_ground_level(&a) {
                    for n in a.traits_implementing::<dyn INotifyCrushed>() {
                        if on_crush {
                            n.on_crush(&a, self_, crushes);
                        } else {
                            n.warn_crush(&a, self_, crushes);
                        }
                    }
                }
            }
        }
    }

    pub fn entering_cell(&self, self_: &ActorRef) {
        if !util::is_at_ground_level(self_) {
            return;
        }
        self.crush_action(self_, false);
    }

    pub fn add_influence(&self) {
        if self.self_.is_in_world() {
            self.self_.world().actor_map().add_influence(&self.self_, self);
        }
    }

    pub fn remove_influence(&self) {
        if self.self_.is_in_world() {
            self.self_.world().actor_map().remove_influence(&self.self_, self);
        }
    }

    pub fn wrap_move(&self, inner: ActivityRef) -> ActivityRef {
        let w = self.move_wrappers.borrow().iter().find(|m| m.is_trait_enabled()).cloned();
        match w {
            Some(m) => m.wrap_move(inner),
            None => inner,
        }
    }

    pub fn movement_speed_for_cell(&self, cell: CPos) -> i32 {
        let terrain = self.locomotor().movement_speed_for_cell(cell);
        let mods = self.speed_modifiers.get_or_init(|| self.self_.traits_implementing::<dyn ISpeedModifier>());
        util::apply_percentage_modifiers(self.info.speed, mods.iter().map(|m| m.get_speed_modifier()).chain(std::iter::once(terrain)))
    }

    pub fn nearest_moveable_cell_range(&self, target: CPos, min_range: i32, max_range: i32) -> CPos {
        let w = self.self_.world();
        if target == self.self_.location() && self.can_stay_in_cell(target) {
            return target;
        }
        if self.can_enter_cell(target, None, BlockedByActor::Immovable) && self.can_stay_in_cell(target) {
            return target;
        }
        for tile in w.map().find_tiles_in_annulus(target, min_range, max_range, false) {
            if self.can_enter_cell(tile, None, BlockedByActor::Immovable) && self.can_stay_in_cell(tile) {
                return tile;
            }
        }
        target
    }

    pub fn nearest_cell(&self, target: CPos, check: &dyn Fn(CPos) -> bool, min_range: i32, max_range: i32) -> CPos {
        if check(target) {
            return target;
        }
        let w = self.self_.world();
        for tile in w.map().find_tiles_in_annulus(target, min_range, max_range, false) {
            if check(tile) {
                return tile;
            }
        }
        target
    }

    pub fn move_to_path(&self, get_path: GetPathFn) -> ActivityRef {
        Move::new_func(&self.self_, get_path)
    }

    fn local_move_inner(&self, self_: &ActorRef, from: WPos, to: WPos, cell: CPos) -> ActivityRef {
        let speed = self.movement_speed_for_cell(cell);
        let length = if speed > 0 { (to - from).length() / speed } else { 0 };
        let delta = to - from;
        let facing = if delta.horizontal_length_squared() != 0 { delta.yaw() } else { self.facing() };
        Drag::new(self_, from, to, length, Some(facing))
    }
}

fn is_movable_actor(other: &ActorRef) -> bool {
    if !other.is_idle() {
        return false;
    }
    mobile_of(other).is_some_and(Mobile::is_movable)
}

/// `actor.OccupiesSpace as Mobile`.
pub fn mobile_of(a: &ActorRef) -> Option<&Mobile> {
    a.cache().occupies_space.as_ref()?.as_any().downcast_ref::<Mobile>()
}

/// `Mobile.ReturnToCellActivity`.
pub struct ReturnToCellActivity {
    base: ActivityBase,
    mobile: Rc<Mobile>,
    recalculate_subcell: bool,
    delay: i32,
}

impl ReturnToCellActivity {
    pub fn new(mobile: Rc<Mobile>, delay: i32, recalculate_subcell: bool) -> ActivityRef {
        let base = ActivityBase::default();
        base.is_interruptible.set(false);
        Rc::new(ReturnToCellActivity {
            base,
            mobile,
            recalculate_subcell,
            delay,
        })
    }
}

impl Activity for ReturnToCellActivity {
    activity_common!("ReturnToCellActivity");

    fn on_first_run(&self, self_: &ActorRef) {
        let w = self_.world();
        if w.map().distance_above_terrain(self_.center_position()).length > 0 && self_.info.has_trait_info_named("Parachutable") {
            self.base.queue_child(Parachute::new(self_));
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let w = self_.world();
        let m = &self.mobile;
        let pos = self_.center_position();
        let cell = m.to_cell();
        let mut sub = m.to_subcell.get();
        if self.recalculate_subcell {
            sub = if m.locomotor_info.shares_cell {
                let id = self_.id;
                w.actor_map().free_subcell_with(cell, sub, &|a: &ActorRef| a.id != id)
            } else {
                FULL_CELL
            };
        }
        if sub == INVALID {
            sub = w.map().default_subcell();
        }
        m.set_position_cell(self_, cell, sub);
        m.set_center_position(self_, pos);
        if self.delay > 0 {
            self.base.queue_child(Wait::new(self.delay));
        }
        self.base.queue_child(m.local_move_inner_wrapped(self_, pos, w.map().center_of_subcell(cell, sub)));
        true
    }
}

impl Mobile {
    fn local_move_inner_wrapped(&self, self_: &ActorRef, from: WPos, to: WPos) -> ActivityRef {
        self.local_move(self_, from, to)
    }
}

/// `Mobile.LeaveProductionActivity`.
pub struct LeaveProductionActivity {
    base: ActivityBase,
    mobile: Rc<Mobile>,
    delay: i32,
    rally_point: Option<Vec<CPos>>,
    return_to_cell: Option<ActivityRef>,
}

impl Activity for LeaveProductionActivity {
    activity_common!("LeaveProductionActivity");

    fn on_first_run(&self, self_: &ActorRef) {
        if let Some(r) = &self.return_to_cell {
            self.base.queue_child(r.clone());
        } else if self.delay > 0 {
            self.base.queue_child(Wait::new(self.delay));
        }
        if let Some(rp) = &self.rally_point {
            for &cell in rp {
                let m = self.mobile.clone();
                self.base
                    .queue_child(crate::attack_move::AttackMoveActivity::new(self_, Rc::new(move || m.move_to(cell, 1, None, true)), false));
            }
        }
    }
}
