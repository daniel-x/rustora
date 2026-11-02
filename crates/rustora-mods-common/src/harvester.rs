// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Harvesting and docking: `Harvester`, `DockClientManager`, `DockHost`, `Refinery`, `StoresResources`,
//! `StoresPlayerResources`, `ResourceClaimLayer`, `CashTrickler` and their activities.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use indexmap::IndexMap;
use rustora_core::expression::BooleanExpression;
use rustora_core::world_types::{CPos, CVec, WAngle, WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::actor::{ActorRef, ConditionCache, INVALID_CONDITION_TOKEN};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::AttackInfo;
use rustora_engine::net_collections::NetMap;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::{activity_common, conditional_observers_impl, info_impl, load_info, register_traits};

use crate::activities::{Drag, MoveCooldownHelper, Turn, Wait};
use crate::interfaces::*;
use crate::locomotor::BlockedByActor;
use crate::mobile::Mobile;
use crate::pathfinder::PATH_COST_FOR_INVALID_PATH;
use crate::player::PlayerResources;
use crate::render::BodyOrientation;
use crate::resources::ResourceLayer;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("Harvester", |n| load_info!(HarvesterInfo, n));
    r.register("DockClientManager", |n| load_info!(DockClientManagerInfo, n));
    r.register("DockHost", |n| load_info!(DockHostInfo, n));
    r.register("Refinery", |n| load_info!(RefineryInfo, n));
    r.register("StoresResources", |n| load_info!(StoresResourcesInfo, n));
    r.register("StoresPlayerResources", |n| load_info!(StoresPlayerResourcesInfo, n));
    r.register("ResourceClaimLayer", |n| load_info!(ResourceClaimLayerInfo, n));
    r.register("CashTrickler", |n| load_info!(CashTricklerInfo, n));
}

/// Tag of `BitSet<DockType>`.
pub struct DockType;

pub trait IDockClient {
    fn get_dock_type(&self) -> BitSet<DockType>;
    fn on_dock_started(&self, self_: &ActorRef, host_actor: &ActorRef, host: &Rc<DockHost>);
    fn on_dock_tick(&self, self_: &ActorRef, host_actor: &ActorRef, host: &Rc<DockHost>) -> bool;
    fn on_dock_completed(&self, self_: &ActorRef, host_actor: &ActorRef, host: &Rc<DockHost>);
    fn can_dock(&self, ty: BitSet<DockType>, force_enter: bool) -> bool;
    fn can_dock_at(&self, host_actor: &ActorRef, host: &Rc<DockHost>, force_enter: bool, ignore_occupancy: bool) -> bool;
    fn can_queue_dock_at(&self, host_actor: &ActorRef, host: &Rc<DockHost>, force_enter: bool, is_queued: bool) -> bool;
}

pub trait IAcceptResources {
    fn accept_resources(&self, self_: &ActorRef, resource_type: &str, count: i32) -> i32;
}

pub trait INotifyResourceAccepted {
    fn on_resource_accepted(&self, self_: &ActorRef, refinery: &ActorRef, resource_type: &str, count: i32, value: i32);
}

pub trait INotifyDockClientMoving {
    fn moving_to_dock(&self, self_: &ActorRef, host_actor: &ActorRef, host: &Rc<DockHost>);
    fn movement_cancelled(&self, self_: &ActorRef);
}

pub trait IResourceValueModifier {
    fn get_resource_value_modifier(&self) -> i32;
}

pub trait ICashTricklerModifier {
    fn get_cash_trickler_modifier(&self) -> i32;
}

#[derive(Clone, Debug, FieldLoad)]
pub struct StoresResourcesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub capacity: i32,
    pub resources: Vec<String>,
}

impl Default for StoresResourcesInfo {
    fn default() -> Self {
        StoresResourcesInfo {
            instance_name: None,
            capacity: 28,
            resources: Vec::new(),
        }
    }
}

pub struct StoresResources {
    info: Rc<StoresResourcesInfo>,
    contents: RefCell<NetMap<String, i32>>,
    contents_sum: Cell<i32>,
}

register_traits!(StoresResources: ISync);
info_impl!(StoresResourcesInfo => |info, _| {
    let mut contents = NetMap::new();
    for r in &info.resources {
        contents.insert(r.clone(), 0);
    }
    Some(Rc::new(StoresResources { info, contents: RefCell::new(contents), contents_sum: Cell::new(0) }))
});

impl ISync for StoresResources {
    fn sync_hash(&self) -> i32 {
        let mut v = 0i32;
        for (k, c) in self.contents.borrow().iter() {
            v = v.wrapping_add(c.wrapping_shl(k.encode_utf16().count() as u32));
        }
        v
    }
}

impl StoresResources {
    pub fn has_type(&self, ty: &str) -> bool {
        self.info.resources.iter().any(|r| r == ty)
    }

    pub fn capacity(&self) -> i32 {
        self.info.capacity
    }

    pub fn contents_sum(&self) -> i32 {
        self.contents_sum.get()
    }

    pub fn contents(&self) -> Vec<(String, i32)> {
        self.contents.borrow().iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    pub fn add_resource(&self, ty: &str, value: i32) -> i32 {
        if !self.has_type(ty) {
            return value;
        }
        let key = ty.to_string();
        let mut c = self.contents.borrow_mut();
        if self.contents_sum.get() + value > self.info.capacity {
            let added = self.info.capacity - self.contents_sum.get();
            *c.get_mut(&key).expect("type") += added;
            self.contents_sum.set(self.info.capacity);
            return value - added;
        }
        *c.get_mut(&key).expect("type") += value;
        self.contents_sum.set(self.contents_sum.get() + value);
        0
    }

    pub fn remove_resource(&self, ty: &str, value: i32) -> i32 {
        if !self.has_type(ty) {
            return value;
        }
        let key = ty.to_string();
        let mut c = self.contents.borrow_mut();
        let cur = *c.get(&key).expect("type");
        if cur < value {
            let leftover = value - cur;
            self.contents_sum.set(self.contents_sum.get() - cur);
            *c.get_mut(&key).expect("type") = 0;
            return leftover;
        }
        *c.get_mut(&key).expect("type") -= value;
        self.contents_sum.set(self.contents_sum.get() - value);
        0
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct StoresPlayerResourcesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub capacity: i32,
}

pub struct StoresPlayerResources {
    info: Rc<StoresPlayerResourcesInfo>,
    player: RefCell<Rc<PlayerResources>>,
}

register_traits!(StoresPlayerResources: INotifyOwnerChanged, INotifyCapture, INotifyKilled, INotifyAddedToWorld, INotifyRemovedFromWorld);
info_impl!(StoresPlayerResourcesInfo => |info, init| Some(Rc::new(StoresPlayerResources {
    info,
    player: RefCell::new(init.self_.owner().player_actor().trait_::<PlayerResources>()),
})));

impl StoresPlayerResources {
    fn stored(&self) -> i32 {
        let p = self.player.borrow();
        let cap = p.resource_capacity.get();
        if cap == 0 { 0 } else { (self.info.capacity as i64 * p.resources.get() as i64 / cap as i64) as i32 }
    }
}

impl INotifyOwnerChanged for StoresPlayerResources {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        *self.player.borrow_mut() = new_owner.player_actor().trait_::<PlayerResources>();
    }
}

impl INotifyCapture for StoresPlayerResources {
    fn on_capture(&self, _self_: &ActorRef, _captor: &ActorRef, old_owner: &PlayerRef, new_owner: &PlayerRef, _types: BitSet<crate::capture::CaptureType>) {
        let resources = self.stored();
        old_owner.player_actor().trait_::<PlayerResources>().take_resources(resources);
        new_owner.player_actor().trait_::<PlayerResources>().give_resources(resources);
    }
}

impl INotifyKilled for StoresPlayerResources {
    fn killed(&self, _self_: &ActorRef, _e: &AttackInfo) {
        let s = self.stored();
        self.player.borrow().take_resources(s);
    }
}

impl INotifyAddedToWorld for StoresPlayerResources {
    fn added_to_world(&self, _self_: &ActorRef) {
        self.player.borrow().add_storage_capacity(self.info.capacity);
    }
}

impl INotifyRemovedFromWorld for StoresPlayerResources {
    fn removed_from_world(&self, _self_: &ActorRef) {
        self.player.borrow().remove_storage_capacity(self.info.capacity);
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ResourceClaimLayerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(Default)]
pub struct ResourceClaimLayer {
    claim_by_cell: RefCell<HashMap<CPos, Vec<ActorRef>>>,
    claim_by_actor: RefCell<HashMap<u32, CPos>>,
}

register_traits!(ResourceClaimLayer);
info_impl!(ResourceClaimLayerInfo => |_, _| Some(Rc::new(ResourceClaimLayer::default())));

impl ResourceClaimLayer {
    pub fn try_claim_cell(&self, claimer: &ActorRef, cell: CPos) -> bool {
        let mut by_cell = self.claim_by_cell.borrow_mut();
        if let Some(claimers) = by_cell.get_mut(&cell) {
            claimers.retain(|a| !a.is_dead());
            if claimers.iter().any(|c| c.id != claimer.id && claimer.owner().is_allied_with(Some(&c.owner()))) {
                return false;
            }
        }
        let last = self.claim_by_actor.borrow().get(&claimer.id).copied();
        if let Some(last) = last {
            if let Some(v) = by_cell.get_mut(&last) {
                if let Some(i) = v.iter().position(|a| a.id == claimer.id) {
                    v.remove(i);
                }
            }
        }
        // C# never adds the claimer to the list, so a claim does not block other harvesters.
        by_cell.entry(cell).or_default();
        self.claim_by_actor.borrow_mut().insert(claimer.id, cell);
        true
    }

