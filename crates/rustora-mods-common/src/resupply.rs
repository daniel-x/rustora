// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/AmmoPool.cs`, `Traits/Rearmable.cs`, `Traits/Repairable.cs`, `Traits/RepairableNear.cs`,
//! `Traits/RepairsUnits.cs` and `Activities/Resupply.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_core::expression::BooleanExpression;
use rustora_core::world_types::{WDist, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef, for_each_activity};
use rustora_engine::actor::{ActorRef, ConditionCache};
use rustora_engine::conditional::*;
use rustora_engine::damage::{Damage, DamageState, DamageTypes};
use rustora_engine::order::Order;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::{activity_common, conditional_simple, info_impl, load_info, register_traits};

use crate::activities::{Move, MoveCooldownHelper};
use crate::aircraft::{Aircraft, TakeOff};
use crate::attack_move::AttackMoveActivity;
use crate::base::PlayerExperience;
use crate::interfaces::*;
use crate::mobile::Mobile;
use crate::player::PlayerResources;
use crate::production::{RallyPoint, ValuedInfo};

pub fn register(r: &mut TraitRegistry) {
    r.register("AmmoPool", |n| load_info!(AmmoPoolInfo, n));
    r.register("Rearmable", |n| load_info!(RearmableInfo, n));
    r.register("Repairable", |n| load_info!(RepairableInfo, n));
    r.register("RepairableNear", |n| load_info!(RepairableNearInfo, n));
    r.register("RepairsUnits", |n| load_info!(RepairsUnitsInfo, n));
}

/// `self.GetDamageState() != DamageState.Undamaged`.
pub fn has_damage(a: &ActorRef) -> bool {
    a.cache()
        .health
        .as_ref()
        .is_some_and(|h| h.damage_state() != DamageState::UNDAMAGED)
}

#[derive(Clone, Debug, FieldLoad)]
pub struct AmmoPoolInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub name: String,
    pub armaments: Vec<String>,
    pub ammo: i32,
    pub initial_ammo: i32,
    pub reload_count: i32,
    pub reload_delay: i32,
    pub ammo_condition: Option<String>,
}

impl Default for AmmoPoolInfo {
    fn default() -> Self {
        AmmoPoolInfo {
            instance_name: None,
            name: "primary".to_string(),
            armaments: vec!["primary".to_string(), "secondary".to_string()],
            ammo: 1,
            initial_ammo: -1,
            reload_count: 1,
            reload_delay: 50,
            ammo_condition: None,
        }
    }
}

pub struct AmmoPool {
    pub info: Rc<AmmoPoolInfo>,
    tokens: RefCell<Vec<i32>>,
    pub remaining_ticks: Cell<i32>,
    current_ammo_count: Cell<i32>,
}

register_traits!(AmmoPool: INotifyCreated, INotifyAttack, ISync);
info_impl!(AmmoPoolInfo => |info, _init| {
    let ammo = if info.initial_ammo < info.ammo && info.initial_ammo >= 0 { info.initial_ammo } else { info.ammo };
    Some(Rc::new(AmmoPool { info, tokens: RefCell::new(Vec::new()), remaining_ticks: Cell::new(0), current_ammo_count: Cell::new(ammo) }))
});

impl ISync for AmmoPool {
    fn sync_hash(&self) -> i32 {
        self.remaining_ticks.get() ^ self.current_ammo_count.get()
    }
}

impl INotifyCreated for AmmoPool {
    fn created(&self, self_: &ActorRef) {
        self.update_condition(self_);
        self.remaining_ticks.set(self.info.reload_delay);
    }
}

impl INotifyAttack for AmmoPool {
    fn attacking(&self, self_: &ActorRef, _target: &Target, a: &crate::attack::Armament, _barrel: &crate::attack::Barrel) {
        if self.info.armaments.contains(&a.info.name) {
            self.take_ammo(self_, a.info.ammo_usage);
        }
    }

    fn preparing_attack(&self, _self_: &ActorRef, _target: &Target, _a: &crate::attack::Armament, _barrel: &crate::attack::Barrel) {}
}

impl AmmoPool {
    pub fn current_ammo_count(&self) -> i32 {
        self.current_ammo_count.get()
    }

