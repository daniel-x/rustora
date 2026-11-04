// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Cargo.cs`, `Traits/Passenger.cs`, `Traits/ParaDrop.cs`, `Activities/Enter.cs`,
//! `Activities/RideTransport.cs` and `Activities/UnloadCargo.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use rustora_core::expression::BooleanExpression;
use rustora_core::world_types::{CPos, WAngle, WDist, WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::actor::{ActorRef, ConditionCache, INVALID_CONDITION_TOKEN};
use rustora_engine::actor_init::{ActorInit, ActorInits, OwnerInit};
use rustora_engine::conditional::*;
use rustora_engine::damage::{AttackInfo, DamageTypes};
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::subcell::{ANY, FULL_CELL, INVALID};
use rustora_engine::traits::*;
use rustora_engine::{activity_common, conditional_plumbing, info_impl, load_info, register_traits};

use crate::activities::{Move, MoveCooldownHelper, Nudge, Wait, WaitFor, notify_blocker_cells};
use crate::aircraft::{Aircraft, Land, LandArgs, TakeOff};
use crate::interfaces::*;
use crate::locomotor::BlockedByActor;
use crate::mobile::Mobile;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("Cargo", |n| load_info!(CargoInfo, n));
    r.register("Passenger", |n| load_info!(PassengerInfo, n));
    r.register("ParaDrop", |n| load_info!(ParaDropInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CargoInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub max_weight: i32,
    pub types: IndexSet<String>,
    pub initial_units: Vec<String>,
    pub eject_on_sell: bool,
    pub eject_on_death: bool,
    pub unload_terrain_types: IndexSet<String>,
    pub load_range: WDist,
    pub passenger_facing: WAngle,
    pub after_load_delay: i32,
    pub before_unload_delay: i32,
    pub after_unload_delay: i32,
    pub loading_condition: Option<String>,
    pub loaded_condition: Option<String>,
    pub passenger_conditions: IndexMap<String, String>,
    pub unload_cursor: String,
    pub unload_blocked_cursor: String,
}

impl Default for CargoInfo {
    fn default() -> Self {
        CargoInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            max_weight: 0,
            types: IndexSet::new(),
            initial_units: Vec::new(),
            eject_on_sell: true,
            eject_on_death: false,
            unload_terrain_types: IndexSet::new(),
            load_range: WDist::from_cells(5),
            passenger_facing: WAngle::new(512),
            after_load_delay: 8,
            before_unload_delay: 8,
            after_unload_delay: 25,
            loading_condition: None,
            loaded_condition: None,
            passenger_conditions: IndexMap::new(),
            unload_cursor: "deploy".into(),
            unload_blocked_cursor: "deploy-blocked".into(),
        }
    }
}

/// `RuntimeCargoInit`.
pub struct RuntimeCargoInit {
    pub value: Vec<ActorRef>,
    pub instance_name: Option<String>,
}

impl ActorInit for RuntimeCargoInit {
    fn instance_name(&self) -> Option<&str> {
        self.instance_name.as_deref()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `CargoInit`.
pub struct CargoInit {
    pub value: Vec<String>,
    pub instance_name: Option<String>,
}

impl ActorInit for CargoInit {
    fn instance_name(&self) -> Option<&str> {
        self.instance_name.as_deref()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn weight_of(a: &ActorRef) -> i32 {
    a.info.trait_info::<PassengerInfo>().expect("passenger info").weight
}

pub struct Cargo {
    pub info: Rc<CargoInfo>,
    state: ConditionalState,
    self_: ActorRef,
    cargo: RefCell<Vec<ActorRef>>,
    reserves: RefCell<Vec<ActorRef>>,
    passenger_tokens: RefCell<IndexMap<String, Vec<i32>>>,
    check_terrain_type: bool,
    total_weight: Cell<i32>,
    reserved_weight: Cell<i32>,
    aircraft: OnceCell<Option<Rc<Aircraft>>>,
    loading_token: Cell<i32>,
    loaded_tokens: RefCell<Vec<i32>>,
    take_off_after_load: Cell<bool>,
    initialised: Cell<bool>,
    locked: Rc<Cell<bool>>,
}

register_traits!(
    Cargo: IResolveOrder,
    crate::orders::IIssueOrder,
    INotifyOwnerChanged,
    INotifySold,
    INotifyActorDisposing,
    INotifyCreated,
    INotifyKilled,
    ITransformActorInitModifier,
    IObservesVariables,
    IDisabledTrait,
    ISync
);
info_impl!(CargoInfo => |info, init| {
    let name = info.instance_name.as_deref();
    let self_ = init.self_.clone();
    let mut cargo = Vec::new();
    let create = |list: &[String], cargo: &mut Vec<ActorRef>| {
        let w = self_.world();
        for u in list {
            cargo.push(w.create_actor_add(false, &u.to_lowercase(), ActorInits::new().with(OwnerInit::from_player(&self_.owner()))));
        }
    };
    if let Some(r) = init.inits.get_for::<RuntimeCargoInit>(name) {
        cargo = r.value.clone();
    } else if let Some(c) = init.inits.get_for::<CargoInit>(name) {
        create(&c.value, &mut cargo);
    } else {
        create(&info.initial_units, &mut cargo);
    }
    let total = cargo.iter().map(weight_of).sum();
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(Cargo {
        state,
        self_,
        cargo: RefCell::new(cargo),
        reserves: RefCell::new(Vec::new()),
        passenger_tokens: RefCell::new(IndexMap::new()),
        check_terrain_type: !info.unload_terrain_types.is_empty(),
        total_weight: Cell::new(total),
        reserved_weight: Cell::new(0),
        aircraft: OnceCell::new(),
        loading_token: Cell::new(INVALID_CONDITION_TOKEN),
        loaded_tokens: RefCell::new(Vec::new()),
        take_off_after_load: Cell::new(false),
        initialised: Cell::new(false),
        locked: Rc::new(Cell::new(false)),
        info,
    }))
});

impl ConditionalHooks for Cargo {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_plumbing!(Cargo);

impl INotifyCreated for Cargo {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
        let _ = self.aircraft.set(self_.trait_or_default::<Aircraft>());
        let passengers = self.cargo.borrow().clone();
        if !passengers.is_empty() {
            for c in &passengers {
                self.grant_passenger_conditions(self_, c);
            }
        }
        let me = self_.clone();
        self_.world().add_frame_end_task(Box::new(move |_w| {
            let cargo = me.trait_::<Cargo>();
            let list = cargo.cargo.borrow().clone();
            for c in &list {
                *c.trait_::<Passenger>().transport.borrow_mut() = Some(me.clone());
                for nec in c.traits_implementing::<dyn INotifyEnteredCargo>() {
                    nec.on_entered_cargo(c, &me);
                }
                for npe in me.traits_implementing::<dyn INotifyPassengerEntered>() {
                    npe.on_passenger_entered(&me, c);
                }
            }
            cargo.initialised.set(true);
        }));
    }
}

impl Cargo {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn passengers(&self) -> Vec<ActorRef> {
        self.cargo.borrow().clone()
    }

    pub fn passenger_count(&self) -> usize {
        self.cargo.borrow().len()
    }

    fn grant_passenger_conditions(&self, self_: &ActorRef, c: &ActorRef) {
        if let Some(cond) = self.info.passenger_conditions.get(&c.info.name) {
            let t = self_.grant_condition(cond);
            self.passenger_tokens.borrow_mut().entry(c.info.name.clone()).or_default().push(t);
        }
        if let Some(cond) = self.info.loaded_condition.as_deref().filter(|c| !c.is_empty()) {
            let t = self_.grant_condition(cond);
            self.loaded_tokens.borrow_mut().push(t);
        }
    }

    /// `CurrentAdjacentCells`.
    pub fn current_adjacent_cells(&self) -> Vec<CPos> {
        let w = self.self_.world();
        let loc = self.self_.location();
        util::adjacent_cells(&w, &Target::from_actor(Some(&self.self_)))
            .into_iter()
            .filter(|c| *c != loc)
            .collect()
    }

    pub fn can_unload(&self, check: BlockedByActor) -> bool {
        if self.is_trait_disabled() {
            return false;
        }
        let w = self.self_.world();
        let loc = self.self_.location();
        if self.check_terrain_type
            && (!w.map().contains(loc) || !self.info.unload_terrain_types.contains(&w.map().terrain_type_info(loc).ty))
        {
            return false;
        }
        if self.is_empty() {
            return false;
        }
        if let Some(a) = self.aircraft.get().and_then(|a| a.as_ref())
            && !a.can_land(loc, None, false)
        {
            return false;
        }
        let passengers = self.cargo.borrow().clone();
        self.current_adjacent_cells().iter().any(|c| {
            passengers
                .iter()
                .any(|p| !p.is_dead() && p.trait_::<dyn IPositionable>().can_enter_cell(*c, None, check))
        })
    }

    pub fn can_load(&self, a: &ActorRef) -> bool {
        !self.is_trait_disabled() && (self.is_reserved(a) || self.has_space(weight_of(a)))
    }

    fn is_reserved(&self, a: &ActorRef) -> bool {
        self.reserves.borrow().iter().any(|r| r.id == a.id)
    }

    pub fn reserve_space(&self, a: &ActorRef) -> bool {
        if self.is_reserved(a) {
            return true;
        }
        let w = weight_of(a);
        if !self.has_space(w) {
            return false;
        }
        if self.loading_token.get() == INVALID_CONDITION_TOKEN {
            self.loading_token
                .set(self.self_.grant_condition(self.info.loading_condition.as_deref().unwrap_or("")));
        }
        self.reserves.borrow_mut().push(a.clone());
        self.reserved_weight.set(self.reserved_weight.get() + w);
        self.lock_for_pickup();
        true
    }

    pub fn unreserve_space(&self, a: &ActorRef) {
        if !self.is_reserved(a) || self.self_.is_dead() {
            return;
        }
        self.reserved_weight.set(self.reserved_weight.get() - weight_of(a));
        self.reserves.borrow_mut().retain(|r| r.id != a.id);
        self.release_lock();
        self.revoke_loading_token();
    }

    fn revoke_loading_token(&self) {
        if self.loading_token.get() != INVALID_CONDITION_TOKEN {
            self.loading_token.set(self.self_.revoke_condition(self.loading_token.get()));
        }
    }

    fn lock_for_pickup(&self) {
        if self.locked.get() {
            return;
        }
        self.locked.set(true);
        let self_ = &self.self_;
        self_.cancel_activity();
        if let Some(air) = self_.trait_or_default::<Aircraft>()
            && !air.at_land_altitude()
        {
            self.take_off_after_load.set(true);
            self_.queue_activity(Land::new(self_));
        }
        let locked = self.locked.clone();
        self_.queue_activity(WaitFor::new(Box::new(move || !locked.get()), false));
    }

    fn release_lock(&self) {
        if self.reserved_weight.get() != 0 {
            return;
        }
        self.locked.set(false);
        let self_ = &self.self_;
        self_.queue_activity(Wait::new_interruptible(self.info.after_load_delay, false));
        if self.take_off_after_load.get() {
            self_.queue_activity(TakeOff::new(self_));
        }
        self.take_off_after_load.set(false);
    }

    pub fn has_space(&self, weight: i32) -> bool {
        self.total_weight.get() + self.reserved_weight.get() + weight <= self.info.max_weight
    }

    pub fn is_empty(&self) -> bool {
        self.cargo.borrow().is_empty()
    }

    pub fn peek(&self) -> ActorRef {
        self.cargo.borrow().last().expect("peek into empty cargo").clone()
    }

    pub fn unload(&self, self_: &ActorRef, passenger: Option<&ActorRef>) -> ActorRef {
        let passenger = passenger.cloned().unwrap_or_else(|| self.peek());
        let idx = self
            .cargo
            .borrow()
            .iter()
            .position(|c| c.id == passenger.id)
            .expect("Attempted to unload an actor that is not a passenger.");
        self.cargo.borrow_mut().remove(idx);
        self.total_weight.set(self.total_weight.get() - weight_of(&passenger));
        self.set_passenger_facing(&passenger);
        for npe in self_.traits_implementing::<dyn INotifyPassengerExited>() {
            npe.on_passenger_exited(self_, &passenger);
        }
        for nec in passenger.traits_implementing::<dyn INotifyExitedCargo>() {
            nec.on_exited_cargo(&passenger, self_);
        }
        *passenger.trait_::<Passenger>().transport.borrow_mut() = None;
        let token = self
            .passenger_tokens
            .borrow_mut()
            .get_mut(&passenger.info.name)
            .and_then(|t| t.pop());
        if let Some(t) = token {
            self_.revoke_condition(t);
        }
        let loaded = self.loaded_tokens.borrow_mut().pop();
        if let Some(t) = loaded {
            self_.revoke_condition(t);
        }
        passenger
    }

    fn set_passenger_facing(&self, passenger: &ActorRef) {
        let Some(facing) = self.self_.trait_or_default::<dyn IFacing>() else {
            return;
        };
        if let Some(pf) = passenger.trait_or_default::<dyn IFacing>() {
            pf.set_facing(facing.facing() + self.info.passenger_facing);
        }
    }

    pub fn load(&self, self_: &ActorRef, a: &ActorRef) {
        self.cargo.borrow_mut().push(a.clone());
        let w = weight_of(a);
        self.total_weight.set(self.total_weight.get() + w);
        if self.is_reserved(a) {
            self.reserved_weight.set(self.reserved_weight.get() - w);
            self.reserves.borrow_mut().retain(|r| r.id != a.id);
            self.release_lock();
            self.revoke_loading_token();
        }
        if self.initialised.get() {
            *a.trait_::<Passenger>().transport.borrow_mut() = Some(self_.clone());
            for nec in a.traits_implementing::<dyn INotifyEnteredCargo>() {
                nec.on_entered_cargo(a, self_);
            }
            for npe in self_.traits_implementing::<dyn INotifyPassengerEntered>() {
                npe.on_passenger_entered(self_, a);
            }
        }
        self.grant_passenger_conditions(self_, a);
    }

    fn spawn_passenger(&self, passenger: ActorRef) {
        let self_ = self.self_.clone();
        self_.world().add_frame_end_task(Box::new(move |w| {
            w.add(&passenger);
            passenger
                .trait_::<dyn IPositionable>()
                .set_position_cell(&passenger, self_.location(), ANY);
        }));
    }
}

impl IResolveOrder for Cargo {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "Unload" {
            if !order.queued && !self.can_unload(BlockedByActor::None) {
                return;
            }
            self_.queue_activity_queued(order.queued, UnloadCargo::new(self_, self.info.load_range));
        }
    }
}

impl INotifyKilled for Cargo {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        let w = self_.world();
        let on_unload_terrain = !self.check_terrain_type
            || self
                .info
                .unload_terrain_types
                .contains(&w.map().terrain_type_info(self_.location()).ty);
        if self.info.eject_on_death && util::is_at_ground_level(self_) && on_unload_terrain {
            while !self.is_empty() {
                let passenger = self.unload(self_, None);
                let attacker = e.attacker.clone();
                let me = self_.clone();
                w.add_frame_end_task(Box::new(move |w| {
                    let positionable = passenger.trait_::<dyn IPositionable>();
                    if positionable.can_enter_cell(me.location(), Some(&me), BlockedByActor::All) {
                        positionable.set_position_cell(&passenger, me.location(), ANY);
                        w.add(&passenger);
                        for nbm in passenger.traits_implementing::<dyn INotifyBlockingMove>() {
                            nbm.on_notify_blocking_move(&passenger, &passenger);
                        }
                        passenger.trait_::<Passenger>().on_ejected_from_killed_cargo(&passenger);
                    } else {
                        crate::health::kill_actor(&passenger, attacker.as_ref(), &DamageTypes::default());
                    }
                }));
            }
        } else {
            let list = self.cargo.borrow().clone();
            for c in &list {
                crate::health::kill_actor(c, e.attacker.as_ref(), &DamageTypes::default());
            }
        }
        self.cargo.borrow_mut().clear();
    }
}

impl INotifyActorDisposing for Cargo {
    fn disposing(&self, _self_: &ActorRef) {
        let list = self.cargo.borrow().clone();
        for c in &list {
            c.dispose();
        }
        self.cargo.borrow_mut().clear();
    }
}

impl INotifySold for Cargo {
    fn selling(&self, _self_: &ActorRef) {}

    fn sold(&self, self_: &ActorRef) {
        if !self.info.eject_on_sell {
            return;
        }
        while !self.is_empty() {
            let p = self.unload(self_, None);
            self.spawn_passenger(p);
        }
    }
}

impl INotifyOwnerChanged for Cargo {
    fn on_owner_changed(&self, _self_: &ActorRef, _old_owner: &PlayerRef, new_owner: &PlayerRef) {
        for p in self.passengers() {
            p.change_owner(new_owner.clone());
        }
    }
}

impl ITransformActorInitModifier for Cargo {
    fn modify_transform_actor_init(&self, _self_: &ActorRef, init: &mut ActorInits) {
        init.push(Rc::new(RuntimeCargoInit {
            value: self.passengers(),
            instance_name: self.info.instance_name.clone(),
        }));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct PassengerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub cargo_type: Option<String>,
    pub weight: i32,
    pub cargo_condition: Option<String>,
    pub cargo_conditions: IndexMap<String, String>,
    pub require_force_move_condition: Option<BooleanExpression>,
    pub enter_cursor: String,
    pub enter_blocked_cursor: String,
}

impl Default for PassengerInfo {
    fn default() -> Self {
        PassengerInfo {
            instance_name: None,
            cargo_type: None,
            weight: 1,
            cargo_condition: None,
            cargo_conditions: IndexMap::new(),
            require_force_move_condition: None,
            enter_cursor: "enter".into(),
            enter_blocked_cursor: "enter-blocked".into(),
        }
    }
}

pub struct Passenger {
    pub info: Rc<PassengerInfo>,
    pub transport: RefCell<Option<ActorRef>>,
    require_force_move: Cell<bool>,
    any_cargo_token: Cell<i32>,
    specific_cargo_token: Cell<i32>,
    reserved_cargo: RefCell<Option<Rc<Cargo>>>,
}

register_traits!(Passenger: IResolveOrder, crate::orders::IIssueOrder, INotifyRemovedFromWorld, INotifyEnteredCargo, INotifyExitedCargo, INotifyKilled, IObservesVariables);
info_impl!(PassengerInfo => |info, _init| Some(Rc::new(Passenger {
    info,
    transport: RefCell::new(None),
    require_force_move: Cell::new(false),
    any_cargo_token: Cell::new(INVALID_CONDITION_TOKEN),
    specific_cargo_token: Cell::new(INVALID_CONDITION_TOKEN),
    reserved_cargo: RefCell::new(None),
})));

impl IObservesVariables for Passenger {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        let Some(e) = &self.info.require_force_move_condition else {
            return Vec::new();
        };
        let w = Rc::downgrade(&self);
        let e2 = e.clone();
        vec![VariableObserver {
            notifier: Rc::new(move |_: &ActorRef, c: &ConditionCache| {
                if let Some(m) = w.upgrade() {
                    m.require_force_move.set(e2.evaluate(c));
                }
            }),
            variables: e.variables().iter().cloned().collect(),
        }]
    }
}

impl Passenger {
    fn can_enter(&self, target: &ActorRef) -> bool {
        target
            .trait_or_default::<Cargo>()
            .is_some_and(|c| !c.is_trait_disabled() && c.has_space(self.info.weight))
    }

    fn is_correct_cargo_type(&self, target: &ActorRef) -> bool {
        let cargo = target.trait_::<Cargo>();
        !cargo.is_trait_disabled() && self.info.cargo_type.as_ref().is_some_and(|t| cargo.info.types.contains(t))
    }

    pub fn reserve(&self, self_: &ActorRef, cargo: &Rc<Cargo>) -> bool {
        if self.reserved_cargo.borrow().as_ref().is_some_and(|r| Rc::ptr_eq(r, cargo)) {
            return true;
        }
        self.unreserve(self_);
        if !cargo.reserve_space(self_) {
            return false;
        }
        *self.reserved_cargo.borrow_mut() = Some(cargo.clone());
        true
    }

    pub fn unreserve(&self, self_: &ActorRef) {
        let Some(c) = self.reserved_cargo.borrow_mut().take() else { return };
        c.unreserve_space(self_);
    }

    pub fn on_before_added_to_world(&self, actor: &ActorRef) {
        actor.cancel_activity();
    }

    pub fn on_ejected_from_killed_cargo(&self, self_: &ActorRef) {
        if let Some(c) = self_.current_activity() {
            c.cancel(self_, false);
        }
        self_.queue_activity(Nudge::new(self_));
    }
}

impl INotifyEnteredCargo for Passenger {
    fn on_entered_cargo(&self, self_: &ActorRef, cargo: &ActorRef) {
        if self.any_cargo_token.get() == INVALID_CONDITION_TOKEN {
            self.any_cargo_token
                .set(self_.grant_condition(self.info.cargo_condition.as_deref().unwrap_or("")));
        }
        if self.specific_cargo_token.get() == INVALID_CONDITION_TOKEN
            && let Some(c) = self.info.cargo_conditions.get(&cargo.info.name)
        {
            self.specific_cargo_token.set(self_.grant_condition(c));
        }
        if self_.is_idle()
            && let Some(mv) = self_.trait_or_default::<dyn IMove>()
            && let Some(a) = mv.return_to_cell(self_)
        {
            self_.queue_activity(a);
        }
    }
}

impl INotifyExitedCargo for Passenger {
    fn on_exited_cargo(&self, self_: &ActorRef, _cargo: &ActorRef) {
        if self.any_cargo_token.get() != INVALID_CONDITION_TOKEN {
            self.any_cargo_token.set(self_.revoke_condition(self.any_cargo_token.get()));
        }
        if self.specific_cargo_token.get() != INVALID_CONDITION_TOKEN {
            self.specific_cargo_token
                .set(self_.revoke_condition(self.specific_cargo_token.get()));
        }
    }
}

impl IResolveOrder for Passenger {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "EnterTransport" || order.target.target_type() != TargetType::Actor {
            return;
        }
        let target_actor = order.target.actor().unwrap();
        if !self.can_enter(target_actor) || !self.is_correct_cargo_type(target_actor) {
            return;
        }
        self_.queue_activity_queued(order.queued, RideTransport::new(self_, &order.target));
    }
}

impl INotifyRemovedFromWorld for Passenger {
    fn removed_from_world(&self, self_: &ActorRef) {
        self.unreserve(self_);
    }
}

impl INotifyKilled for Passenger {
    fn killed(&self, self_: &ActorRef, _e: &AttackInfo) {
        let Some(transport) = self.transport.borrow().clone() else { return };
        if !transport.is_dead() {
            transport.trait_::<Cargo>().unload(&transport, Some(self_));
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ParaDropInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub drop_range: WDist,
    pub drop_interval: i32,
}

impl Default for ParaDropInfo {
    fn default() -> Self {
        ParaDropInfo {
            instance_name: None,
            drop_range: WDist::from_cells(4),
            drop_interval: 5,
        }
    }
}

pub type ActorEvent = Rc<dyn Fn(&ActorRef)>;

pub struct ParaDrop {
    info: Rc<ParaDropInfo>,
    self_: ActorRef,
    cargo: Rc<Cargo>,
    pub on_removed_from_world: RefCell<Vec<ActorEvent>>,
    pub on_entered_drop_range: RefCell<Vec<ActorEvent>>,
    pub on_exited_drop_range: RefCell<Vec<ActorEvent>>,
    in_drop_range: Cell<bool>,
    target: RefCell<Target>,
    drop_delay: Cell<i32>,
    check_for_suitable_cell: Cell<bool>,
}

register_traits!(ParaDrop: ITick, ISync, INotifyRemovedFromWorld);
info_impl!(ParaDropInfo => |info, init| Some(Rc::new(ParaDrop {
    info,
    self_: init.self_.clone(),
    cargo: init.self_.trait_::<Cargo>(),
    on_removed_from_world: RefCell::new(Vec::new()),
    on_entered_drop_range: RefCell::new(Vec::new()),
    on_exited_drop_range: RefCell::new(Vec::new()),
    in_drop_range: Cell::new(false),
    target: RefCell::new(Target::Invalid),
    drop_delay: Cell::new(0),
    check_for_suitable_cell: Cell::new(false),
})));

impl ISync for ParaDrop {
    fn sync_hash(&self) -> i32 {
        sync_bool(self.in_drop_range.get()) ^ self.target.borrow().sync_hash() ^ self.drop_delay.get()
    }
}

impl ParaDrop {
    pub fn set_lz(&self, lz: CPos, check_landing_cell: bool) {
        *self.target.borrow_mut() = Target::from_cell(&self.self_.world(), lz, FULL_CELL);
        self.check_for_suitable_cell.set(check_landing_cell);
    }
}

impl ITick for ParaDrop {
    fn tick(&self, self_: &ActorRef) {
        if self.drop_delay.get() > 0 {
            self.drop_delay.set(self.drop_delay.get() - 1);
            return;
        }
        let was_in_drop_range = self.in_drop_range.get();
        let in_range = self.target.borrow().is_in_range(self_.center_position(), self.info.drop_range);
        self.in_drop_range.set(in_range);
        if in_range && !was_in_drop_range {
            let events = self.on_entered_drop_range.borrow().clone();
            events.iter().for_each(|e| e(self_));
        }
        if !in_range && was_in_drop_range {
            let events = self.on_exited_drop_range.borrow().clone();
            events.iter().for_each(|e| e(self_));
        }
        let w = self_.world();
        if !in_range || self.cargo.is_empty() || !w.map().contains(self_.location()) {
            return;
        }
        let drop_actor = self.cargo.peek();
        let drop_positionable = drop_actor.trait_::<dyn IPositionable>();
        let drop_cell = self_.location();
        let mut drop_sub_cell = drop_positionable.get_available_subcell(drop_cell, ANY, None, BlockedByActor::All);
        if drop_sub_cell == INVALID {
            if self.check_for_suitable_cell.get() {
                return;
            }
            drop_sub_cell = ANY;
        }
        let unloaded = self.cargo.unload(self_, None);
        assert!(unloaded.id == drop_actor.id, "Peeked cargo was not unloaded!");
        let z = self_.center_position().z;
        w.add_frame_end_task(Box::new(move |w| {
            drop_positionable.set_position_cell(&drop_actor, drop_cell, drop_sub_cell);
            let drop_position: WPos = drop_actor.center_position() + WVec::new(0, 0, z - drop_actor.center_position().z);
            drop_positionable.set_center_position(&drop_actor, drop_position);
            w.add(&drop_actor);
        }));
        self.drop_delay.set(self.info.drop_interval);
    }
}

impl INotifyRemovedFromWorld for ParaDrop {
    fn removed_from_world(&self, self_: &ActorRef) {
        let events = self.on_removed_from_world.borrow().clone();
        events.iter().for_each(|e| e(self_));
    }
}

/// Virtual members of the abstract C# `Enter` activity.
pub trait EnterHooks {
    fn type_name(&self) -> &'static str;
    fn tick_inner(&self, _enter: &Enter, _self_: &ActorRef, _target: &Target, _target_is_dead_or_hidden_actor: bool) {}
    fn try_start_enter(&self, _enter: &Enter, _self_: &ActorRef, _target_actor: &ActorRef) -> bool {
        true
    }
    fn on_enter_complete(&self, _self_: &ActorRef, _target_actor: &ActorRef) {}
    fn on_last_run(&self, _self_: &ActorRef) {}
    fn on_cancel(&self, _self_: &ActorRef) {}
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnterState {
    Approaching,
    Entering,
    Exiting,
    Finished,
}

pub struct Enter {
    base: ActivityBase,
    mv: Rc<dyn IMove>,
    helper: MoveCooldownHelper,
    target: RefCell<Target>,
    last_visible_target: RefCell<Target>,
    use_last_visible_target: Cell<bool>,
    last_state: Cell<EnterState>,
    hooks: Box<dyn EnterHooks>,
}

impl Enter {
    pub fn new(self_: &ActorRef, target: &Target, hooks: Box<dyn EnterHooks>) -> ActivityRef {
        let base = ActivityBase::default();
        base.child_has_priority.set(false);
        let helper = MoveCooldownHelper::new(self_.trait_or_default::<Mobile>());
        helper.retry_if_destination_blocked.set(true);
        Rc::new(Enter {
            base,
            mv: self_.trait_::<dyn IMove>(),
            helper,
            target: RefCell::new(target.clone()),
            last_visible_target: RefCell::new(Target::Invalid),
            use_last_visible_target: Cell::new(false),
            last_state: Cell::new(EnterState::Approaching),
            hooks,
        })
    }

    pub fn cancel_keep_queue(&self, self_: &ActorRef) {
        self.cancel(self_, true);
    }
}

impl Activity for Enter {
    fn base(&self) -> &ActivityBase {
        &self.base
    }

    fn type_name(&self) -> &'static str {
        self.hooks.type_name()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let (target, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = target.clone();
        if !hidden && target.target_type() == TargetType::Actor {
            *self.last_visible_target.borrow_mut() = Target::from_target_positions(&target);
        }
        let use_last = hidden || !target.is_valid_for(self_);
        self.use_last_visible_target.set(use_last);
        if !self.base.is_canceling() && use_last && self.last_state.get() == EnterState::Entering {
            self.cancel(self_, true);
        }
        self.hooks.tick_inner(self, self_, &target, use_last);
        if !self.base.tick_child(self_) {
            return false;
        }
        if let Some(r) = self.helper.tick(&self_.world(), hidden) {
            return r;
        }
        match self.last_state.get() {
            EnterState::Approaching => {
                if self.base.is_canceling() {
                    return true;
                }
                if use_last && self.last_visible_target.borrow().target_type() == TargetType::Invalid {
                    return true;
                }
                if target.target_type() != TargetType::Invalid && !self.mv.can_enter_target_now(self_, &target) {
                    self.helper.notify_move_queued();
                    let initial = if use_last {
                        self.last_visible_target.borrow().center_position()
                    } else {
                        target.center_position()
                    };
                    if let Some(a) = self.mv.move_to_target(self_, &target, Some(initial)) {
                        self.base.queue_child(a);
                    }
                    return false;
                }
                if use_last || target.target_type() != TargetType::Actor {
                    return true;
                }
                if self.hooks.try_start_enter(self, self_, target.actor().unwrap()) {
                    self.helper.notify_move_queued();
                    self.last_state.set(EnterState::Entering);
                    if let Some(a) = self.mv.move_into_target(self_, &target) {
                        self.base.queue_child(a);
                    }
                    return false;
                }
                self.base.is_canceling()
            }
            EnterState::Entering => {
                let target_pos = util::closest_position(&target.positions(), self_.center_position());
                if !self.base.is_canceling() && Some(self_.center_position()) == target_pos && target.target_type() == TargetType::Actor {
                    self.hooks.on_enter_complete(self_, target.actor().unwrap());
                }
                self.last_state.set(EnterState::Exiting);
                false
            }
            EnterState::Exiting => {
                self.helper.notify_move_queued();
                if let Some(a) = self.mv.return_to_cell(self_) {
                    self.base.queue_child(a);
                }
                self.last_state.set(EnterState::Finished);
                false
            }
            EnterState::Finished => true,
        }
    }

    fn on_last_run(&self, self_: &ActorRef) {
        self.hooks.on_last_run(self_);
    }

    fn cancel(&self, self_: &ActorRef, keep_queue: bool) {
        self.hooks.on_cancel(self_);
        self.base.cancel(self_, keep_queue);
    }

    fn get_targets(&self, _self_: &ActorRef) -> Vec<Target> {
        vec![self.target.borrow().clone()]
    }
}

struct RideTransportHooks {
    passenger: Rc<Passenger>,
    enter_actor: RefCell<Option<ActorRef>>,
    enter_cargo: RefCell<Option<Rc<Cargo>>>,
}

/// `Activities/RideTransport.cs`.
pub struct RideTransport;

impl RideTransport {
    pub fn new(self_: &ActorRef, target: &Target) -> ActivityRef {
        Enter::new(
            self_,
            target,
            Box::new(RideTransportHooks {
                passenger: self_.trait_::<Passenger>(),
                enter_actor: RefCell::new(None),
                enter_cargo: RefCell::new(None),
            }),
        )
    }
}

impl EnterHooks for RideTransportHooks {
    fn type_name(&self) -> &'static str {
        "RideTransport"
    }

    fn try_start_enter(&self, enter: &Enter, self_: &ActorRef, target_actor: &ActorRef) -> bool {
        *self.enter_actor.borrow_mut() = Some(target_actor.clone());
        let cargo = target_actor.trait_or_default::<Cargo>();
        *self.enter_cargo.borrow_mut() = cargo.clone();
        let reserved = cargo
            .as_ref()
            .is_some_and(|c| !c.is_trait_disabled() && self.passenger.reserve(self_, c));
        if !reserved {
            enter.cancel_keep_queue(self_);
            return false;
        }
        target_actor.trait_or_default::<Aircraft>().is_none_or(|a| a.at_land_altitude())
    }

    fn tick_inner(&self, enter: &Enter, self_: &ActorRef, _target: &Target, _hidden: bool) {
        if self.enter_cargo.borrow().as_ref().is_some_and(|c| c.is_trait_disabled()) {
            enter.cancel_keep_queue(self_);
        }
    }

    fn on_enter_complete(&self, self_: &ActorRef, target_actor: &ActorRef) {
        let (me, target) = (self_.clone(), target_actor.clone());
        let enter_actor = self.enter_actor.borrow().clone();
        let enter_cargo = self.enter_cargo.borrow().clone();
        self_.world().add_frame_end_task(Box::new(move |w| {
            if me.is_dead() || enter_actor.as_ref().is_none_or(|e| e.id != target.id) {
                return;
            }
            let (enter_actor, cargo) = (enter_actor.unwrap(), enter_cargo.expect("enter cargo"));
            if !cargo.can_load(&me) {
                return;
            }
            for inl in target.traits_implementing::<dyn INotifyLoadCargo>() {
                inl.loading(&me);
            }
            cargo.load(&enter_actor, &me);
            w.remove(&me);
        }));
    }

    fn on_last_run(&self, self_: &ActorRef) {
        self.passenger.unreserve(self_);
    }

    fn on_cancel(&self, self_: &ActorRef) {
        self.passenger.unreserve(self_);
    }
}

/// `Activities/UnloadCargo.cs`.
pub struct UnloadCargo {
    base: ActivityBase,
    cargo: Rc<Cargo>,
    notifiers: Vec<Rc<dyn INotifyUnloadCargo>>,
    unload_all: bool,
    aircraft: Option<Rc<Aircraft>>,
    mobile: Option<Rc<Mobile>>,
    assign_target_on_first_run: bool,
    unload_range: WDist,
    destination: RefCell<Target>,
    take_off_after_unload: Cell<bool>,
}

impl UnloadCargo {
    pub fn new(self_: &ActorRef, unload_range: WDist) -> ActivityRef {
        Self::make(self_, Target::Invalid, unload_range, true, true)
    }

    pub fn with_destination(self_: &ActorRef, destination: Target, unload_range: WDist, unload_all: bool) -> ActivityRef {
        Self::make(self_, destination, unload_range, unload_all, false)
    }

    fn make(self_: &ActorRef, destination: Target, unload_range: WDist, unload_all: bool, assign_target_on_first_run: bool) -> ActivityRef {
        Rc::new(UnloadCargo {
            base: ActivityBase::default(),
            cargo: self_.trait_::<Cargo>(),
            notifiers: self_.traits_implementing::<dyn INotifyUnloadCargo>(),
            unload_all,
            aircraft: self_.trait_or_default::<Aircraft>(),
            mobile: self_.trait_or_default::<Mobile>(),
            assign_target_on_first_run,
            unload_range,
            destination: RefCell::new(destination),
            take_off_after_unload: Cell::new(false),
        })
    }

    /// `ChooseExitSubCell`: the `Shuffle` iterator draws lazily and stops at the first free cell.
    fn choose_exit_sub_cell(&self, self_: &ActorRef, passenger: &ActorRef) -> Option<(CPos, u8)> {
        let pos = passenger.trait_::<dyn IPositionable>();
        let mut items = self.cargo.current_adjacent_cells();
        let w = self_.world();
        let n = items.len();
        for i in 0..n {
            let j = i + w.shared_random.borrow_mut().next_max((n - i) as i32) as usize;
            items.swap(i, j);
            let c = items[i];
            let sub = pos.get_available_subcell(c, ANY, None, BlockedByActor::All);
            if sub != INVALID {
                return Some((c, sub));
            }
        }
        None
    }
}

impl Activity for UnloadCargo {
    activity_common!("UnloadCargo");

    fn on_first_run(&self, self_: &ActorRef) {
        let w = self_.world();
        if self.assign_target_on_first_run {
            *self.destination.borrow_mut() = Target::from_cell(&w, self_.location(), FULL_CELL);
        }
        let destination = self.destination.borrow().clone();
        if let Some(a) = &self.aircraft {
            self.base.queue_child(Land::full(
                self_,
                destination,
                LandArgs {
                    land_range: self.unload_range,
                    ..Default::default()
                },
            ));
            self.take_off_after_unload.set(!a.at_land_altitude());
        } else if self.mobile.is_some() {
            let cell = w.map().clamp(w.map().cell_containing(destination.center_position()));
            self.base.queue_child(Move::new_near(self_, cell, self.unload_range, None, false));
        }
        self.base.queue_child(Wait::new(self.cargo.info.before_unload_delay));
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() || self.cargo.is_empty() {
            return true;
        }
        if self.cargo.can_unload(BlockedByActor::None) {
            for inu in &self.notifiers {
                inu.unloading(self_);
            }
            let actor = self.cargo.peek();
            let spawn = self_.center_position();
            let Some((cell, sub)) = self.choose_exit_sub_cell(self_, &actor) else {
                let pos = actor.trait_::<dyn IPositionable>();
                let blocked: Vec<CPos> = self
                    .cargo
                    .current_adjacent_cells()
                    .into_iter()
                    .filter(|c| pos.can_enter_cell(*c, None, BlockedByActor::All) != pos.can_enter_cell(*c, None, BlockedByActor::None))
                    .collect();
                notify_blocker_cells(self_, &blocked);
                self.base.queue_child(Wait::new(10));
                return false;
            };
            self.cargo.unload(self_, None);
            self_.world().add_frame_end_task(Box::new(move |w| {
                if actor.disposed() {
                    return;
                }
                let pos = actor.trait_::<dyn IPositionable>();
                let passenger = actor.trait_::<Passenger>();
                pos.set_position_cell(&actor, cell, sub);
                pos.set_center_position(&actor, spawn);
                passenger.on_before_added_to_world(&actor);
                w.add(&actor);
            }));
        }
        if !self.unload_all || !self.cargo.can_unload(BlockedByActor::None) {
            if self.cargo.info.after_unload_delay > 0 {
                self.base
                    .queue_child(Wait::new_interruptible(self.cargo.info.after_unload_delay, false));
            }
            if self.take_off_after_unload.get() {
                self.base.queue_child(TakeOff::new(self_));
            }
            return true;
        }
        false
    }

    fn get_targets(&self, _self_: &ActorRef) -> Vec<Target> {
        vec![self.destination.borrow().clone()]
    }
}

impl crate::orders::IIssueOrder for Cargo {
    /// The cursor is chosen when the targeter is made instead of when it targets; nothing changes in between.
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn crate::orders::IOrderTargeter>> {
        if self.is_trait_disabled() {
            return Vec::new();
        }
        let cursor = if self.can_unload(BlockedByActor::None) {
            &self.info.unload_cursor
        } else {
            &self.info.unload_blocked_cursor
        }
        .clone();
        vec![Box::new(crate::orders::DeployOrderTargeter::new(
            "Unload",
            10,
            Box::new(move || cursor.clone()),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn crate::orders::IOrderTargeter, _target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "Unload").then(|| Order::new("Unload", Some(self_.clone()), Target::Invalid, queued))
    }
}

impl crate::orders::IIssueOrder for Passenger {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn crate::orders::IOrderTargeter>> {
        let require_force_move = self.require_force_move.get();
        let (cargo_type, weight) = (self.info.cargo_type.clone(), self.info.weight);
        vec![Box::new(crate::orders::UnitOrderTargeter::enter_allied(
            "EnterTransport",
            5,
            &self.info.enter_cursor,
            &self.info.enter_blocked_cursor,
            |t| t.info.has_trait_info::<CargoInfo>(),
            Box::new(move |t, modifiers| {
                if require_force_move && !modifiers.has_modifier(crate::orders::TargetModifiers::FORCE_MOVE) {
                    return false;
                }
                let cargo = t.trait_::<Cargo>();
                !cargo.is_trait_disabled() && cargo_type.as_ref().is_some_and(|c| cargo.info.types.contains(c))
            }),
            Box::new(move |t| {
                t.trait_or_default::<Cargo>()
                    .is_some_and(|c| !c.is_trait_disabled() && c.has_space(weight))
            }),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn crate::orders::IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "EnterTransport").then(|| Order::new("EnterTransport", Some(self_.clone()), target.clone(), queued))
    }
}