    pub fn can_claim_cell(&self, claimer: &ActorRef, cell: CPos) -> bool {
        match self.claim_by_cell.borrow().get(&cell) {
            None => true,
            Some(claimers) => !claimers.iter().any(|c| c.id != claimer.id && !c.is_dead() && claimer.owner().is_allied_with(Some(&c.owner()))),
        }
    }

    pub fn remove_claim(&self, claimer: &ActorRef) {
        let last = self.claim_by_actor.borrow_mut().remove(&claimer.id);
        if let Some(last) = last {
            if let Some(v) = self.claim_by_cell.borrow_mut().get_mut(&last) {
                if let Some(i) = v.iter().position(|a| a.id == claimer.id) {
                    v.remove(i);
                }
            }
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RefineryInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub use_storage: bool,
    pub discard_excess_resources: bool,
    pub show_ticks: bool,
    pub tick_rate: i32,
}

impl Default for RefineryInfo {
    fn default() -> Self {
        RefineryInfo {
            instance_name: None,
            use_storage: true,
            discard_excess_resources: false,
            show_ticks: true,
            tick_rate: 10,
        }
    }
}

pub struct Refinery {
    info: Rc<RefineryInfo>,
    player_resources: RefCell<Rc<PlayerResources>>,
    modifiers: OnceCell<Vec<Rc<dyn IResourceValueModifier>>>,
}

register_traits!(Refinery: IAcceptResources, INotifyCreated, INotifyOwnerChanged);
info_impl!(RefineryInfo => |info, init| Some(Rc::new(Refinery {
    info,
    player_resources: RefCell::new(init.self_.owner().player_actor().trait_::<PlayerResources>()),
    modifiers: OnceCell::new(),
})));

impl INotifyCreated for Refinery {
    fn created(&self, self_: &ActorRef) {
        let _ = self.modifiers.set(self_.traits_implementing::<dyn IResourceValueModifier>());
    }
}

impl INotifyOwnerChanged for Refinery {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        *self.player_resources.borrow_mut() = new_owner.player_actor().trait_::<PlayerResources>();
    }
}

impl IAcceptResources for Refinery {
    fn accept_resources(&self, self_: &ActorRef, resource_type: &str, mut count: i32) -> i32 {
        let pr = self.player_resources.borrow().clone();
        let Some(resource_value) = pr.info.resource_values.get(resource_type).copied() else { return 0 };
        let mods = self.modifiers.get().expect("created");
        let apply = |c: i32| util::apply_percentage_modifiers(c * resource_value, mods.iter().map(|m| m.get_resource_value_modifier()));
        let mut value = apply(count);
        if self.info.use_storage {
            let storage_limit = (pr.resource_capacity.get() - pr.resources.get()).max(0);
            if !self.info.discard_excess_resources {
                while value > storage_limit {
                    count -= 1;
                    value = apply(count);
                }
            } else {
                value = value.min(pr.resource_capacity.get() - pr.resources.get());
            }
            pr.give_resources(value);
        } else {
            value = pr.change_cash(value);
        }
        let owner = self_.owner();
        for (a, t) in self_.world().actors_with_trait::<dyn INotifyResourceAccepted>() {
            if Rc::ptr_eq(&a.owner(), &owner) {
                t.on_resource_accepted(&a, self_, resource_type, count, value);
            }
        }
        count
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CashTricklerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub interval: i32,
    pub initial_delay: i32,
    pub amount: i32,
    pub use_resource_storage: bool,
}

impl Default for CashTricklerInfo {
    fn default() -> Self {
        CashTricklerInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            interval: 50,
            initial_delay: 0,
            amount: 15,
            use_resource_storage: false,
        }
    }
}

pub struct CashTrickler {
    info: Rc<CashTricklerInfo>,
    state: ConditionalState,
    resources: RefCell<Option<Rc<PlayerResources>>>,
    ticks: Cell<i32>,
}

register_traits!(CashTrickler: ITick, ISync, INotifyCreated, INotifyOwnerChanged, IObservesVariables, IDisabledTrait);
info_impl!(CashTricklerInfo => |info, _| {
    let state = ConditionalState::new_pausable(&info.base);
    Some(Rc::new(CashTrickler { ticks: Cell::new(info.initial_delay), info, state, resources: RefCell::new(None) }))
});

impl ConditionalHooks for CashTrickler {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_observers_impl!(CashTrickler);

impl ISync for CashTrickler {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.ticks.get()
    }
}

impl INotifyCreated for CashTrickler {
    fn created(&self, self_: &ActorRef) {
        *self.resources.borrow_mut() = Some(self_.owner().player_actor().trait_::<PlayerResources>());
        conditional_created(self, self_);
    }
}

impl INotifyOwnerChanged for CashTrickler {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        *self.resources.borrow_mut() = Some(new_owner.player_actor().trait_::<PlayerResources>());
    }
}

impl ITick for CashTrickler {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            self.ticks.set(self.info.interval);
        }
        if self.state.is_trait_paused() || self.state.is_trait_disabled() {
            return;
        }
        self.ticks.set(self.ticks.get() - 1);
        if self.ticks.get() < 0 {
            let mods = self_.traits_implementing::<dyn ICashTricklerModifier>();
            self.ticks.set(self.info.interval);
            let amount = util::apply_percentage_modifiers(self.info.amount, mods.iter().map(|m| m.get_cash_trickler_modifier()));
            let r = self.resources.borrow().clone().expect("created");
            if self.info.use_resource_storage {
                r.give_resources(amount);
            } else {
                r.change_cash(amount);
            }
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct DockClientManagerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub search_for_dock_delay: i32,
    pub occupancy_cost_modifier: i32,
    pub require_force_move_condition: Option<BooleanExpression>,
}

impl Default for DockClientManagerInfo {
    fn default() -> Self {
        DockClientManagerInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            search_for_dock_delay: 125,
            occupancy_cost_modifier: 12,
            require_force_move_condition: None,
        }
    }
}

pub struct DockClientManager {
    pub info: Rc<DockClientManagerInfo>,
    state: ConditionalState,
    me: Weak<DockClientManager>,
    self_: ActorRef,
    dock_clients: RefCell<Vec<Rc<dyn IDockClient>>>,
    require_force_move: Cell<bool>,
    reserved_host_actor: RefCell<Option<ActorRef>>,
    reserved_host: RefCell<Option<Rc<DockHost>>>,
    last_reserved_dock_host: RefCell<Option<Rc<DockHost>>>,
}

register_traits!(DockClientManager: IResolveOrder, INotifyKilled, INotifyActorDisposing, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(DockClientManagerInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new_cyclic(|me| DockClientManager {
        info,
        state,
        me: me.clone(),
        self_: init.self_.clone(),
        dock_clients: RefCell::new(Vec::new()),
        require_force_move: Cell::new(false),
        reserved_host_actor: RefCell::new(None),
        reserved_host: RefCell::new(None),
        last_reserved_dock_host: RefCell::new(None),
    }))
});

