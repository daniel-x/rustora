// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Carryall.cs`, `Traits/Carryable.cs`, `Traits/Buildings/FreeActorWithDelivery.cs`,
//! `Activities/PickupUnit.cs` and `Activities/DeliverUnit.cs`.
//!
//! Not ported: the preview of the carried actor in `IRender`, so a carried actor is not drawn.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use rustora_core::fieldloader::Color;
use rustora_core::world_types::{CPos, WDist, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef, LINE_YELLOW, with_line_color};
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN, hash_actor};
use rustora_engine::conditional::*;
use rustora_engine::damage::{AttackInfo, DamageTypes};
use rustora_engine::frozen::FrozenActorRef;
use rustora_engine::order::Order;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::IOrderVoice;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::traits::*;
use rustora_engine::{activity_common, info_impl, load_info, register_traits};

use crate::activities::Wait;
use crate::aircraft::{Aircraft, Fly, FlyIdle, Land, LandArgs, TakeOff};
use crate::interfaces::*;
use crate::mobile::Mobile;
use crate::orders::{DeployOrderTargeter, IIssueDeployOrder, IIssueOrder, IOrderTargeter, TargetModifiers, UnitOrderTargeter};
use crate::render::BodyOrientation;
use crate::util::appears_friendly_to;

pub fn register(r: &mut TraitRegistry) {
    r.register("Carryall", |n| load_info!(CarryallInfo, n));
    r.register("Carryable", |n| load_info!(CarryableInfo, n));
    r.register("FreeActorWithDelivery", |n| load_info!(FreeActorWithDeliveryInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CarryallInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub initial_actor: Option<String>,
    pub before_load_delay: i32,
    pub before_unload_delay: i32,
    pub local_offset: WVec,
    pub drop_range: WDist,
    pub unload_cursor: String,
    pub unload_blocked_cursor: String,
    pub allow_drop_off: bool,
    pub drop_off_cursor: String,
    pub drop_off_blocked_cursor: String,
    pub pick_up_cursor: String,
    pub carry_condition: Option<String>,
    pub carryable_conditions: IndexMap<String, String>,
    pub voice: String,
    pub target_line_color: Color,
}

impl Default for CarryallInfo {
    fn default() -> Self {
        CarryallInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            initial_actor: None,
            before_load_delay: 0,
            before_unload_delay: 0,
            local_offset: WVec::ZERO,
            drop_range: WDist::from_cells(5),
            unload_cursor: "deploy".into(),
            unload_blocked_cursor: "deploy-blocked".into(),
            allow_drop_off: false,
            drop_off_cursor: "ability".into(),
            drop_off_blocked_cursor: "move-blocked".into(),
            pick_up_cursor: "ability".into(),
            carry_condition: None,
            carryable_conditions: IndexMap::new(),
            voice: "Action".into(),
            target_line_color: LINE_YELLOW,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CarryallState {
    Idle,
    Reserved,
    Carrying,
}

/// Transports actors that have `Carryable`.
pub struct Carryall {
    pub info: Rc<CarryallInfo>,
    state: ConditionalState,
    self_: Weak<rustora_engine::actor::Actor>,
    me: Weak<Carryall>,
    carryable: RefCell<Option<ActorRef>>,
    carry_state: Cell<CarryallState>,
    landable_terrain_types: RefCell<Option<Rc<IndexSet<String>>>>,
    carry_condition_token: Cell<i32>,
    carryable_condition_token: Cell<i32>,
    carryable_offset: Cell<WVec>,
    aircraft: OnceCell<Rc<Aircraft>>,
    body: OnceCell<Rc<BodyOrientation>>,
}

register_traits!(Carryall: INotifyKilled, ITick, INotifyActorDisposing, IIssueOrder, IResolveOrder, IOrderVoice,
    IIssueDeployOrder, IAircraftCenterPositionOffset, IOverrideAircraftLanding, INotifyCreated, IObservesVariables,
    IDisabledTrait, ISync);
info_impl!(CarryallInfo => |info, init| Some(Rc::new_cyclic(|me| Carryall {
    state: ConditionalState::new(&info.base),
    info,
    self_: Rc::downgrade(&init.self_),
    me: me.clone(),
    carryable: RefCell::new(None),
    carry_state: Cell::new(CarryallState::Idle),
    landable_terrain_types: RefCell::new(None),
    carry_condition_token: Cell::new(INVALID_CONDITION_TOKEN),
    carryable_condition_token: Cell::new(INVALID_CONDITION_TOKEN),
    carryable_offset: Cell::new(WVec::ZERO),
    aircraft: OnceCell::new(),
    body: OnceCell::new(),
})));

impl ConditionalHooks for Carryall {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}
rustora_engine::conditional_observers_impl!(Carryall);

impl ISync for Carryall {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ hash_actor(self.carryable.borrow().as_ref())
    }
}

impl INotifyCreated for Carryall {
    fn created(&self, self_: &ActorRef) {
        let _ = self.aircraft.set(self_.trait_::<Aircraft>());
        let _ = self.body.set(self_.trait_::<BodyOrientation>());
        conditional_created(self, self_);
    }
}

impl Carryall {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn carryable(&self) -> Option<ActorRef> {
        self.carryable.borrow().clone()
    }

    pub fn carry_state(&self) -> CarryallState {
        self.carry_state.get()
    }

    pub fn carryable_offset(&self) -> WVec {
        self.carryable_offset.get()
    }

    fn aircraft(&self) -> &Rc<Aircraft> {
        self.aircraft.get().expect("Carryall created")
    }

    fn body(&self) -> &Rc<BodyOrientation> {
        self.body.get().expect("Carryall created")
    }

    pub fn offset_for_carryable(&self, carryable: &ActorRef) -> WVec {
        let local = carryable.info.trait_info::<CarryableInfo>().map_or(WVec::ZERO, |c| c.local_offset);
        self.info.local_offset - local
    }

    pub fn attach_carryable(&self, self_: &ActorRef, carryable: &ActorRef) -> bool {
        if self.carry_state.get() == CarryallState::Carrying {
            return false;
        }
        *self.carryable.borrow_mut() = Some(carryable.clone());
        self.carry_state.set(CarryallState::Carrying);
        if self.carry_condition_token.get() == INVALID_CONDITION_TOKEN {
            self.carry_condition_token
                .set(self_.grant_condition(self.info.carry_condition.as_deref().unwrap_or("")));
        }
        if let Some(c) = self.info.carryable_conditions.get(&carryable.info.name) {
            self.carryable_condition_token.set(self_.grant_condition(c));
        }
        self.carryable_offset.set(self.offset_for_carryable(carryable));
        let terrain: IndexSet<String> = carryable
            .trait_::<Mobile>()
            .locomotor_info()
            .terrain_speeds
            .keys()
            .cloned()
            .collect();
        *self.landable_terrain_types.borrow_mut() = Some(Rc::new(terrain));
        true
    }

    pub fn detach_carryable(&self, self_: &ActorRef) {
        self.unreserve_carryable(self_);
        if self.carry_condition_token.get() != INVALID_CONDITION_TOKEN {
            self.carry_condition_token
                .set(self_.revoke_condition(self.carry_condition_token.get()));
        }
        if self.carryable_condition_token.get() != INVALID_CONDITION_TOKEN {
            self.carryable_condition_token
                .set(self_.revoke_condition(self.carryable_condition_token.get()));
        }
        *self.landable_terrain_types.borrow_mut() = None;
        self.carryable_offset.set(WVec::ZERO);
    }

    pub fn reserve_carryable(&self, self_: &ActorRef, carryable: &ActorRef) -> bool {
        if self.carry_state.get() == CarryallState::Reserved {
            self.unreserve_carryable(self_);
        }
        if self.carry_state.get() != CarryallState::Idle || !carryable.trait_::<Carryable>().reserve(carryable, self_) {
            return false;
        }
        *self.carryable.borrow_mut() = Some(carryable.clone());
        self.carry_state.set(CarryallState::Reserved);
        true
    }

    pub fn unreserve_carryable(&self, self_: &ActorRef) {
        let current = self.carryable.borrow().clone();
        if let Some(c) = current
            && c.is_in_world()
            && !c.is_dead()
        {
            let carryable = c.trait_::<Carryable>();
            if carryable.carrier().is_some_and(|a| a.id == self_.id) {
                carryable.unreserve(&c);
            }
        }
        *self.carryable.borrow_mut() = None;
        self.carry_state.set(CarryallState::Idle);
    }

    /// `CanUnload`: whether the carried actor can be dropped at the current location.
    pub fn can_unload(&self) -> bool {
        if self.state.is_trait_disabled() {
            return false;
        }
        let Some(self_) = self.self_.upgrade() else { return false };
        let aircraft = self.aircraft();
        let cell = self_.world().map().cell_containing(aircraft.get_position());
        self.carryable.borrow().is_some() && aircraft.can_land(cell, None, false)
    }
}

impl ITick for Carryall {
    fn tick(&self, self_: &ActorRef) {
        if self.carry_state.get() == CarryallState::Carrying && self.carryable.borrow().as_ref().is_none_or(|c| c.is_dead()) {
            self.detach_carryable(self_);
        }
    }
}

impl INotifyActorDisposing for Carryall {
    fn disposing(&self, self_: &ActorRef) {
        if self.carry_state.get() == CarryallState::Carrying {
            let c = self.carryable.borrow_mut().take();
            if let Some(c) = c {
                c.dispose();
            }
        }
        self.unreserve_carryable(self_);
    }
}

impl INotifyKilled for Carryall {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        if self.carry_state.get() == CarryallState::Carrying {
            let c = self.carryable.borrow().clone();
            if let Some(c) = c
                && !c.is_dead()
            {
                if let Some(p) = c.trait_or_default::<dyn IPositionable>() {
                    p.set_position_cell(&c, self_.location(), crate::locomotor::any_sub());
                }
                if !c.disposed()
                    && let Some(h) = c.cache().health.clone()
                {
                    h.kill(&c, e.attacker.as_ref(), &DamageTypes::default());
                }
            }
            *self.carryable.borrow_mut() = None;
        }
        self.unreserve_carryable(self_);
    }
}

impl IAircraftCenterPositionOffset for Carryall {
    fn position_offset(&self) -> WVec {
        let Some(self_) = self.self_.upgrade() else { return WVec::ZERO };
        let body = self.body();
        let local = self.carryable_offset.get().rotate(&body.quantize_orientation(self_.orientation()));
        body.local_to_world(local)
    }
}

impl IOverrideAircraftLanding for Carryall {
    fn landable_terrain_types(&self) -> Option<Rc<IndexSet<String>>> {
        self.landable_terrain_types.borrow().clone()
    }
}

fn can_pickup(self_: &ActorRef, target: &ActorRef) -> bool {
    if !appears_friendly_to(target, self_) {
        return false;
    }
    let Some(carryable) = target.trait_or_default::<Carryable>() else {
        return false;
    };
    if carryable.state.is_trait_disabled() {
        return false;
    }
    !(carryable.reserved() && carryable.carrier().is_none_or(|c| c.id != self_.id))
}

/// `CarryallDeliverUnitTargeter`.
struct DeliverUnitTargeter {
    info: Rc<CarryallInfo>,
    move_into_shroud: bool,
    is_queued: Cell<bool>,
}

impl IOrderTargeter for DeliverUnitTargeter {
    fn order_id(&self) -> &str {
        "DeliverUnit"
    }

    fn order_priority(&self) -> i32 {
        6
    }

    fn can_target(&self, self_: &ActorRef, target: &Target, modifiers: &mut TargetModifiers, cursor: &mut Option<String>) -> bool {
        if !self.info.allow_drop_off || !modifiers.has_modifier(TargetModifiers::FORCE_MOVE) {
            return false;
        }
        let is_building = match target.target_type() {
            TargetType::Actor => target
                .actor()
                .is_some_and(|a| a.info.has_trait_info::<crate::building::BuildingInfo>()),
            TargetType::FrozenActor => target
                .frozen_actor()
                .is_some_and(|f| f.info().has_trait_info::<crate::building::BuildingInfo>()),
            _ => false,
        };
        if is_building {
            *cursor = Some(self.info.drop_off_blocked_cursor.clone());
            return true;
        }
        let w = self_.world();
        let location = w.map().cell_containing(target.center_position());
        let explored = self_.owner().shroud().is_explored(location);
        *cursor = Some(if w.map().contains(location) {
            self.info.drop_off_cursor.clone()
        } else {
            self.info.drop_off_blocked_cursor.clone()
        });
        self.is_queued.set(modifiers.has_modifier(TargetModifiers::FORCE_QUEUE));
        if !explored && !self.move_into_shroud {
            *cursor = Some(self.info.drop_off_blocked_cursor.clone());
        }
        true
    }

    fn is_queued(&self) -> bool {
        self.is_queued.get()
    }

    fn target_overrides_selection(&self, _: &ActorRef, _: &Target, _: &[ActorRef], _: CPos, _: TargetModifiers) -> bool {
        true
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl IIssueOrder for Carryall {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let me = self.me.upgrade().expect("alive");
        let (unload, blocked) = (self.info.unload_cursor.clone(), self.info.unload_blocked_cursor.clone());
        vec![
            Box::new(UnitOrderTargeter::new(
                "PickupUnit",
                5,
                &self.info.pick_up_cursor,
                false,
                true,
                Box::new(|self_, target, _, _| can_pickup(self_, target)),
                Box::new(|self_, target: &FrozenActorRef, _, _| target.actor().is_some_and(|a| can_pickup(self_, &a))),
            )),
            Box::new(DeployOrderTargeter::new(
                "Unload",
                10,
                Box::new(move || if me.can_unload() { unload.clone() } else { blocked.clone() }),
            )),
            Box::new(DeliverUnitTargeter {
                info: self.info.clone(),
                move_into_shroud: self.aircraft().info.move_into_shroud,
                is_queued: Cell::new(false),
            }),
        ]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        matches!(order.order_id(), "PickupUnit" | "DeliverUnit" | "Unload")
            .then(|| Order::new(order.order_id(), Some(self_.clone()), target.clone(), queued))
    }
}

impl IIssueDeployOrder for Carryall {
    fn issue_deploy_order(&self, self_: &ActorRef, queued: bool) -> Option<Order> {
        Some(Order::new("Unload", Some(self_.clone()), Target::Invalid, queued))
    }

    fn can_issue_deploy_order(&self, _self_: &ActorRef, _queued: bool) -> bool {
        !self.state.is_trait_disabled()
    }
}

impl IResolveOrder for Carryall {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        match order.order_string.as_str() {
            "DeliverUnit" => {
                if !order.target.is_valid_for(self_) {
                    return;
                }
                let w = self_.world();
                let cell = w.map().clamp(w.map().cell_containing(order.target.center_position()));
                if !self.aircraft().info.move_into_shroud && !self_.owner().shroud().is_explored(cell) {
                    return;
                }
                let a = DeliverUnit::new(
                    self_,
                    Some(order.target.clone()),
                    self.info.drop_range,
                    Some(self.info.target_line_color),
                );
                self_.queue_activity_queued(order.queued, a);
                crate::target_lines::show_target_lines(self_);
            }
            "Unload" => {
                if !order.queued && !self.can_unload() {
                    return;
                }
                let a = DeliverUnit::new(self_, None, self.info.drop_range, Some(self.info.target_line_color));
                self_.queue_activity_queued(order.queued, a);
            }
            "PickupUnit" => {
                if order.target.target_type() != TargetType::Actor {
                    return;
                }
                let Some(cargo) = order.target.actor().cloned() else { return };
                let a = PickupUnit::new(self_, &cargo, self.info.before_load_delay, Some(self.info.target_line_color));
                self_.queue_activity_queued(order.queued, a);
                crate::target_lines::show_target_lines(self_);
            }
            _ => {}
        }
    }
}

impl IOrderVoice for Carryall {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        matches!(order.order_string.as_str(), "DeliverUnit" | "Unload" | "PickupUnit").then(|| self.info.voice.clone())
    }
}

#[derive(Clone, Debug, FieldLoad, Default)]
pub struct CarryableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub reserved_condition: Option<String>,
    pub carried_condition: Option<String>,
    pub locked_condition: Option<String>,
    pub local_offset: WVec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockResponse {
    Success,
    Pending,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CarryableState {
    Free,
    Reserved,
    Locked,
}

/// Can be carried by actors with `Carryall`.
pub struct Carryable {
    pub info: Rc<CarryableInfo>,
    state: ConditionalState,
    reserved_token: Cell<i32>,
    carried_token: Cell<i32>,
    locked_token: Cell<i32>,
    delay_pickups: RefCell<Vec<Rc<dyn IDelayCarryallPickup>>>,
    carrier: RefCell<Option<ActorRef>>,
    mobile: OnceCell<Option<Rc<Mobile>>>,
    carry_state: Cell<CarryableState>,
    attached: Cell<bool>,
}

register_traits!(Carryable: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(CarryableInfo => |info, _init| Some(Rc::new(Carryable {
    state: ConditionalState::new(&info.base),
    info,
    reserved_token: Cell::new(INVALID_CONDITION_TOKEN),
    carried_token: Cell::new(INVALID_CONDITION_TOKEN),
    locked_token: Cell::new(INVALID_CONDITION_TOKEN),
    delay_pickups: RefCell::new(Vec::new()),
    carrier: RefCell::new(None),
    mobile: OnceCell::new(),
    carry_state: Cell::new(CarryableState::Free),
    attached: Cell::new(false),
})));
rustora_engine::conditional_plumbing!(Carryable);

impl ConditionalHooks for Carryable {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl INotifyCreated for Carryable {
    fn created(&self, self_: &ActorRef) {
        let _ = self.mobile.set(self_.trait_or_default::<Mobile>());
        *self.delay_pickups.borrow_mut() = self_.traits_implementing::<dyn IDelayCarryallPickup>();
        conditional_created(self, self_);
    }
}

impl Carryable {
    pub fn carrier(&self) -> Option<ActorRef> {
        self.carrier.borrow().clone()
    }

    pub fn reserved(&self) -> bool {
        self.carry_state.get() != CarryableState::Free
    }

    fn mobile(&self) -> Option<&Rc<Mobile>> {
        self.mobile.get().and_then(|m| m.as_ref())
    }

    pub fn attached(&self, self_: &ActorRef, carrier: &ActorRef) {
        if self.attached.get() {
            return;
        }
        self.attached.set(true);
        *self.carrier.borrow_mut() = Some(carrier.clone());
        if self.carried_token.get() == INVALID_CONDITION_TOKEN {
            self.carried_token
                .set(self_.grant_condition(self.info.carried_condition.as_deref().unwrap_or("")));
        }
    }

    pub fn detached(&self, self_: &ActorRef) {
        if !self.attached.get() {
            return;
        }
        self.attached.set(false);
        if self.carried_token.get() != INVALID_CONDITION_TOKEN {
            self.carried_token.set(self_.revoke_condition(self.carried_token.get()));
        }
    }

    pub fn reserve(&self, self_: &ActorRef, carrier: &ActorRef) -> bool {
        if self.reserved() || self.state.is_trait_disabled() {
            return false;
        }
        self.carry_state.set(CarryableState::Reserved);
        *self.carrier.borrow_mut() = Some(carrier.clone());
        if self.reserved_token.get() == INVALID_CONDITION_TOKEN {
            self.reserved_token
                .set(self_.grant_condition(self.info.reserved_condition.as_deref().unwrap_or("")));
        }
        true
    }

    pub fn unreserve(&self, self_: &ActorRef) {
        self.carry_state.set(CarryableState::Free);
        *self.carrier.borrow_mut() = None;
        if self.reserved_token.get() != INVALID_CONDITION_TOKEN {
            self.reserved_token.set(self_.revoke_condition(self.reserved_token.get()));
        }
        if self.locked_token.get() != INVALID_CONDITION_TOKEN {
            self.locked_token.set(self_.revoke_condition(self.locked_token.get()));
        }
    }

    /// `LockForPickup`: prepares for the pickup by `carrier`.
    pub fn lock_for_pickup(&self, self_: &ActorRef, carrier: &ActorRef) -> LockResponse {
        if self.carry_state.get() == CarryableState::Locked && self.carrier().is_none_or(|c| c.id != carrier.id) {
            return LockResponse::Failed;
        }
        let delays = self.delay_pickups.borrow().clone();
        if delays
            .iter()
            .any(|d| d.is_trait_enabled() && !d.try_lock_for_pickup(self_, carrier))
        {
            return LockResponse::Pending;
        }
        if let Some(m) = self.mobile()
            && !m.can_stay_in_cell(self_.location())
        {
            return LockResponse::Pending;
        }
        if self.carry_state.get() != CarryableState::Locked {
            self.carry_state.set(CarryableState::Locked);
            *self.carrier.borrow_mut() = Some(carrier.clone());
            if self.locked_token.get() == INVALID_CONDITION_TOKEN {
                self.locked_token
                    .set(self_.grant_condition(self.info.locked_condition.as_deref().unwrap_or("")));
            }
        }
        if self.mobile().is_some_and(|m| m.is_moving_between_cells()) {
            return LockResponse::Pending;
        }
        LockResponse::Success
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickupState {
    Intercept,
    LockCarryable,
    Pickup,
}

/// `Activities/PickupUnit.cs`.
pub struct PickupUnit {
    base: ActivityBase,
    cargo: ActorRef,
    carryall: Rc<Carryall>,
    carryable: Rc<Carryable>,
    delay: i32,
    state: Cell<PickupState>,
    reserve_failed: Cell<bool>,
}

impl PickupUnit {
    pub fn new(self_: &ActorRef, cargo: &ActorRef, delay: i32, target_line_color: Option<Color>) -> ActivityRef {
        let a = Rc::new(PickupUnit {
            base: ActivityBase::default(),
            cargo: cargo.clone(),
            carryall: self_.trait_::<Carryall>(),
            carryable: cargo.trait_::<Carryable>(),
            delay,
            state: Cell::new(PickupState::Intercept),
            reserve_failed: Cell::new(false),
        });
        a.base.child_has_priority.set(false);
        match target_line_color {
            Some(c) => with_line_color(a, c),
            None => a,
        }
    }

    fn cargo_invalid(&self, self_: &ActorRef) -> bool {
        self.cargo.is_dead()
            || self.carryable.state.is_trait_disabled()
            || self.carryall.is_trait_disabled()
            || !appears_friendly_to(&self.cargo, self_)
    }
}

const TARGET_LOCK_RANGE: WDist = WDist::new(4 * 1024);

impl Activity for PickupUnit {
    activity_common!("PickupUnit");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        Some(Target::from_actor(Some(&self.cargo)))
    }

    fn on_first_run(&self, self_: &ActorRef) {
        if self.cargo_invalid(self_) {
            self.reserve_failed.set(true);
            return;
        }
        if self.carryall.reserve_carryable(self_, &self.cargo) {
            self.base.queue_child(Fly::new(self_, Target::from_actor(Some(&self.cargo))));
            self.base.queue_child(FlyIdle::new(self_, -1, false));
        } else {
            self.reserve_failed.set(true);
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() || self.reserve_failed.get() {
            return true;
        }
        if self.cargo_invalid(self_) || self.carryall.carryable().is_none_or(|c| c.id != self.cargo.id) {
            self.cancel(self_, true);
            return false;
        }
        if self.state.get() == PickupState::Intercept
            && (self.cargo.center_position() - self_.center_position()).horizontal_length_squared() <= TARGET_LOCK_RANGE.length_squared()
        {
            self.state.set(PickupState::LockCarryable);
        }
        if self.state.get() == PickupState::LockCarryable {
            match self.carryable.lock_for_pickup(&self.cargo, self_) {
                LockResponse::Failed => {
                    self.cancel(self_, true);
                    return false;
                }
                LockResponse::Success => {
                    if let Some(c) = self.base.child_activity() {
                        c.cancel(self_, false);
                    }
                    let body = self.cargo.trait_::<BodyOrientation>();
                    let local = self
                        .carryall
                        .offset_for_carryable(&self.cargo)
                        .rotate(&body.quantize_orientation(self.cargo.orientation()));
                    let facing = self.cargo.cache().facing.as_ref().map(|f| f.facing());
                    self.base.queue_child(Land::full(
                        self_,
                        Target::from_actor(Some(&self.cargo)),
                        LandArgs {
                            land_range: WDist::ZERO,
                            offset: -body.local_to_world(local),
                            facing,
                            clear_cells: Vec::new(),
                        },
                    ));
                    if self.delay > 0 {
                        self.base.queue_child(Wait::new_interruptible(self.delay, false));
                    }
                    self.base.queue_child(AttachUnit::new(self_, &self.cargo));
                    self.base.queue_child(TakeOff::new(self_));
                    self.state.set(PickupState::Pickup);
                }
                LockResponse::Pending => {}
            }
        }
        self.base.tick_child(self_) && self.state.get() == PickupState::Pickup
    }

    fn cancel(&self, self_: &ActorRef, keep_queue: bool) {
        self.base.cancel(self_, keep_queue);
        if !self.base.is_interruptible.get() {
            return;
        }
        if self.carryall.carry_state() == CarryallState::Reserved {
            self.carryall.unreserve_carryable(self_);
        }
        if self.base.child_activity().is_some_and(|c| c.type_name() == "TakeOff") {
            self.base.child_has_priority.set(true);
            return;
        }
        if self_.trait_::<Aircraft>().has_influence() {
            self.base.child_has_priority.set(true);
            self.base.queue_child(TakeOff::new(self_));
        }
    }
}

/// `PickupUnit.AttachUnit`.
struct AttachUnit {
    base: ActivityBase,
    cargo: ActorRef,
    carryable: Rc<Carryable>,
    carryall: Rc<Carryall>,
}

impl AttachUnit {
    fn new(self_: &ActorRef, cargo: &ActorRef) -> ActivityRef {
        Rc::new(AttachUnit {
            base: ActivityBase::default(),
            cargo: cargo.clone(),
            carryable: cargo.trait_::<Carryable>(),
            carryall: self_.trait_::<Carryall>(),
        })
    }
}

impl Activity for AttachUnit {
    activity_common!("AttachUnit");

    fn on_first_run(&self, self_: &ActorRef) {
        if self.cargo.is_dead()
            || self.carryable.state.is_trait_disabled()
            || self.carryall.is_trait_disabled()
            || self.carryall.carryable().is_none_or(|c| c.id != self.cargo.id)
            || !appears_friendly_to(&self.cargo, self_)
        {
            return;
        }
        let (cargo, carryable, carryall, me) = (self.cargo.clone(), self.carryable.clone(), self.carryall.clone(), self_.clone());
        self_.world().add_frame_end_task(Box::new(move |w| {
            w.remove(&cargo);
            carryable.attached(&cargo, &me);
            carryall.attach_carryable(&me, &cargo);
        }));
    }
}

/// `Activities/DeliverUnit.cs`.
pub struct DeliverUnit {
    base: ActivityBase,
    carryall: Rc<Carryall>,
    assign_target_on_first_run: bool,
    deliver_range: WDist,
    destination: RefCell<Target>,
}

impl DeliverUnit {
    /// Delivers to `destination`, or to the current location for `None`.
    pub fn new(self_: &ActorRef, destination: Option<Target>, deliver_range: WDist, target_line_color: Option<Color>) -> ActivityRef {
        let a = Rc::new(DeliverUnit {
            base: ActivityBase::default(),
            carryall: self_.trait_::<Carryall>(),
            assign_target_on_first_run: destination.is_none(),
            deliver_range,
            destination: RefCell::new(destination.unwrap_or(Target::Invalid)),
        });
        match target_line_color {
            Some(c) => with_line_color(a, c),
            None => a,
        }
    }
}

impl Activity for DeliverUnit {
    activity_common!("DeliverUnit");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        Some(self.destination.borrow().clone())
    }

    fn on_first_run(&self, self_: &ActorRef) {
        if self.carryall.carryable().is_none() {
            return;
        }
        if self.assign_target_on_first_run {
            *self.destination.borrow_mut() = Target::from_cell(&self_.world(), self_.location(), FULL_CELL);
        }
        let destination = self.destination.borrow().clone();
        self.base.queue_child(Land::full(
            self_,
            destination,
            LandArgs {
                land_range: self.deliver_range,
                ..Default::default()
            },
        ));
        self.base
            .queue_child(Wait::new_interruptible(self.carryall.info.before_unload_delay, false));
        self.base.queue_child(ReleaseUnit::new(self_));
        self.base.queue_child(TakeOff::new(self_));
    }
}

/// `DeliverUnit.ReleaseUnit`.
struct ReleaseUnit {
    base: ActivityBase,
    carryall: Rc<Carryall>,
    body: Rc<BodyOrientation>,
}

impl ReleaseUnit {
    fn new(self_: &ActorRef) -> ActivityRef {
        Rc::new(ReleaseUnit {
            base: ActivityBase::default(),
            carryall: self_.trait_::<Carryall>(),
            body: self_.trait_::<BodyOrientation>(),
        })
    }
}

impl Activity for ReleaseUnit {
    activity_common!("ReleaseUnit");

    fn on_first_run(&self, self_: &ActorRef) {
        let Some(cargo) = self.carryall.carryable() else { return };
        let local = self
            .carryall
            .carryable_offset()
            .rotate(&self.body.quantize_orientation(self_.orientation()));
        let target_position = self_.center_position() + self.body.local_to_world(local);
        let w = self_.world();
        let target_location = w.map().cell_containing(target_position);
        cargo
            .trait_::<dyn IPositionable>()
            .set_position_cell(&cargo, target_location, FULL_CELL);
        if let (Some(f), Some(own)) = (cargo.cache().facing.as_ref(), self_.cache().facing.as_ref()) {
            f.set_facing(own.facing());
        }
        let (carryall, me) = (self.carryall.clone(), self_.clone());
        w.add_frame_end_task(Box::new(move |w| {
            if me.is_dead() {
                return;
            }
            let Some(cargo) = carryall.carryable() else { return };
            let carryable = cargo.trait_::<Carryable>();
            w.add(&cargo);
            carryall.detach_carryable(&me);
            carryable.unreserve(&cargo);
            carryable.detached(&cargo);
        }));
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct FreeActorWithDeliveryInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub free: crate::base::FreeActorInfo,
    #[field(require)]
    pub delivering_actor: String,
    pub spawn_location: CPos,
    pub delivery_offset: rustora_core::world_types::CVec,
    pub delivery_range: WDist,
}

/// `Traits/Buildings/FreeActorWithDelivery.cs`: a free actor that a carryall brings in.
pub struct FreeActorWithDelivery {
    info: Rc<FreeActorWithDeliveryInfo>,
    state: ConditionalState,
    allow_spawn: Cell<bool>,
}

register_traits!(FreeActorWithDelivery: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(FreeActorWithDeliveryInfo => |info, init| {
    let state = ConditionalState::new(&info.free.base);
    let allow = init
        .inits
        .get_for::<crate::base::FreeActorInit>(info.instance_name.as_deref())
        .is_none_or(|i| i.value);
    Some(Rc::new(FreeActorWithDelivery { info, state, allow_spawn: Cell::new(allow) }))
});
rustora_engine::conditional_plumbing!(FreeActorWithDelivery);

impl ConditionalHooks for FreeActorWithDelivery {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        if !self.allow_spawn.get() {
            return;
        }
        self.allow_spawn.set(self.info.free.allow_respawn);
        self.do_delivery(
            self_,
            self_.location() + self.info.delivery_offset,
            &self.info.free.actor,
            &self.info.delivering_actor,
        );
    }
}

impl INotifyCreated for FreeActorWithDelivery {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl FreeActorWithDelivery {
    /// `DoDelivery`: spawns `carrier_name` at the map edge carrying `actor_name` to `location`.
    pub fn do_delivery(&self, self_: &ActorRef, location: CPos, actor_name: &str, carrier_name: &str) {
        use rustora_engine::actor_init::{ActorInits, CenterPositionInit, FacingInit, LocationInit, OwnerInit};
        let w = self_.world();
        let mut spawn_cell = self.info.spawn_location;
        if spawn_cell == CPos::new(0, 0) {
            spawn_cell = w.map().choose_closest_edge_cell(self_.location());
        }
        let mut spawn = w.map().center_of_cell(spawn_cell);
        let initial_facing = w
            .map()
            .facing_between(spawn_cell, self_.location(), rustora_core::world_types::WAngle::ZERO);
        let carrier_info = w.rules().actor(&carrier_name.to_lowercase());
        if let Some(ai) = carrier_info.and_then(|a| a.trait_info::<crate::aircraft::AircraftInfo>()) {
            spawn = spawn + WVec::new(0, 0, ai.cruise_altitude.length);
        }
        let carrier = w.create_actor_add(
            false,
            &carrier_name.to_lowercase(),
            ActorInits::new()
                .with(LocationInit::new(spawn_cell))
                .with(CenterPositionInit::new(spawn))
                .with(OwnerInit::from_player(&self_.owner()))
                .with(FacingInit::new(initial_facing)),
        );
        let cargo = w.create_actor_add(
            false,
            &actor_name.to_lowercase(),
            ActorInits::new().with(OwnerInit::from_player(&self_.owner())),
        );
        cargo.trait_::<Carryable>().reserve(&cargo, &carrier);
        let carryall = carrier.trait_::<Carryall>();
        carryall.attach_carryable(&carrier, &cargo);
        carrier.queue_activity(DeliverUnit::new(
            &carrier,
            Some(Target::from_cell(&w, location, FULL_CELL)),
            self.info.delivery_range,
            Some(carryall.info.target_line_color),
        ));
        let edge = w.map().choose_random_edge_cell(&mut w.shared_random.borrow_mut());
        carrier.queue_activity(Fly::new(&carrier, Target::from_cell(&w, edge, FULL_CELL)));
        carrier.queue_activity(crate::aircraft::RemoveSelf::new());
        w.add_frame_end_task(Box::new(move |w| w.add(&carrier)));
    }
}