    pub fn has_ammo(&self) -> bool {
        self.current_ammo_count.get() > 0
    }

    pub fn has_full_ammo(&self) -> bool {
        self.current_ammo_count.get() == self.info.ammo
    }

    pub fn give_ammo(&self, self_: &ActorRef, count: i32) -> bool {
        if self.current_ammo_count.get() >= self.info.ammo || count < 0 {
            return false;
        }
        self.current_ammo_count
            .set((self.current_ammo_count.get() + count).clamp(0, self.info.ammo));
        self.update_condition(self_);
        true
    }

    pub fn take_ammo(&self, self_: &ActorRef, count: i32) -> bool {
        if self.current_ammo_count.get() <= 0 || count < 0 {
            return false;
        }
        self.current_ammo_count
            .set((self.current_ammo_count.get() - count).clamp(0, self.info.ammo));
        self.update_condition(self_);
        true
    }

    fn update_condition(&self, self_: &ActorRef) {
        let Some(condition) = self.info.ammo_condition.as_deref().filter(|c| !c.is_empty()) else {
            return;
        };
        while self.current_ammo_count.get() > self.tokens.borrow().len() as i32 && (self.tokens.borrow().len() as i32) < self.info.ammo {
            let t = self_.grant_condition(condition);
            self.tokens.borrow_mut().push(t);
        }
        while self.current_ammo_count.get() < self.tokens.borrow().len() as i32 && !self.tokens.borrow().is_empty() {
            let t = self.tokens.borrow_mut().pop().unwrap();
            self_.revoke_condition(t);
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RearmableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub rearm_actors: IndexSet<String>,
    pub ammo_pools: IndexSet<String>,
}

impl Default for RearmableInfo {
    fn default() -> Self {
        RearmableInfo {
            instance_name: None,
            rearm_actors: IndexSet::new(),
            ammo_pools: IndexSet::from(["primary".to_string()]),
        }
    }
}

pub struct Rearmable {
    pub info: Rc<RearmableInfo>,
    rearmable_ammo_pools: RefCell<Vec<Rc<AmmoPool>>>,
}

register_traits!(Rearmable: INotifyCreated, INotifyDockClient);
info_impl!(RearmableInfo => |info, _init| Some(Rc::new(Rearmable { info, rearmable_ammo_pools: RefCell::new(Vec::new()) })));

impl INotifyCreated for Rearmable {
    fn created(&self, self_: &ActorRef) {
        *self.rearmable_ammo_pools.borrow_mut() = self_
            .traits_implementing::<AmmoPool>()
            .into_iter()
            .filter(|p| self.info.ammo_pools.contains(&p.info.name))
            .collect();
    }
}

impl INotifyDockClient for Rearmable {
    fn docked(&self, _self_: &ActorRef, _host: &ActorRef) {
        for pool in self.rearmable_ammo_pools.borrow().iter() {
            pool.remaining_ticks.set(pool.info.reload_delay);
        }
    }

    fn undocked(&self, _self_: &ActorRef, _host: &ActorRef) {}
}

impl Rearmable {
    pub fn rearmable_ammo_pools(&self) -> Vec<Rc<AmmoPool>> {
        self.rearmable_ammo_pools.borrow().clone()
    }

    pub fn rearm_tick(&self, self_: &ActorRef) -> bool {
        let mut rearm_complete = true;
        for pool in self.rearmable_ammo_pools.borrow().iter() {
            if !pool.has_full_ammo() {
                pool.remaining_ticks.set(pool.remaining_ticks.get() - 1);
                if pool.remaining_ticks.get() <= 0 {
                    pool.remaining_ticks.set(pool.info.reload_delay);
                    pool.give_ammo(self_, pool.info.reload_count);
                }
                rearm_complete = false;
            }
        }
        rearm_complete
    }
}

/// Force-move state shared by `Repairable` and `RepairableNear`.
fn force_move_observer<T: 'static>(this: &Rc<T>, e: &Option<BooleanExpression>, cell: fn(&T) -> &Cell<bool>) -> Vec<VariableObserver> {
    let Some(e) = e else { return Vec::new() };
    let w = Rc::downgrade(this);
    let e2 = e.clone();
    vec![VariableObserver {
        notifier: Rc::new(move |_: &ActorRef, c: &ConditionCache| {
            if let Some(m) = w.upgrade() {
                cell(&m).set(e2.evaluate(c));
            }
        }),
        variables: e.variables().iter().cloned().collect(),
    }]
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RepairableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub repair_actors: IndexSet<String>,
    pub hp_per_step: i32,
    pub require_force_move_condition: Option<BooleanExpression>,
}

impl Default for RepairableInfo {
    fn default() -> Self {
        RepairableInfo {
            instance_name: None,
            repair_actors: IndexSet::new(),
            hp_per_step: -1,
            require_force_move_condition: None,
        }
    }
}

pub struct Repairable {
    pub info: Rc<RepairableInfo>,
    rearmable: OnceCell<Option<Rc<Rearmable>>>,
    require_force_move: Cell<bool>,
    is_aircraft: Cell<bool>,
}

register_traits!(Repairable: IResolveOrder, INotifyCreated, IObservesVariables);
info_impl!(RepairableInfo => |info, _init| Some(Rc::new(Repairable {
    info,
    rearmable: OnceCell::new(),
    require_force_move: Cell::new(false),
    is_aircraft: Cell::new(false),
})));

impl INotifyCreated for Repairable {
    fn created(&self, self_: &ActorRef) {
        let _ = self.rearmable.set(self_.trait_or_default::<Rearmable>());
        self.is_aircraft.set(self_.info.has_trait_info::<crate::aircraft::AircraftInfo>());
    }
}

impl IObservesVariables for Repairable {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        force_move_observer(&self, &self.info.require_force_move_condition, |s| &s.require_force_move)
    }
}