impl ConditionalHooks for DockClientManager {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl ISync for DockClientManager {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash()
    }
}

impl IDisabledTrait for DockClientManager {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl IObservesVariables for DockClientManager {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        let mut out = conditional_observers(&self);
        if let Some(e) = &self.info.require_force_move_condition {
            let (w, e2) = (Rc::downgrade(&self), e.clone());
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

impl INotifyCreated for DockClientManager {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
        *self.dock_clients.borrow_mut() = self_.traits_implementing::<dyn IDockClient>();
    }
}

impl INotifyKilled for DockClientManager {
    fn killed(&self, _self_: &ActorRef, _e: &AttackInfo) {
        self.unreserve_host();
    }
}

impl INotifyActorDisposing for DockClientManager {
    fn disposing(&self, _self_: &ActorRef) {
        self.unreserve_host();
    }
}

impl IResolveOrder for DockClientManager {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "Dock" || order.order_string == "ForceDock" {
            if order.target.target_type() != TargetType::Actor {
                return;
            }
            self_.queue_activity_queued(
                order.queued,
                MoveToDock::new(self_, order.target.actor().cloned(), None, order.order_string == "ForceDock", true),
            );
        }
    }
}

impl DockClientManager {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn reserved_host(&self) -> Option<Rc<DockHost>> {
        self.reserved_host.borrow().clone()
    }

    pub fn reserved_host_actor(&self) -> Option<ActorRef> {
        self.reserved_host_actor.borrow().clone()
    }

    pub fn last_reserved_host(&self) -> Option<Rc<DockHost>> {
        let last = self.last_reserved_dock_host.borrow().clone();
        if let Some(l) = last {
            if !l.is_enabled_and_in_world() {
                *self.last_reserved_dock_host.borrow_mut() = None;
            } else {
                return Some(l);
            }
        }
        self.reserved_host.borrow().clone()
    }

    pub fn unreserve_host(&self) {
        let host = self.reserved_host.borrow_mut().take();
        if let Some(h) = host {
            *self.last_reserved_dock_host.borrow_mut() = Some(h.clone());
            *self.reserved_host_actor.borrow_mut() = None;
            h.unreserve(self);
        }
    }

    pub fn reserve_host(&self, host_actor: &ActorRef, host: &Rc<DockHost>) -> bool {
        if self.reserved_host.borrow().as_ref().is_some_and(|r| Rc::ptr_eq(r, host)) {
            return true;
        }
        self.unreserve_host();
        if host.reserve(host_actor, &self.me.upgrade().expect("trait alive")) {
            *self.reserved_host.borrow_mut() = Some(host.clone());
            *self.reserved_host_actor.borrow_mut() = Some(host_actor.clone());
            *self.last_reserved_dock_host.borrow_mut() = None;
            return true;
        }
        false
    }

    fn clients(&self) -> Vec<Rc<dyn IDockClient>> {
        self.dock_clients.borrow().clone()
    }

    pub fn on_dock_started(&self, self_: &ActorRef, host_actor: &ActorRef, host: &Rc<DockHost>) {
        for c in self.clients() {
            c.on_dock_started(self_, host_actor, host);
        }
    }

    pub fn on_dock_tick(&self, self_: &ActorRef, host_actor: &ActorRef, host: &Rc<DockHost>) -> bool {
        if self.is_trait_disabled() {
            return true;
        }
        let mut cancel = true;
        for c in self.clients() {
            if !c.on_dock_tick(self_, host_actor, host) {
                cancel = false;
            }
        }
        cancel
    }

    pub fn on_dock_completed(&self, self_: &ActorRef, host_actor: &ActorRef, host: &Rc<DockHost>) {
        for c in self.clients() {
            c.on_dock_completed(self_, host_actor, host);
        }
        self.unreserve_host();
    }

    pub fn can_dock_type(&self, ty: BitSet<DockType>, force_enter: bool) -> bool {
        !self.is_trait_disabled() && self.clients().iter().any(|c| c.can_dock(ty, force_enter))
    }

    pub fn can_dock_at(&self, host_actor: &ActorRef, host: &Rc<DockHost>, force_enter: bool, ignore_occupancy: bool) -> bool {
        !self.is_trait_disabled() && self.clients().iter().any(|c| c.can_dock_at(host_actor, host, force_enter, ignore_occupancy))
    }

    pub fn can_dock_at_actor(&self, target: &ActorRef, force_enter: bool, ignore_occupancy: bool) -> bool {
        !self.is_trait_disabled()
            && target
                .traits_implementing::<DockHost>()
                .iter()
                .any(|h| self.clients().iter().any(|c| c.can_dock_at(target, h, force_enter, ignore_occupancy)))
    }

    pub fn can_queue_dock_at(&self, target: &ActorRef, force_enter: bool, is_queued: bool) -> bool {
        !self.is_trait_disabled()
            && target
                .traits_implementing::<DockHost>()
                .iter()
                .any(|h| self.clients().iter().any(|c| c.can_queue_dock_at(target, h, force_enter, is_queued)))
    }

    fn available_clients(&self, ty: BitSet<DockType>, force_enter: bool) -> Vec<Rc<dyn IDockClient>> {
        if ty.is_empty() {
            return self.clients();
        }
        self.clients().into_iter().filter(|c| c.can_dock(ty, force_enter)).collect()
    }

    pub fn closest_dock(&self, ignore: Option<&Rc<DockHost>>, ty: BitSet<DockType>, force_enter: bool, ignore_occupancy: bool) -> Option<(ActorRef, Rc<DockHost>)> {
        let clients = self.available_clients(ty, force_enter);
        let docks: Vec<(ActorRef, Rc<DockHost>)> = self
            .self_
            .world()
            .actors_with_trait::<DockHost>()
            .into_iter()
            .filter(|(a, h)| !ignore.is_some_and(|i| Rc::ptr_eq(i, h)) && clients.iter().any(|c| c.can_dock_at(a, h, force_enter, ignore_occupancy)))
            .collect();
        closest_dock(&docks, &self.self_, self)
    }

    pub fn available_dock_hosts(&self, target: &ActorRef, ty: BitSet<DockType>, force_enter: bool, ignore_occupancy: bool) -> Vec<(ActorRef, Rc<DockHost>)> {
        let clients = self.available_clients(ty, force_enter);
        target
            .traits_implementing::<DockHost>()
            .into_iter()
            .filter(|h| clients.iter().any(|c| c.can_dock_at(target, h, force_enter, ignore_occupancy)))
            .map(|h| (target.clone(), h))
            .collect()
    }
}

/// `DockExts.ClosestDock`.
pub fn closest_dock(docks: &[(ActorRef, Rc<DockHost>)], client_actor: &ActorRef, client: &DockClientManager) -> Option<(ActorRef, Rc<DockHost>)> {
    let w = client_actor.world();
    if let Some(mobile) = client_actor.trait_or_default::<Mobile>() {
        let mut lookup: IndexMap<CPos, (ActorRef, Rc<DockHost>)> = IndexMap::new();
        for d in docks {
            lookup.entry(w.map().cell_containing(d.1.dock_position())).or_insert_with(|| d.clone());
        }
        let keys: Vec<CPos> = lookup.keys().copied().collect();
        let lookup = Rc::new(lookup);
        let (l2, modifier) = (lookup.clone(), client.info.occupancy_cost_modifier);
        let path = mobile.path_finder().find_path_to_target_cell(
            client_actor,
            &keys,
            client_actor.location(),
            BlockedByActor::None,
            Some(Rc::new(move |location| match l2.get(&location) {
                Some(d) => d.1.reservation_count() * modifier,
                None => 0,
            })),
            None,
            true,
        );
        if let Some(last) = path.last() {
            return lookup.get(last).cloned();
        }
        None
    } else {
        let key = |d: &(ActorRef, Rc<DockHost>)| {
            (client_actor.location() - w.map().cell_containing(d.1.dock_position())).length_squared() + d.1.reservation_count() * client.info.occupancy_cost_modifier
        };
        let mut best: Option<&(ActorRef, Rc<DockHost>)> = None;
        for d in docks {
            if best.is_none_or(|b| key(d) < key(b)) {
                best = Some(d);
            }
        }
        best.cloned()
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct DockHostInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(name = "Type")]
    pub ty: BitSet<DockType>,
    pub max_queue_length: i32,
    pub dock_wait: i32,
    pub dock_angle: WAngle,
    pub dock_offset: WVec,
    pub is_drag_required: bool,
    pub drag_offset: WVec,
    pub drag_length: i32,
}

impl Default for DockHostInfo {
    fn default() -> Self {
        DockHostInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            ty: BitSet::default(),
            max_queue_length: 3,
            dock_wait: 10,
            dock_angle: WAngle::ZERO,
            dock_offset: WVec::ZERO,
            is_drag_required: false,
            drag_offset: WVec::ZERO,
            drag_length: 0,
        }
    }
}

pub struct DockHost {
    pub info: Rc<DockHostInfo>,
    state: ConditionalState,
    me: Weak<DockHost>,
    self_: ActorRef,
    reserved_dock_clients: RefCell<Vec<Rc<DockClientManager>>>,
    prevent_dock: Cell<bool>,
    docked_client_actor: RefCell<Option<ActorRef>>,
    docked_client: RefCell<Option<Rc<DockClientManager>>>,
}

register_traits!(DockHost: ITick, INotifySold, INotifyCapture, INotifyOwnerChanged, ISync, INotifyKilled, INotifyActorDisposing, INotifyCreated, IObservesVariables, IDisabledTrait);
info_impl!(DockHostInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new_cyclic(|me| DockHost {
        info,
        state,
        me: me.clone(),
        self_: init.self_.clone(),
        reserved_dock_clients: RefCell::new(Vec::new()),
        prevent_dock: Cell::new(false),
        docked_client_actor: RefCell::new(None),
        docked_client: RefCell::new(None),
    }))
});

impl ConditionalHooks for DockHost {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_disabled(&self, _self_: &ActorRef) {
        self.unreserve_all();
    }
}

conditional_observers_impl!(DockHost);

impl ISync for DockHost {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ sync_bool(self.prevent_dock.get()) ^ rustora_engine::actor::hash_actor(self.docked_client_actor.borrow().as_ref())
    }
}

impl INotifyCreated for DockHost {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl ITick for DockHost {
    fn tick(&self, self_: &ActorRef) {
        let a = self.docked_client_actor.borrow().clone();
        if let Some(a) = a {
            if a.is_dead() || !a.is_in_world() {
                let c = self.docked_client.borrow().clone().expect("docked client");
                self.on_dock_completed(self_, &a, &c);
            }
        }
    }
}

impl DockHost {
    pub fn is_enabled_and_in_world(&self) -> bool {
        !self.prevent_dock.get() && !self.state.is_trait_disabled() && !self.self_.is_dead() && self.self_.is_in_world()
    }

    pub fn reservation_count(&self) -> i32 {
        self.reserved_dock_clients.borrow().len() as i32
    }

    pub fn can_be_reserved(&self) -> bool {
        self.reservation_count() < self.info.max_queue_length
    }

    pub fn dock_position(&self) -> WPos {
        self.self_.center_position() + self.info.dock_offset
    }

    pub fn is_docking_possible(&self, client: Option<&Rc<DockClientManager>>, ignore_reservations: bool) -> bool {
        !self.state.is_trait_disabled()
            && (ignore_reservations
                || self.can_be_reserved()
                || client.is_some_and(|c| self.reserved_dock_clients.borrow().iter().any(|r| Rc::ptr_eq(r, c))))
    }

    pub fn reserve(&self, self_: &ActorRef, client: &Rc<DockClientManager>) -> bool {
        if self.can_be_reserved() && !self.reserved_dock_clients.borrow().iter().any(|r| Rc::ptr_eq(r, client)) {
            self.reserved_dock_clients.borrow_mut().push(client.clone());
            client.reserve_host(self_, &self.me.upgrade().expect("trait alive"));
            return true;
        }
        false
    }

    pub fn unreserve_all(&self) {
        loop {
            let first = self.reserved_dock_clients.borrow().first().cloned();
            match first {
                Some(c) => self.unreserve(&c),
                None => break,
            }
        }
    }

    pub fn unreserve(&self, client: &DockClientManager) {
        let removed = {
            let mut v = self.reserved_dock_clients.borrow_mut();
            match v.iter().position(|r| std::ptr::eq(Rc::as_ptr(r), client)) {
                Some(i) => {
                    v.remove(i);
                    true
                }
                None => false,
            }
        };
        if removed {
            client.unreserve_host();
        }
    }

    pub fn on_dock_started(&self, _self_: &ActorRef, client_actor: &ActorRef, client: &Rc<DockClientManager>) {
        *self.docked_client_actor.borrow_mut() = Some(client_actor.clone());
        *self.docked_client.borrow_mut() = Some(client.clone());
    }