impl Repairable {
    fn can_rearm(&self) -> bool {
        self.rearmable
            .get()
            .and_then(|r| r.as_ref())
            .is_some_and(|r| r.rearmable_ammo_pools().iter().any(|p| !p.has_full_ammo()))
    }

    /// `FindRepairBuilding`.
    pub fn find_repair_building(&self, self_: &ActorRef) -> Option<ActorRef> {
        find_repair_building(self_, &self.info.repair_actors)
    }
}

impl IResolveOrder for Repairable {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "Repair" || order.target.target_type() != TargetType::Actor || self.is_aircraft.get() {
            return;
        }
        let host = order.target.actor().unwrap();
        if !self.info.repair_actors.contains(&host.info.name) || (!has_damage(self_) && !self.can_rearm()) {
            return;
        }
        self_.queue_activity_queued(order.queued, Resupply::new(self_, host, WDist::new(512), false));
    }
}

fn find_repair_building(self_: &ActorRef, repair_actors: &IndexSet<String>) -> Option<ActorRef> {
    let w = self_.world();
    let mut buildings: Vec<ActorRef> = w
        .actors_having_trait::<RepairsUnits>()
        .into_iter()
        .filter(|a| {
            !a.is_dead() && a.is_in_world() && a.owner().is_allied_with(Some(&self_.owner())) && repair_actors.contains(&a.info.name)
        })
        .collect();
    buildings.sort_by_key(|a| {
        (
            if Rc::ptr_eq(&a.owner(), &self_.owner()) { 0 } else { 1 },
            (self_.location() - a.location()).length_squared(),
        )
    });
    buildings.into_iter().next()
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RepairableNearInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub repair_actors: IndexSet<String>,
    pub close_enough: WDist,
    pub require_force_move_condition: Option<BooleanExpression>,
}

impl Default for RepairableNearInfo {
    fn default() -> Self {
        RepairableNearInfo {
            instance_name: None,
            repair_actors: IndexSet::new(),
            close_enough: WDist::from_cells(4),
            require_force_move_condition: None,
        }
    }
}

pub struct RepairableNear {
    pub info: Rc<RepairableNearInfo>,
    require_force_move: Cell<bool>,
}

register_traits!(RepairableNear: IResolveOrder, IObservesVariables);
info_impl!(RepairableNearInfo => |info, _init| Some(Rc::new(RepairableNear { info, require_force_move: Cell::new(false) })));