    pub fn on_dock_completed(&self, _self_: &ActorRef, _client_actor: &ActorRef, _client: &Rc<DockClientManager>) {
        *self.docked_client_actor.borrow_mut() = None;
        *self.docked_client.borrow_mut() = None;
    }

    fn queue_move_activity(&self, base: &ActivityBase, self_: &ActorRef, client_actor: &ActorRef, helper: &MoveCooldownHelper) -> bool {
        let mv = client_actor.trait_::<dyn IMove>();
        let w = client_actor.world();
        let at_dock = match mv.as_mobile() {
            Some(_) => client_actor.location() == w.map().cell_containing(self.dock_position()),
            None => client_actor.center_position() == self.dock_position(),
        };
        let facing = client_actor.trait_or_default::<dyn IFacing>();
        if !at_dock || facing.is_none_or(|f| f.facing() != self.info.dock_angle) {
            helper.notify_move_queued();
            base.queue_child(mv.move_onto_target(
                client_actor,
                &Target::from_actor(Some(self_)),
                self.dock_position() - self_.center_position(),
                Some(self.info.dock_angle),
            ));
            return true;
        }
        false
    }

    fn queue_dock_activity(&self, base: &ActivityBase, self_: &ActorRef, client_actor: &ActorRef, client: &Rc<DockClientManager>) {
        base.queue_child(GenericDockSequence::new(client_actor, client, self_, &self.me.upgrade().expect("trait alive"), &self.info));
    }
}

impl INotifyOwnerChanged for DockHost {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, _new: &PlayerRef) {
        self.unreserve_all();
    }
}

impl INotifyCapture for DockHost {
    fn on_capture(&self, self_: &ActorRef, _captor: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef, _types: BitSet<crate::capture::CaptureType>) {
        let a = self.docked_client_actor.borrow().clone();
        if let Some(a) = a.filter(|a| !a.is_dead() && a.is_in_world()) {
            a.change_owner(new_owner.clone());
            let c = self.docked_client.borrow().clone().expect("docked client");
            c.reserve_host(self_, &self.me.upgrade().expect("trait alive"));
        }
    }
}

impl INotifySold for DockHost {
    fn selling(&self, _self_: &ActorRef) {
        self.prevent_dock.set(true);
    }
    fn sold(&self, _self_: &ActorRef) {
        self.unreserve_all();
    }
}

impl INotifyKilled for DockHost {
    fn killed(&self, _self_: &ActorRef, _e: &AttackInfo) {
        self.unreserve_all();
    }
}

impl INotifyActorDisposing for DockHost {
    fn disposing(&self, _self_: &ActorRef) {
        self.prevent_dock.set(true);
        self.unreserve_all();
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct HarvesterInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(name = "Type")]
    pub ty: BitSet<DockType>,
    pub unblock_cell: CVec,
    pub bale_load_delay: i32,
    pub bale_unload_delay: i32,
    pub bale_unload_amount: i32,
    pub harvest_facings: i32,
    pub resources: Vec<String>,
    pub fully_loaded_speed: i32,
    pub search_on_creation: bool,
    pub search_from_proc_radius: i32,
    pub search_from_harvester_radius: i32,
    pub wait_duration: i32,
    pub resource_refinery_direction_penalty: i32,
    pub queue_full_load: bool,
    pub empty_condition: Option<String>,
}

impl Default for HarvesterInfo {
    fn default() -> Self {
        HarvesterInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            ty: BitSet::new(&["Unload"]),
            unblock_cell: CVec::new(0, 4),
            bale_load_delay: 4,
            bale_unload_delay: 4,
            bale_unload_amount: 1,
            harvest_facings: 0,
            resources: Vec::new(),
            fully_loaded_speed: 85,
            search_on_creation: true,
            search_from_proc_radius: 24,
            search_from_harvester_radius: 12,
            wait_duration: 25,
            resource_refinery_direction_penalty: 200,
            queue_full_load: false,
            empty_condition: None,
        }
    }
}

pub struct Harvester {
    pub info: Rc<HarvesterInfo>,
    state: ConditionalState,
    self_: ActorRef,
    mobile: OnceCell<Option<Rc<Mobile>>>,
    resource_layer: Rc<ResourceLayer>,
    claim_layer: Rc<ResourceClaimLayer>,
    stores_resources: Vec<Rc<StoresResources>>,
    dock_client_manager: Option<Rc<DockClientManager>>,
    token: Cell<i32>,
    current_unload_ticks: Cell<i32>,
    accept_resources: RefCell<Option<Rc<dyn IAcceptResources>>>,
}

register_traits!(Harvester: IResolveOrder, ISpeedModifier, IDockClient, ISync, INotifyCreated, IObservesVariables, IDisabledTrait);
info_impl!(HarvesterInfo => |info, init| {
    let self_ = init.self_.clone();
    let w = init.world();
    let state = ConditionalState::new(&info.base);
    let stores_resources = self_.traits_implementing::<StoresResources>().into_iter().filter(|sr| info.resources.iter().any(|r| sr.has_type(r))).collect();
    Some(Rc::new(Harvester {
        state,
        mobile: OnceCell::new(),
        resource_layer: w.world_actor().trait_::<ResourceLayer>(),
        claim_layer: w.world_actor().trait_::<ResourceClaimLayer>(),
        stores_resources,
        dock_client_manager: self_.trait_or_default::<DockClientManager>(),
        token: Cell::new(INVALID_CONDITION_TOKEN),
        current_unload_ticks: Cell::new(0),
        accept_resources: RefCell::new(None),
        self_,
        info,
    }))
});

impl ConditionalHooks for Harvester {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        if self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
    }
}

conditional_observers_impl!(Harvester);

impl ISync for Harvester {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.current_unload_ticks.get()
    }
}

impl INotifyCreated for Harvester {
    fn created(&self, self_: &ActorRef) {
        let _ = self.mobile.set(self_.trait_or_default::<Mobile>());
        self.update_condition(self_);
        if self.info.search_on_creation && self.mobile().is_some() {
            self_.queue_activity(FindAndDeliverResources::new(self_, None));
        }
        conditional_created(self, self_);
    }
}

impl Harvester {
    fn mobile(&self) -> Option<&Rc<Mobile>> {
        self.mobile.get().and_then(|m| m.as_ref())
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_full(&self) -> bool {
        self.stores_resources.iter().all(|sr| sr.contents_sum() >= sr.capacity())
    }

    pub fn is_empty(&self) -> bool {
        self.stores_resources.iter().all(|sr| sr.contents_sum() == 0)
    }

    pub fn fullness(&self) -> i32 {
        let s: i32 = self.stores_resources.iter().map(|sr| sr.contents_sum() * 100 / sr.capacity()).sum();
        s / self.stores_resources.len() as i32
    }

    fn update_condition(&self, self_: &ActorRef) {
        let Some(c) = self.info.empty_condition.as_deref().filter(|c| !c.is_empty()) else { return };
        let enabled = self.is_empty();
        if enabled && self.token.get() == INVALID_CONDITION_TOKEN {
            self.token.set(self_.grant_condition(c));
        } else if !enabled && self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
    }

    pub fn add_resource(&self, self_: &ActorRef, ty: &str) {
        for sr in &self.stores_resources {
            if sr.add_resource(ty, 1) == 0 {
                break;
            }
        }
        self.update_condition(self_);
    }

    pub fn can_harvest_cell(&self, cell: CPos) -> bool {
        match self.resource_layer.get_resource(cell).ty {
            Some(t) => self.info.resources.iter().any(|r| **r == *t),
            None => false,
        }
    }

    fn base_can_dock(&self, ty: BitSet<DockType>) -> bool {
        !self.state.is_trait_disabled() && self.info.ty.overlaps(&ty)
    }
}

impl IDockClient for Harvester {
    fn get_dock_type(&self) -> BitSet<DockType> {
        self.info.ty
    }

    fn can_dock(&self, ty: BitSet<DockType>, force_enter: bool) -> bool {
        self.base_can_dock(ty) && (force_enter || !self.is_empty())
    }

    fn can_dock_at(&self, host_actor: &ActorRef, host: &Rc<DockHost>, force_enter: bool, ignore_occupancy: bool) -> bool {
        self.can_dock(host.info.ty, force_enter)
            && host.is_docking_possible(self.dock_client_manager.as_ref(), ignore_occupancy)
            && (Rc::ptr_eq(&self.self_.owner(), &host_actor.owner()) || (ignore_occupancy && self.self_.owner().is_allied_with(Some(&host_actor.owner()))))
    }

    fn can_queue_dock_at(&self, host_actor: &ActorRef, host: &Rc<DockHost>, _force_enter: bool, _is_queued: bool) -> bool {
        self.can_dock(host.info.ty, true) && host.is_docking_possible(self.dock_client_manager.as_ref(), true) && self.self_.owner().is_allied_with(Some(&host_actor.owner()))
    }

    fn on_dock_started(&self, _self_: &ActorRef, host_actor: &ActorRef, host: &Rc<DockHost>) {
        if self.base_can_dock(host.info.ty) {
            *self.accept_resources.borrow_mut() = host_actor.trait_or_default::<dyn IAcceptResources>();
        }
    }

    fn on_dock_tick(&self, self_: &ActorRef, host_actor: &ActorRef, _host: &Rc<DockHost>) -> bool {
        let accept = self.accept_resources.borrow().clone();
        let Some(accept) = accept.filter(|_| !self.is_trait_disabled()) else { return true };
        self.current_unload_ticks.set(self.current_unload_ticks.get() - 1);
        if self.current_unload_ticks.get() > 0 {
            return false;
        }
        for sr in &self.stores_resources {
            for (k, v) in sr.contents() {
                let count = v.min(self.info.bale_unload_amount);
                let accepted = accept.accept_resources(host_actor, &k, count);
                if accepted == 0 {
                    continue;
                }
                sr.remove_resource(&k, accepted);
                self.current_unload_ticks.set(self.info.bale_unload_delay);
                self.update_condition(self_);
                return false;
            }
        }
        self.is_empty()
    }

    fn on_dock_completed(&self, self_: &ActorRef, _host_actor: &ActorRef, dock: &Rc<DockHost>) {
        *self.accept_resources.borrow_mut() = None;
        if self.info.ty.overlaps(&dock.info.ty) {
            let cur = self_.current_activity();
            let need = match &cur {
                None => true,
                Some(c) => c.as_any().downcast_ref::<FindAndDeliverResources>().is_none() && c.base().next_activity().is_none(),
            };
            if need {
                self_.queue_activity_queued(true, FindAndDeliverResources::new(self_, None));
            }
        }
    }
}

impl IResolveOrder for Harvester {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        let Some(mobile) = self.mobile().filter(|_| order.order_string == "Harvest") else { return };
        let loc = if order.target.target_type() != TargetType::Invalid {
            let cell = self_.world().map().cell_containing(order.target.center_position());
            let (m, c, a) = (mobile.clone(), self.claim_layer.clone(), self_.clone());
            mobile.nearest_cell(cell, &move |p| m.can_enter_cell(p, None, BlockedByActor::All) && c.try_claim_cell(&a, p), 1, 6)
        } else {
            self_.location()
        };
        self_.queue_activity_queued(order.queued, FindAndDeliverResources::new(self_, Some(loc)));
    }
}

impl ISpeedModifier for Harvester {
    fn get_speed_modifier(&self) -> i32 {
        100 - (100 - self.info.fully_loaded_speed) * self.fullness() / 100
    }
}

/// `Activities/FindAndDeliverResources.cs`.
pub struct FindAndDeliverResources {
    base: ActivityBase,
    harv: Rc<Harvester>,
    mobile: Rc<Mobile>,
    claim_layer: Rc<ResourceClaimLayer>,
    helper: MoveCooldownHelper,
    order_location: Cell<Option<CPos>>,
    last_harvested_cell: Cell<Option<CPos>>,
    has_delivered_load: Cell<bool>,
    has_harvested_cell: Cell<bool>,
    has_waited: Cell<bool>,
    last_search_failed: Cell<bool>,
}

impl FindAndDeliverResources {
    pub fn new(self_: &ActorRef, order_location: Option<CPos>) -> ActivityRef {
        let mobile = self_.trait_::<Mobile>();
        let helper = MoveCooldownHelper::new(Some(mobile.clone()));
        helper.retry_if_destination_blocked.set(true);
        Rc::new(FindAndDeliverResources {
            base: ActivityBase::default(),
            harv: self_.trait_::<Harvester>(),
            mobile,
            claim_layer: self_.world().world_actor().trait_::<ResourceClaimLayer>(),
            helper,
            order_location: Cell::new(order_location),
            last_harvested_cell: Cell::new(None),
            has_delivered_load: Cell::new(false),
            has_harvested_cell: Cell::new(false),
            has_waited: Cell::new(false),
            last_search_failed: Cell::new(false),
        })
    }

    pub fn last_search_failed(&self) -> bool {
        self.last_search_failed.get()
    }

    fn manager(&self) -> Option<&Rc<DockClientManager>> {
        self.harv.dock_client_manager.as_ref()
    }

    fn closest_harvestable_pos(&self, self_: &ActorRef) -> Option<CPos> {
        match self.order_location.get() {
            None => {
                if self.harv.can_harvest_cell(self_.location()) && self.claim_layer.can_claim_cell(self_, self_.location()) {
                    return Some(self_.location());
                }
            }
            Some(o) => {
                if self.harv.can_harvest_cell(o) && self.claim_layer.can_claim_cell(self_, o) {
                    return Some(o);
                }
                self.order_location.set(None);
            }
        }
        let w = self_.world();
        let info = &self.harv.info;
        let dock_pos = self.manager().and_then(|m| m.last_reserved_host()).map(|h| h.dock_position());
        let (search_from, radius) = match self.last_harvested_cell.get() {
            Some(c) => (c, info.search_from_harvester_radius),
            None => (
                dock_pos.map_or(self_.location(), |d| w.map().cell_containing(d)),
                info.search_from_proc_radius,
            ),
        };
        let radius2 = radius * radius;
        let harv_pos = self_.center_position();
        let (h1, c1, a1) = (self.harv.clone(), self.claim_layer.clone(), self_.clone());
        let (h2, w2, penalty) = (self.harv.clone(), w.clone(), info.resource_refinery_direction_penalty);
        let path = self.mobile.path_finder().find_path_to_target_cell_by_predicate(
            self_,
            &[search_from, self_.location()],
            Box::new(move |loc| h1.can_harvest_cell(loc) && c1.can_claim_cell(&a1, loc)),
            BlockedByActor::Stationary,
            Some(Rc::new(move |loc: CPos| {
                if (loc - search_from).length_squared() > radius2 {
                    return PATH_COST_FOR_INVALID_PATH;
                }
                if let Some(d) = dock_pos {
                    if penalty > 0 && h2.can_harvest_cell(loc) {
                        let pos = w2.map().center_of_cell(loc);
                        let b = pos - d;
                        if b != WVec::ZERO {
                            let c = pos - harv_pos;
                            if c != WVec::ZERO {
                                let a = harv_pos - d;
                                let num = 512 * (b.length_squared() + c.length_squared() - a.length_squared());
                                let cos_a = (num / b.length() as i64 / c.length() as i64) as i32;
                                return (penalty / 2).abs() + penalty * cos_a / 2048;
                            }
                        }
                    }
                }
                0
            })),
            None,
            true,
        );
        path.first().copied()
    }
}