impl IObservesVariables for RepairableNear {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        force_move_observer(&self, &self.info.require_force_move_condition, |s| &s.require_force_move)
    }
}

impl RepairableNear {
    /// `FindRepairBuilding`.
    pub fn find_repair_building(&self, self_: &ActorRef) -> Option<ActorRef> {
        find_repair_building(self_, &self.info.repair_actors)
    }
}

impl IResolveOrder for RepairableNear {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "RepairNear" || order.target.target_type() != TargetType::Actor {
            return;
        }
        let host = order.target.actor().unwrap();
        if !self.info.repair_actors.contains(&host.info.name) || !has_damage(self_) {
            return;
        }
        self_.queue_activity_queued(order.queued, Resupply::new(self_, host, self.info.close_enough, false));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RepairsUnitsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub value_percentage: i32,
    pub hp_per_step: i32,
    pub interval: i32,
    pub repair_damage_types: DamageTypes,
    pub player_experience: i32,
}

impl Default for RepairsUnitsInfo {
    fn default() -> Self {
        RepairsUnitsInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            value_percentage: 20,
            hp_per_step: 10,
            interval: 24,
            repair_damage_types: DamageTypes::default(),
            player_experience: 0,
        }
    }
}

pub struct RepairsUnits {
    pub info: Rc<RepairsUnitsInfo>,
    state: ConditionalState,
}