impl Activity for FindAndDeliverResources {
    activity_common!("FindAndDeliverResources");

    fn on_first_run(&self, self_: &ActorRef) {
        if let Some(o) = self.order_location.get() {
            self.last_harvested_cell.set(Some(o));
            if self.harv.is_full() {
                self.base.queue_child(MoveToDock::new(self_, None, None, false, false));
            }
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() || self.harv.is_trait_disabled() {
            return true;
        }
        if self.base.next_activity().is_some() {
            if !self.harv.info.queue_full_load && (self.has_harvested_cell.get() || self.last_search_failed.get()) {
                return true;
            }
            if self.has_delivered_load.get() || self.harv.is_full() {
                return true;
            }
        }
        if self.harv.is_full() || (!self.harv.is_empty() && self.last_search_failed.get()) {
            if self.manager().expect("dock manager").reserved_host().is_some() {
                return false;
            }
            self.base.queue_child(MoveToDock::new(self_, None, None, false, false));
            self.has_delivered_load.set(true);
        }
        if self.last_search_failed.get() && !self.has_waited.get() {
            self.base.queue_child(Wait::new(self.harv.info.wait_duration));
            self.has_waited.set(true);
            return false;
        }
        self.has_waited.set(false);
        let mut closest = self.closest_harvestable_pos(self_);
        if closest.is_none() {
            if self.last_harvested_cell.get().is_some() {
                self.last_harvested_cell.set(None);
                closest = self.closest_harvestable_pos(self_);
                self.last_search_failed.set(closest.is_none());
            } else {
                self.last_search_failed.set(true);
            }
        } else {
            self.last_search_failed.set(false);
        }
        if let Some(r) = self.helper.tick(&self_.world(), false) {
            return r;
        }
        if self.last_search_failed.get() {
            if let Some(lastproc) = self.manager().and_then(|m| m.last_reserved_host()) {
                let delivery_loc = self_.world().map().cell_containing(lastproc.dock_position());
                if self_.location() == delivery_loc && self.harv.is_empty() {
                    let unblock = delivery_loc + self.harv.info.unblock_cell;
                    let move_to = self.mobile.nearest_moveable_cell_range(unblock, 1, 5);
                    self.helper.notify_move_queued();
                    self.base.queue_child(self.mobile.move_to(move_to, 1, None, false));
                }
            }
            return false;
        }
        self.helper.notify_move_queued();
        let c = closest.expect("found cell");
        self.base.queue_child(HarvestResource::new(self_, c));
        self.last_harvested_cell.set(Some(c));
        self.has_harvested_cell.set(true);
        false
    }

    fn get_targets(&self, self_: &ActorRef) -> Vec<Target> {
        vec![Target::from_cell(&self_.world(), self_.location(), subcell::FULL_CELL)]
    }
}

/// `Activities/HarvestResource.cs`.
pub struct HarvestResource {
    base: ActivityBase,
    harv: Rc<Harvester>,
    facing: Rc<dyn IFacing>,
    claim_layer: Rc<ResourceClaimLayer>,
    resource_layer: Rc<ResourceLayer>,
    body: Rc<BodyOrientation>,
    mv: Rc<dyn IMove>,
    target_cell: CPos,
    notify: Vec<Rc<dyn INotifyHarvestAction>>,
    helper: MoveCooldownHelper,
}

impl HarvestResource {
    pub fn new(self_: &ActorRef, target_cell: CPos) -> ActivityRef {
        let w = self_.world();
        Rc::new(HarvestResource {
            base: ActivityBase::default(),
            harv: self_.trait_::<Harvester>(),
            facing: self_.trait_::<dyn IFacing>(),
            body: self_.trait_::<BodyOrientation>(),
            mv: self_.trait_::<dyn IMove>(),
            claim_layer: w.world_actor().trait_::<ResourceClaimLayer>(),
            resource_layer: w.world_actor().trait_::<ResourceLayer>(),
            target_cell,
            notify: self_.traits_implementing::<dyn INotifyHarvestAction>(),
            helper: MoveCooldownHelper::new(self_.trait_or_default::<Mobile>()),
        })
    }
}

impl Activity for HarvestResource {
    activity_common!("HarvestResource");

    fn on_first_run(&self, self_: &ActorRef) {
        self.claim_layer.try_claim_cell(self_, self.target_cell);
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.harv.is_trait_disabled() {
            self.cancel(self_, true);
        }
        if self.base.is_canceling() || self.harv.is_full() {
            return true;
        }
        if let Some(r) = self.helper.tick(&self_.world(), false) {
            return r;
        }
        if self_.location() != self.target_cell {
            for n in &self.notify {
                n.moving_to_resources(self_, self.target_cell);
            }
            self.helper.notify_move_queued();
            self.base.queue_child(self.mv.move_to(self.target_cell, 0, None, false));
            return false;
        }
        if !self.harv.can_harvest_cell(self_.location()) {
            return true;
        }
        if self.harv.info.harvest_facings != 0 {
            let current = self.facing.facing();
            let desired = self.body.quantize_facing_to(current, self.harv.info.harvest_facings);
            if desired != current {
                self.base.queue_child(Turn::new(self_, desired));
                return false;
            }
        }
        let resource = self.resource_layer.get_resource(self_.location());
        let Some(ty) = resource.ty else { return true };
        if self.resource_layer.remove_resource(&ty, self_.location(), 1) != 1 {
            return true;
        }
        self.harv.add_resource(self_, &ty);
        for t in &self.notify {
            t.harvested(self_, &ty);
        }
        self.base.queue_child(Wait::new(self.harv.info.bale_load_delay));
        false
    }

    fn on_last_run(&self, self_: &ActorRef) {
        self.claim_layer.remove_claim(self_);
    }

    fn cancel(&self, self_: &ActorRef, keep_queue: bool) {
        for n in &self.notify {
            n.movement_cancelled(self_);
        }
        self.base.cancel(self_, keep_queue);
    }
}

/// `Activities/MoveToDock.cs`.
pub struct MoveToDock {
    base: ActivityBase,
    dock_client: Rc<DockClientManager>,
    dock_host_actor: RefCell<Option<ActorRef>>,
    dock_host: RefCell<Option<Rc<DockHost>>>,
    notify_moving: Vec<Rc<dyn INotifyDockClientMoving>>,
    helper: MoveCooldownHelper,
    force_enter: bool,
    ignore_occupancy: bool,
    docking_cancelled: Cell<bool>,
}

impl MoveToDock {
    pub fn new(self_: &ActorRef, dock_host_actor: Option<ActorRef>, dock_host: Option<Rc<DockHost>>, force_enter: bool, ignore_occupancy: bool) -> ActivityRef {
        let helper = MoveCooldownHelper::new(self_.trait_or_default::<Mobile>());
        helper.retry_if_destination_blocked.set(true);
        Rc::new(MoveToDock {
            base: ActivityBase::default(),
            dock_client: self_.trait_::<DockClientManager>(),
            dock_host_actor: RefCell::new(dock_host_actor),
            dock_host: RefCell::new(dock_host),
            notify_moving: self_.traits_implementing::<dyn INotifyDockClientMoving>(),
            helper,
            force_enter,
            ignore_occupancy,
            docking_cancelled: Cell::new(false),
        })
    }
}

impl Activity for MoveToDock {
    activity_common!("MoveToDock");

    fn on_first_run(&self, self_: &ActorRef) {
        if self.dock_client.is_trait_disabled() {
            return;
        }
        let actor = self.dock_host_actor.borrow().clone();
        if let Some(a) = actor.filter(|_| self.dock_host.borrow().is_none()) {
            if a.is_dead() || !a.is_in_world() {
                self.docking_cancelled.set(true);
                return;
            }
            let hosts = self.dock_client.available_dock_hosts(&a, BitSet::default(), self.force_enter, self.ignore_occupancy);
            match closest_dock(&hosts, self_, &self.dock_client) {
                Some(l) => *self.dock_host.borrow_mut() = Some(l.1),
                None => self.docking_cancelled.set(true),
            }
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        if self.docking_cancelled.get() || self.dock_client.is_trait_disabled() {
            self.cancel(self_, true);
            return true;
        }
        if self.dock_host.borrow().as_ref().is_none_or(|h| !h.is_enabled_and_in_world()) {
            match self.dock_client.closest_dock(None, BitSet::default(), false, false) {
                Some((a, h)) => {
                    *self.dock_host.borrow_mut() = Some(h);
                    *self.dock_host_actor.borrow_mut() = Some(a);
                }
                None => {
                    self.base.queue_child(Wait::new(self.dock_client.info.search_for_dock_delay));
                    return false;
                }
            }
        }
        if let Some(r) = self.helper.tick(&self_.world(), false) {
            return r;
        }
        let host = self.dock_host.borrow().clone().expect("dock host");
        let host_actor = self.dock_host_actor.borrow().clone().expect("dock host actor");
        if self.dock_client.reserve_host(&host_actor, &host) {
            if host.queue_move_activity(&self.base, &host_actor, self_, &self.helper) {
                for n in &self.notify_moving {
                    n.moving_to_dock(self_, &host_actor, &host);
                }
                return false;
            }
            host.queue_dock_activity(&self.base, &host_actor, self_, &self.dock_client);
            return true;
        }
        self.base.queue_child(Wait::new(self.dock_client.info.search_for_dock_delay));
        false
    }

    fn cancel(&self, self_: &ActorRef, keep_queue: bool) {
        self.dock_client.unreserve_host();
        for n in &self.notify_moving {
            n.movement_cancelled(self_);
        }
        self.base.cancel(self_, keep_queue);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DockingState {
    Wait,
    Drag,
    Dock,
    Loop,
    Undock,
    Complete,
}

/// `Activities/GenericDockSequence.cs`.
pub struct GenericDockSequence {
    base: ActivityBase,
    dock_host_actor: ActorRef,
    dock_host: Rc<DockHost>,
    dock_client: Rc<DockClientManager>,
    dock_client_body: Option<Rc<dyn IDockClientBody>>,
    is_drag_required: bool,
    drag_length: i32,
    start_drag: WPos,
    end_drag: WPos,
    docking_state: Rc<Cell<DockingState>>,
    dock_initiated: Cell<bool>,
}

impl GenericDockSequence {
    fn new(self_: &ActorRef, client: &Rc<DockClientManager>, host_actor: &ActorRef, host: &Rc<DockHost>, info: &DockHostInfo) -> ActivityRef {
        let a = Rc::new(GenericDockSequence {
            base: ActivityBase::default(),
            dock_host_actor: host_actor.clone(),
            dock_host: host.clone(),
            dock_client: client.clone(),
            dock_client_body: self_.trait_or_default::<dyn IDockClientBody>(),
            is_drag_required: info.is_drag_required,
            drag_length: info.drag_length,
            start_drag: self_.center_position(),
            end_drag: host_actor.center_position() + info.drag_offset,
            docking_state: Rc::new(Cell::new(DockingState::Drag)),
            dock_initiated: Cell::new(false),
        });
        a.base.queue_child(Wait::new(info.dock_wait));
        a
    }

    fn host_gone(&self) -> bool {
        self.dock_host_actor.is_dead() || !self.dock_host_actor.is_in_world()
    }
}

impl Activity for GenericDockSequence {
    activity_common!("GenericDockSequence");

    fn tick(&self, self_: &ActorRef) -> bool {
        let state = self.docking_state.clone();
        match state.get() {
            DockingState::Wait => false,
            DockingState::Drag => {
                if self.base.is_canceling() || self.host_gone() || !self.dock_client.can_dock_at(&self.dock_host_actor, &self.dock_host, false, true) {
                    self.dock_client.unreserve_host();
                    return true;
                }
                state.set(DockingState::Dock);
                if self.is_drag_required {
                    self.base.queue_child(Drag::new(self_, self.start_drag, self.end_drag, self.drag_length, None));
                }
                false
            }
            DockingState::Dock => {
                if !self.base.is_canceling() && !self.host_gone() && self.dock_client.can_dock_at(&self.dock_host_actor, &self.dock_host, false, true) {
                    self.dock_initiated.set(true);
                    match &self.dock_client_body {
                        Some(b) => {
                            state.set(DockingState::Wait);
                            let s = state.clone();
                            b.play_dock_animation(self_, Rc::new(move || s.set(DockingState::Loop)));
                        }
                        None => state.set(DockingState::Loop),
                    }
                    self.dock_host.on_dock_started(&self.dock_host_actor, self_, &self.dock_client);
                    self.dock_client.on_dock_started(self_, &self.dock_host_actor, &self.dock_host);
                } else {
                    state.set(DockingState::Undock);
                }
                false
            }
            DockingState::Loop => {
                if self.base.is_canceling() || self.host_gone() || self.dock_client.on_dock_tick(self_, &self.dock_host_actor, &self.dock_host) {
                    state.set(DockingState::Undock);
                }
                false
            }
            DockingState::Undock => {
                if self.dock_initiated.get() {
                    match &self.dock_client_body {
                        Some(b) => {
                            state.set(DockingState::Wait);
                            let s = state.clone();
                            b.play_reverse_dock_animation(self_, Rc::new(move || s.set(DockingState::Complete)));
                        }
                        None => state.set(DockingState::Complete),
                    }
                } else {
                    state.set(DockingState::Complete);
                }
                false
            }
            DockingState::Complete => {
                self.dock_host.on_dock_completed(&self.dock_host_actor, self_, &self.dock_client);
                self.dock_client.on_dock_completed(self_, &self.dock_host_actor, &self.dock_host);
                if self.is_drag_required {
                    self.base.queue_child(Drag::new(self_, self.end_drag, self.start_drag, self.drag_length, None));
                }
                true
            }
        }
    }

    fn get_targets(&self, _self_: &ActorRef) -> Vec<Target> {
        vec![Target::from_actor(Some(&self.dock_host_actor))]
    }
}