register_traits!(RepairsUnits: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(RepairsUnitsInfo => |info, _init| {
    let state = ConditionalState::new_pausable(&info.base);
    Some(Rc::new(RepairsUnits { info, state }))
});
conditional_simple!(RepairsUnits);

const RESUPPLY_REPAIR: u8 = 1;
const RESUPPLY_REARM: u8 = 2;

pub struct Resupply {
    base: ActivityBase,
    health: Option<Rc<dyn IHealth>>,
    all_repairs_units: Vec<Rc<RepairsUnits>>,
    host: Target,
    close_enough: WDist,
    repairable: Option<Rc<Repairable>>,
    repairable_near: Option<Rc<RepairableNear>>,
    rearmable: Option<Rc<Rearmable>>,
    notify_dock_hosts: Vec<Rc<dyn INotifyDockHost>>,
    notify_dock_clients: Vec<Rc<dyn INotifyDockClient>>,
    mv: Rc<dyn IMove>,
    aircraft: Option<Rc<Aircraft>>,
    stay_on_resupplier: bool,
    was_repaired: bool,
    player_resources: Rc<PlayerResources>,
    unit_cost: i32,
    helper: MoveCooldownHelper,
    remaining_ticks: Cell<i32>,
    actual_resupply_started: Cell<bool>,
    active_resupply_types: Cell<u8>,
}

impl Resupply {
    pub fn new(self_: &ActorRef, host: &ActorRef, close_enough: WDist, stay_on_resupplier: bool) -> ActivityRef {
        let all_repairs_units = host.traits_implementing::<RepairsUnits>();
        let health = self_.cache().health.clone();
        let repairable = self_.trait_or_default::<Repairable>();
        let repairable_near = self_.trait_or_default::<RepairableNear>();
        let rearmable = self_.trait_or_default::<Rearmable>();
        let helper = MoveCooldownHelper::new(self_.trait_or_default::<Mobile>());
        helper.retry_if_destination_blocked.set(true);

        let name = &host.info.name;
        let cannot_repair_at_host = health.as_ref().is_none_or(|h| h.damage_state() == DamageState::UNDAMAGED)
            || all_repairs_units.is_empty()
            || (repairable.as_ref().is_none_or(|r| !r.info.repair_actors.contains(name))
                && repairable_near.as_ref().is_none_or(|r| !r.info.repair_actors.contains(name)));
        let mut active = 0;
        if !cannot_repair_at_host {
            active |= RESUPPLY_REPAIR;
        }
        let cannot_rearm_at_host = rearmable
            .as_ref()
            .is_none_or(|r| !r.info.rearm_actors.contains(name) || r.rearmable_ammo_pools().iter().all(|p| p.has_full_ammo()));
        if !cannot_rearm_at_host {
            active |= RESUPPLY_REARM;
        }
        Rc::new(Resupply {
            base: ActivityBase::default(),
            health,
            all_repairs_units,
            host: Target::from_actor(Some(host)),
            close_enough,
            repairable,
            repairable_near,
            rearmable,
            notify_dock_hosts: host.traits_implementing::<dyn INotifyDockHost>(),
            notify_dock_clients: self_.traits_implementing::<dyn INotifyDockClient>(),
            mv: self_.trait_::<dyn IMove>(),
            aircraft: self_.trait_or_default::<Aircraft>(),
            stay_on_resupplier,
            was_repaired: !cannot_repair_at_host,
            player_resources: self_.owner().player_actor().trait_::<PlayerResources>(),
            unit_cost: self_.info.trait_info::<ValuedInfo>().map_or(0, |v| v.cost),
            helper,
            remaining_ticks: Cell::new(0),
            actual_resupply_started: Cell::new(false),
            active_resupply_types: Cell::new(active),
        })
    }

    fn host_actor(&self) -> &ActorRef {
        self.host.actor().expect("resupply host")
    }

    fn on_resupply_ending(&self, self_: &ActorRef, is_host_invalid: bool) {
        let rp = if is_host_invalid {
            None
        } else {
            self.host_actor().trait_or_default::<RallyPoint>()
        };
        let rally_path = rp.as_ref().map(|r| r.path.borrow().clone()).unwrap_or_default();
        let next_is_none = self_.current_activity().is_some_and(|c| c.base().next_activity().is_none());
        let ignore_host = if self.repairable_near.is_some() || is_host_invalid {
            None
        } else {
            Some(self.host_actor().clone())
        };
        let queue_rally = |evaluate: bool| {
            for cell in &rally_path {
                let (mv, c, ignore) = (self.mv.clone(), *cell, ignore_host.clone());
                self.base.queue_child(AttackMoveActivity::new(
                    self_,
                    Rc::new(move || mv.move_to(c, 1, ignore.clone(), evaluate)),
                    false,
                ));
            }
        };
        if let Some(aircraft) = &self.aircraft {
            if self.was_repaired || is_host_invalid || (!self.stay_on_resupplier && aircraft.info.take_off_on_resupply) {
                if next_is_none && !rally_path.is_empty() {
                    self.helper.notify_move_queued();
                    queue_rally(false);
                } else {
                    self.base.queue_child(TakeOff::new(self_));
                }
                aircraft.un_reserve();
            } else {
                aircraft.allow_yielding_reservation();
            }
        } else if !self.stay_on_resupplier && !is_host_invalid {
            self.helper.notify_move_queued();
            if next_is_none {
                if !rally_path.is_empty() {
                    queue_rally(true);
                } else if self.repairable_near.is_none() {
                    if let Some(a) = self.mv.move_to_target(self_, &self.host, None) {
                        self.base.queue_child(a);
                    }
                }
            } else if self.repairable_near.is_none()
                && self_
                    .current_activity()
                    .and_then(|c| c.base().next_activity())
                    .is_some_and(|n| n.type_name() != "Move")
            {
                if let Some(a) = self.mv.move_to_target(self_, &self.host, None) {
                    self.base.queue_child(a);
                }
            }
        }
        if !is_host_invalid {
            let host = self.host_actor();
            for nd in &self.notify_dock_clients {
                nd.undocked(self_, host);
            }
            for nd in &self.notify_dock_hosts {
                nd.undocked(host, self_);
            }
        }
    }

    fn repair_tick(&self, self_: &ActorRef) {
        let repairs_units = self
            .all_repairs_units
            .iter()
            .find(|r| !r.state.is_trait_disabled() && !r.state.is_trait_paused());
        let Some(repairs_units) = repairs_units else {
            if !self.all_repairs_units.iter().any(|r| r.state.is_trait_paused()) {
                self.active_resupply_types.set(self.active_resupply_types.get() & !RESUPPLY_REPAIR);
            }
            return;
        };
        let health = self.health.as_ref().expect("repair without health");
        if health.damage_state() == DamageState::UNDAMAGED {
            let host = self.host_actor();
            if !Rc::ptr_eq(&host.owner(), &self_.owner()) {
                if let Some(xp) = host.owner().player_actor().trait_or_default::<PlayerExperience>() {
                    xp.give_experience(repairs_units.info.player_experience);
                }
            }
            self.active_resupply_types.set(self.active_resupply_types.get() & !RESUPPLY_REPAIR);
            return;
        }
        if self.remaining_ticks.get() == 0 {
            let hp_to_repair = match &self.repairable {
                Some(r) if r.info.hp_per_step > 0 => r.info.hp_per_step,
                _ => repairs_units.info.hp_per_step,
            };
            let value = self.unit_cost as i64 * repairs_units.info.value_percentage as i64;
            let cost = if value == 0 {
                0
            } else {
                1.max((hp_to_repair as i64 * value / (health.max_hp() as i64 * 100)) as i32)
            };
            if !self.player_resources.take_cash(cost, true) {
                self.remaining_ticks.set(1);
                return;
            }
            health.inflict_damage(
                self_,
                Some(self.host_actor()),
                &Damage::new(-hp_to_repair, repairs_units.info.repair_damage_types),
                false,
            );
            self.remaining_ticks.set(repairs_units.info.interval);
        } else {
            self.remaining_ticks.set(self.remaining_ticks.get() - 1);
        }
    }
}

impl Activity for Resupply {
    activity_common!("Resupply");

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() && self.remaining_ticks.get() > 0 {
            self.remaining_ticks.set(self.remaining_ticks.get() - 1);
            return false;
        }
        let is_host_invalid = self.host.target_type() != TargetType::Actor || !self.host_actor().is_in_world();
        let mut is_close_enough = false;
        if !is_host_invalid {
            if self.close_enough < WDist::ZERO {
                is_close_enough = true;
            } else if self.repairable_near.is_some() {
                is_close_enough = self.host.is_in_range(self_.center_position(), self.close_enough);
            } else {
                is_close_enough = (self.host.center_position() - self_.center_position()).horizontal_length_squared()
                    <= self.close_enough.length_squared();
            }
        }
        if !self.base.is_canceling() && is_host_invalid {
            self.cancel(self_, true);
        }
        if self.base.is_canceling() || is_host_invalid {
            if is_close_enough || is_host_invalid {
                self.on_resupply_ending(self_, is_host_invalid);
            }
            return true;
        }
        if let Some(r) = self.helper.tick(&self_.world(), false) {
            return r;
        }
        if self.active_resupply_types.get() != 0 && self.aircraft.is_none() && !is_close_enough {
            self.helper.notify_move_queued();
            if self.repairable_near.is_none() {
                self.base.queue_child(self.mv.move_onto_target(self_, &self.host, WVec::ZERO, None));
            } else {
                self.base
                    .queue_child(self.mv.move_within_range(&self.host, WDist::ZERO, self.close_enough, None));
            }
            return false;
        }
        if !self.actual_resupply_started.get() && self.active_resupply_types.get() > 0 {
            self.actual_resupply_started.set(true);
            let host = self.host_actor();
            for nd in &self.notify_dock_clients {
                nd.docked(self_, host);
            }
            for nd in &self.notify_dock_hosts {
                nd.docked(host, self_);
            }
        }
        if self.active_resupply_types.get() & RESUPPLY_REPAIR != 0 {
            self.repair_tick(self_);
        }
        if self.active_resupply_types.get() & RESUPPLY_REARM != 0 && self.rearmable.as_ref().unwrap().rearm_tick(self_) {
            self.active_resupply_types.set(self.active_resupply_types.get() & !RESUPPLY_REARM);
        }
        if self.active_resupply_types.get() == 0 {
            self.on_resupply_ending(self_, false);
            return true;
        }
        false
    }

    fn cancel(&self, self_: &ActorRef, keep_queue: bool) {
        for_each_activity(self.base.child_activity(), true, &mut |a| {
            if let Some(m) = a.as_any().downcast_ref::<Move>() {
                m.cancel_force(self_, false, true);
            }
        });
        self.base.cancel(self_, keep_queue);
    }

    fn get_targets(&self, _self_: &ActorRef) -> Vec<Target> {
        vec![self.host.clone()]
    }
}
