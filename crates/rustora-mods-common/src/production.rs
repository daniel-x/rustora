// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ProductionQueue`, `ClassicProductionQueue`, `TechTree`, `Production`, `Exit`, `Buildable`,
//! `Valued`, `ProvidesPrerequisite`, `PrimaryBuilding`, `RallyPoint`, `PlaceBuilding` and `Reservable`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use rustora_core::world_types::{CPos, CVec, WAngle, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::actor_init::{ActorInit, ActorInits, CenterPositionInit, FacingInit, FactionInit, LocationInit, OwnerInit};
use rustora_engine::conditional::*;
use rustora_engine::damage::AttackInfo;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{conditional_plumbing, conditional_simple, info_impl, load_info, register_traits};

use crate::base::*;
use crate::building::BuildingInfo;
use crate::interfaces::*;
use crate::locomotor::BlockedByActor;
use crate::mobile::{CreationActivityDelayInit, MobileInfo, RallyPointInit};
use crate::player::PlayerResources;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("ProductionQueue", |n| load_info!(ProductionQueueInfo, n));
    r.register("ClassicProductionQueue", |n| load_info!(ClassicProductionQueueInfo, n));
    r.register("TechTree", |n| load_info!(TechTreeInfo, n));
    r.register("Exit", |n| load_info!(ExitInfo, n));
    r.register("Production", |n| load_info!(ProductionInfo, n));
    r.register("Buildable", |n| load_info!(BuildableInfo, n));
    r.register("Valued", |n| load_info!(ValuedInfo, n));
    r.register("CustomSellValue", |n| load_info!(CustomSellValueInfo, n));
    r.register("ProvidesPrerequisite", |n| load_info!(ProvidesPrerequisiteInfo, n));
    r.register("ProvidesTechPrerequisite", |n| load_info!(ProvidesTechPrerequisiteInfo, n));
    r.register("PrimaryBuilding", |n| load_info!(PrimaryBuildingInfo, n));
    r.register("RallyPoint", |n| load_info!(RallyPointInfo, n));
    r.register("PlaceBuilding", |n| load_info!(PlaceBuildingInfo, n));
    r.register("GrantConditionOnPrerequisite", |n| load_info!(GrantConditionOnPrerequisiteInfo, n));
    r.register("GrantConditionOnPrerequisiteManager", |n| load_info!(GrantConditionOnPrerequisiteManagerInfo, n));
    r.register("LobbyPrerequisiteCheckbox", |n| load_info!(LobbyPrerequisiteCheckboxInfo, n));
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ValuedInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub cost: i32,
}

pub struct Valued;
register_traits!(Valued);
info_impl!(ValuedInfo => |_, _| Some(Rc::new(Valued)));

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct CustomSellValueInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub value: i32,
}

pub struct CustomSellValue;
register_traits!(CustomSellValue);
info_impl!(CustomSellValueInfo => |_, _| Some(Rc::new(CustomSellValue)));

/// `CustomSellValueExts.GetSellValue`.
pub fn get_sell_value(a: &ActorRef) -> i32 {
    if let Some(c) = a.info.trait_info::<CustomSellValueInfo>() {
        return c.value;
    }
    a.info.trait_info::<ValuedInfo>().map_or(0, |v| v.cost)
}

#[derive(Clone, Debug, FieldLoad)]
pub struct BuildableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub prerequisites: Vec<String>,
    pub queue: IndexSet<String>,
    pub build_at_production_type: Option<String>,
    pub build_limit: i32,
    pub force_faction: Option<String>,
    pub icon: String,
    pub icon_palette: String,
    pub icon_palette_is_player_palette: bool,
    pub build_duration: i32,
    pub build_duration_modifier: i32,
    pub build_palette_order: i32,
    pub description: Option<String>,
}

impl Default for BuildableInfo {
    fn default() -> Self {
        BuildableInfo {
            instance_name: None,
            prerequisites: Vec::new(),
            queue: IndexSet::new(),
            build_at_production_type: None,
            build_limit: 0,
            force_faction: None,
            icon: "icon".into(),
            icon_palette: "chrome".into(),
            icon_palette_is_player_palette: false,
            build_duration: -1,
            build_duration_modifier: 60,
            build_palette_order: 9999,
            description: None,
        }
    }
}

pub struct Buildable;
register_traits!(Buildable);
info_impl!(BuildableInfo => |_, _| Some(Rc::new(Buildable)));

fn get_initial_faction(ai: &ActorInfo, default: &str) -> String {
    ai.trait_info::<BuildableInfo>().and_then(|b| b.force_faction.clone()).unwrap_or_else(|| default.to_string())
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct TechTreeInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

struct Watcher {
    key: String,
    registered_by: Rc<dyn ITechTreeElement>,
    registered_ptr: usize,
    prerequisites: Vec<String>,
    has_prerequisites: Cell<bool>,
    limit: i32,
    hidden: Cell<bool>,
    initialized: Cell<bool>,
}

impl Watcher {
    fn prereq_unmet(prereq: &str, owned: &HashMap<String, i32>) -> bool {
        let without_tilde = prereq.replace('~', "");
        without_tilde.starts_with('!') ^ !owned.contains_key(&without_tilde.replace('!', ""))
    }

    fn update(&self, owned: &HashMap<String, i32>) {
        let reached = self.limit > 0 && owned.get(&self.key).is_some_and(|c| *c >= self.limit);
        let now_has = !reached && !self.prerequisites.iter().any(|p| Self::prereq_unmet(p, owned));
        let now_hidden = self.prerequisites.iter().any(|p| p.starts_with('~') && Self::prereq_unmet(p, owned));
        if !self.initialized.get() {
            self.initialized.set(true);
            self.has_prerequisites.set(!now_has);
            self.hidden.set(!now_hidden);
        }
        if now_hidden && !self.hidden.get() {
            self.registered_by.prerequisites_item_hidden(&self.key);
        }
        if !now_hidden && self.hidden.get() {
            self.registered_by.prerequisites_item_visible(&self.key);
        }
        if now_has && !self.has_prerequisites.get() {
            self.registered_by.prerequisites_available(&self.key);
        }
        if !now_has && self.has_prerequisites.get() {
            self.registered_by.prerequisites_unavailable(&self.key);
        }
        self.hidden.set(now_hidden);
        self.has_prerequisites.set(now_has);
    }
}

pub struct TechTree {
    watchers: RefCell<Vec<Rc<Watcher>>>,
    owner: PlayerRef,
    world: Weak<World>,
    me: Weak<TechTree>,
}

register_traits!(TechTree);
info_impl!(TechTreeInfo => |_, init| {
    let w = init.world();
    let t = Rc::new_cyclic(|me| TechTree {
        watchers: RefCell::new(Vec::new()),
        owner: init.self_.owner(),
        world: Rc::downgrade(&w),
        me: me.clone(),
    });
    let weak = t.me.clone();
    w.on_actor_changed(Rc::new(move |a: &ActorRef| {
        if let Some(t) = weak.upgrade() {
            t.actor_changed(a);
        }
    }));
    Some(t)
});

fn gather_owned_prerequisites(player: &PlayerRef, world: &World) -> HashMap<String, i32> {
    let mut ret: HashMap<String, i32> = HashMap::new();
    let valid = |a: &ActorRef| Rc::ptr_eq(&a.owner(), player) && a.is_in_world() && !a.is_dead();
    for (a, t) in world.actors_with_trait::<dyn ITechTreePrerequisite>() {
        if valid(&a) {
            for p in t.provides_prerequisites() {
                *ret.entry(p).or_insert(0) += 1;
            }
        }
    }
    for (a, _) in world.actors_with_trait::<Buildable>() {
        if valid(&a) && !ret.contains_key(&a.info.name) && a.info.trait_info::<BuildableInfo>().expect("Buildable").build_limit > 0 {
            *ret.entry(a.info.name.clone()).or_insert(0) += 1;
        }
    }
    ret
}

impl TechTree {
    fn world(&self) -> Rc<World> {
        self.world.upgrade().expect("world alive")
    }

    pub fn actor_changed(&self, a: &ActorRef) {
        let bi = a.info.trait_info::<BuildableInfo>();
        if a.owner_opt().is_some_and(|o| Rc::ptr_eq(&o, &self.owner))
            && (a.info.has_trait_info_named("ProvidesPrerequisite")
                || a.info.has_trait_info_named("ProvidesTechPrerequisite")
                || a.info.has_trait_info_named("LobbyPrerequisiteCheckbox")
                || bi.is_some_and(|b| b.build_limit > 0))
        {
            self.update();
        }
    }

    pub fn update(&self) {
        let owned = gather_owned_prerequisites(&self.owner, &self.world());
        let watchers = self.watchers.borrow().clone();
        for w in watchers {
            w.update(&owned);
        }
    }

    pub fn add(&self, key: &str, prerequisites: Vec<String>, limit: i32, tte: Rc<dyn ITechTreeElement>) {
        let registered_ptr = Rc::as_ptr(&tte) as *const () as usize;
        self.watchers.borrow_mut().push(Rc::new(Watcher {
            key: key.to_string(),
            registered_by: tte,
            registered_ptr,
            prerequisites,
            has_prerequisites: Cell::new(false),
            limit,
            hidden: Cell::new(false),
            initialized: Cell::new(false),
        }));
    }

    pub fn remove_key(&self, key: &str) {
        self.watchers.borrow_mut().retain(|w| w.key != key);
    }

    pub fn remove_element(&self, tte: usize) {
        self.watchers.borrow_mut().retain(|w| w.registered_ptr != tte);
    }

    pub fn has_prerequisites(&self, prerequisites: &[String]) -> bool {
        let owned = gather_owned_prerequisites(&self.owner, &self.world());
        prerequisites.iter().all(|p| {
            let t = p.replace('~', "");
            !(t.starts_with('!') ^ !owned.contains_key(&p.replace('!', "").replace('~', "")))
        })
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ExitInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub spawn_offset: WVec,
    pub exit_cell: CVec,
    pub facing: Option<WAngle>,
    pub production_types: IndexSet<String>,
    pub exit_delay: i32,
    pub priority: i32,
}

impl Default for ExitInfo {
    fn default() -> Self {
        ExitInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            spawn_offset: WVec::ZERO,
            exit_cell: CVec::ZERO,
            facing: None,
            production_types: IndexSet::new(),
            exit_delay: 0,
            priority: 1,
        }
    }
}

pub struct Exit {
    pub info: Rc<ExitInfo>,
    state: ConditionalState,
}

register_traits!(Exit: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ExitInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(Exit { info, state }))
});

conditional_simple!(Exit);

/// `ExitExts.Exits`.
pub fn exits(actor: &ActorRef, production_type: Option<&str>) -> Vec<Rc<Exit>> {
    if !actor.is_in_world() || actor.disposed() {
        return Vec::new();
    }
    actor
        .traits_implementing::<Exit>()
        .into_iter()
        .filter(|t| !t.state.is_trait_disabled())
        .filter(|e| match production_type.filter(|p| !p.is_empty()) {
            Some(p) => e.info.production_types.is_empty() || e.info.production_types.contains(p),
            None => true,
        })
        .collect()
}

/// `ExitExts.NearestExitOrDefault`.
pub fn nearest_exit_or_default(actor: &ActorRef, pos: rustora_core::world_types::WPos, production_type: Option<&str>, p: Option<&dyn Fn(&Exit) -> bool>) -> Option<Rc<Exit>> {
    let w = actor.world();
    let mut all = exits(actor, production_type);
    all.sort_by_key(|e| (std::cmp::Reverse(e.info.priority), (w.map().center_of_cell(actor.location() + e.info.exit_cell) - pos).length_squared()));
    match p {
        Some(f) => all.into_iter().find(|e| f(e)),
        None => all.into_iter().next(),
    }
}

/// `ExitExts.RandomExitOrDefault`.
pub fn random_exit_or_default(actor: &ActorRef, world: &World, production_type: Option<&str>, p: Option<&dyn Fn(&Exit) -> bool>) -> Option<Rc<Exit>> {
    if !actor.is_in_world() || actor.disposed() {
        return None;
    }
    let mut groups: IndexMap<i32, Vec<Rc<Exit>>> = IndexMap::new();
    for e in exits(actor, production_type) {
        groups.entry(e.info.priority).or_default().push(e);
    }
    for g in groups.values() {
        // The C# Shuffle iterator draws lazily and stops at the first accepted exit.
        let mut items = g.clone();
        let n = items.len();
        for i in 0..n {
            if i + 1 < n {
                let j = world.shared_random.borrow_mut().next_max((n - i) as i32) as usize;
                items.swap(i, i + j);
            }
            match p {
                None => return Some(items[i].clone()),
                Some(f) => {
                    if f(&items[i]) {
                        return Some(items[i].clone());
                    }
                }
            }
        }
    }
    None
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ProductionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    #[field(require)]
    pub produces: Vec<String>,
    pub update_faction_on_owner_change: bool,
}

pub struct Production {
    pub info: Rc<ProductionInfo>,
    state: ConditionalState,
    rp: RefCell<Option<Rc<RallyPoint>>>,
    faction: RefCell<String>,
}

register_traits!(Production: INotifyOwnerChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ProductionInfo => |info, init| {
    let state = ConditionalState::new_pausable(&info.base);
    let faction = init.inits.get::<FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone());
    Some(Rc::new(Production { info, state, rp: RefCell::new(None), faction: RefCell::new(faction) }))
});

impl ConditionalHooks for Production {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_plumbing!(Production);

impl INotifyCreated for Production {
    fn created(&self, self_: &ActorRef) {
        *self.rp.borrow_mut() = self_.trait_or_default::<RallyPoint>();
        conditional_created(self, self_);
    }
}

impl INotifyOwnerChanged for Production {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, _new: &PlayerRef) {
        if self.info.update_faction_on_owner_change {
            *self.faction.borrow_mut() = self_.owner().faction.clone();
        }
    }
}

fn has_occupy_space_info(a: &ActorInfo) -> bool {
    ["Mobile", "Aircraft", "Building", "Immobile", "Husk", "Crate"].iter().any(|n| a.has_trait_info_named(n))
}

fn initial_facing_of(a: &ActorInfo) -> WAngle {
    if let Some(m) = a.trait_info::<MobileInfo>() {
        return m.initial_facing;
    }
    if let Some(i) = a.trait_info::<crate::aircraft::AircraftInfo>() {
        return i.initial_facing;
    }
    WAngle::ZERO
}

impl Production {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.state.is_trait_paused()
    }

    pub fn faction(&self) -> String {
        self.faction.borrow().clone()
    }

    pub fn do_production(&self, self_: &ActorRef, producee: &Rc<ActorInfo>, exit_info: Option<&Rc<ExitInfo>>, production_type: &str, inits: ActorInits) {
        let mut exit = CPos::ZERO;
        let mut td = inits;
        if let Some(e) = exit_info {
            if self_.cache().occupies_space.is_some() && has_occupy_space_info(producee) {
                let w = self_.world();
                exit = self_.location() + e.exit_cell;
                let spawn = self_.center_position() + e.spawn_offset;
                let to = w.map().center_of_cell(exit);
                let initial_facing = match e.facing {
                    Some(f) => f,
                    None => {
                        let delta = to - spawn;
                        if delta.horizontal_length_squared() == 0 { initial_facing_of(producee) } else { delta.yaw() }
                    }
                };
                let exit_locations: Vec<CPos> = match self.rp.borrow().as_ref().filter(|r| !r.path.borrow().is_empty()) {
                    Some(r) => r.path.borrow().clone(),
                    None => vec![exit],
                };
                td.push(Rc::new(LocationInit::new(exit)));
                td.push(Rc::new(CenterPositionInit::new(spawn)));
                td.push(Rc::new(FacingInit::new(initial_facing)));
                td.push(Rc::new(CreationActivityDelayInit::new(e.exit_delay)));
                td.push(Rc::new(RallyPointInit::new(exit_locations)));
            }
        }
        let (me, name, pt) = (self_.clone(), producee.name.clone(), production_type.to_string());
        self_.world().add_frame_end_task(Box::new(move |w: &Rc<World>| {
            let new_unit = w.create_actor(&name, td.clone());
            if !me.is_dead() {
                for t in me.traits_implementing::<dyn INotifyProduction>() {
                    t.unit_produced(&me, &new_unit, exit);
                }
            }
            for (a, t) in w.actors_with_trait::<dyn INotifyOtherProduction>() {
                t.unit_produced_by_other(&a, &me, &new_unit, &pt, &td);
            }
        }));
    }

    fn select_exit(&self, self_: &ActorRef, producee: &ActorInfo, production_type: &str) -> Option<Rc<Exit>> {
        let check = |e: &Exit| can_use_exit(self_, producee, &e.info);
        let rp = self.rp.borrow().clone();
        match rp.filter(|r| !r.path.borrow().is_empty()) {
            None => random_exit_or_default(self_, &self_.world(), Some(production_type), Some(&check)),
            Some(r) => {
                let first = r.path.borrow()[0];
                nearest_exit_or_default(self_, self_.world().map().center_of_cell(first), Some(production_type), Some(&check))
            }
        }
    }

    pub fn produce(&self, self_: &ActorRef, producee: &Rc<ActorInfo>, production_type: &str, inits: ActorInits, _refundable_value: i32) -> bool {
        if self.is_trait_disabled() || self.is_trait_paused() || crate::aircraft::is_reserved(self_) {
            return false;
        }
        let exit = self.select_exit(self_, producee, production_type);
        if exit.is_some() || self_.cache().occupies_space.is_none() || !has_occupy_space_info(producee) {
            self.do_production(self_, producee, exit.as_ref().map(|e| &e.info), production_type, inits);
            return true;
        }
        false
    }
}

fn can_use_exit(self_: &ActorRef, producee: &ActorInfo, s: &ExitInfo) -> bool {
    let cell = self_.location() + s.exit_cell;
    crate::activities::notify_blocker(self_, cell);
    producee
        .trait_info::<MobileInfo>()
        .is_none_or(|m| m.can_enter_cell(&self_.world(), Some(self_), cell, subcell::FULL_CELL, Some(self_), BlockedByActor::All))
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ProductionQueueInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    #[field(name = "Type")]
    pub ty: String,
    pub display_order: i32,
    pub group: Option<String>,
    pub factions: IndexSet<String>,
    pub sticky: bool,
    pub pay_up_front: bool,
    pub disallow_paused: bool,
    pub build_duration_modifier: i32,
    pub item_limit: i32,
    pub queue_limit: i32,
    pub low_power_modifier: i32,
    pub infinite_build_limit: i32,
    pub ready_audio: Option<String>,
    pub blocked_audio: Option<String>,
    pub limited_audio: Option<String>,
    pub queued_audio: Option<String>,
    pub on_hold_audio: Option<String>,
    pub cancelled_audio: Option<String>,
}

impl Default for ProductionQueueInfo {
    fn default() -> Self {
        ProductionQueueInfo {
            instance_name: None,
            ty: String::new(),
            display_order: 0,
            group: None,
            factions: IndexSet::new(),
            sticky: true,
            pay_up_front: false,
            disallow_paused: false,
            build_duration_modifier: 100,
            item_limit: 999,
            queue_limit: 0,
            low_power_modifier: 100,
            infinite_build_limit: -1,
            ready_audio: None,
            blocked_audio: None,
            limited_audio: None,
            queued_audio: None,
            on_hold_audio: None,
            cancelled_audio: None,
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ClassicProductionQueueInfo {
    #[field(flatten)]
    pub base: ProductionQueueInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub speed_up: bool,
    pub build_time_speed_reduction: Vec<i32>,
}

impl Default for ClassicProductionQueueInfo {
    fn default() -> Self {
        ClassicProductionQueueInfo {
            base: ProductionQueueInfo::default(),
            instance_name: None,
            speed_up: false,
            build_time_speed_reduction: vec![100, 86, 75, 67, 60, 55, 50],
        }
    }
}

#[derive(Default)]
struct ProductionState {
    visible: Cell<bool>,
    buildable: Cell<bool>,
}

/// `ProductionItem`.
pub struct ProductionItem {
    pub item: String,
    queue: Weak<ProductionQueue>,
    pub total_cost: i32,
    on_complete: Rc<dyn Fn()>,
    pub total_time: Cell<i32>,
    pub remaining_time: Cell<i32>,
    pub remaining_cost: Cell<i32>,
    pub resources_paid: Cell<i32>,
    pub paused: Cell<bool>,
    pub done: Cell<bool>,
    pub started: Cell<bool>,
    pub slowdown: Cell<i32>,
    pub infinite: Cell<bool>,
    pub build_palette_order: i32,
    ai: Rc<ActorInfo>,
    bi: Rc<BuildableInfo>,
    pm: Option<Rc<PowerManager>>,
}

impl ProductionItem {
    fn new(queue: &ProductionQueue, item: &str, cost: i32, pm: Option<Rc<PowerManager>>, on_complete: Rc<dyn Fn()>) -> Rc<ProductionItem> {
        let ai = queue.actor.world().rules().actor(item).expect("item actor");
        let bi = ai.trait_info::<BuildableInfo>().expect("Buildable");
        Rc::new(ProductionItem {
            item: item.to_string(),
            queue: queue.me.clone(),
            total_cost: cost,
            on_complete,
            total_time: Cell::new(1),
            remaining_time: Cell::new(1),
            remaining_cost: Cell::new(cost),
            resources_paid: Cell::new(0),
            paused: Cell::new(false),
            done: Cell::new(false),
            started: Cell::new(false),
            slowdown: Cell::new(0),
            infinite: Cell::new(false),
            build_palette_order: bi.build_palette_order,
            ai,
            bi,
            pm,
        })
    }

    fn tick(&self, pr: &PlayerResources) {
        let queue = self.queue.upgrade().expect("queue alive");
        if !self.started.get() {
            let time = queue.get_build_time(&self.ai, &self.bi);
            if time > 0 {
                self.remaining_time.set(time);
                self.total_time.set(time);
            }
            self.started.set(true);
        }
        if self.done.get() {
            (self.on_complete)();
            return;
        }
        if self.paused.get() {
            return;
        }
        if let Some(pm) = &self.pm {
            if pm.power_state() != PowerState::Normal {
                self.slowdown.set(self.slowdown.get() - 100);
                if self.slowdown.get() < 0 {
                    self.slowdown.set(queue.info.low_power_modifier + self.slowdown.get());
                } else {
                    return;
                }
            }
        }
        if !queue.info.pay_up_front {
            let rt = self.remaining_time.get();
            let expected = if rt == 1 {
                0
            } else {
                self.total_cost.wrapping_mul(rt) / self.total_time.get().max(1)
            };
            let cost_this_frame = self.remaining_cost.get() - expected;
            let res = pr.resources.get();
            if res > 0 && res <= cost_this_frame {
                self.resources_paid.set(self.resources_paid.get() + res);
            } else if res > cost_this_frame {
                self.resources_paid.set(self.resources_paid.get() + cost_this_frame);
            }
            if cost_this_frame != 0 && !pr.take_cash(cost_this_frame, true) {
                self.resources_paid.set(self.resources_paid.get() - pr.resources.get());
                return;
            }
            self.remaining_cost.set(self.remaining_cost.get() - cost_this_frame);
        }
        self.remaining_time.set(self.remaining_time.get() - 1);
        if self.remaining_time.get() > 0 {
            return;
        }
        self.done.set(true);
    }
}

/// Mutable per-owner references of a queue.
struct QueueOwnerRefs {
    player_power: Option<Rc<PowerManager>>,
    player_resources: Rc<PlayerResources>,
    developer_mode: Rc<DeveloperMode>,
    tech_tree: Rc<TechTree>,
}

/// `ProductionQueue` and, when `classic` is set, `ClassicProductionQueue`.
pub struct ProductionQueue {
    pub info: Rc<ProductionQueueInfo>,
    classic: Option<Rc<ClassicProductionQueueInfo>>,
    me: Weak<ProductionQueue>,
    pub actor: ActorRef,
    producible: RefCell<IndexMap<String, ProductionState>>,
    queue: RefCell<Vec<Rc<ProductionItem>>>,
    production_traits: RefCell<Vec<Rc<Production>>>,
    refs: RefCell<Option<QueueOwnerRefs>>,
    enabled: Cell<bool>,
    faction: RefCell<String>,
    is_valid_faction: Cell<bool>,
}

fn new_queue(info: Rc<ProductionQueueInfo>, classic: Option<Rc<ClassicProductionQueueInfo>>, init: &rustora_engine::actor_init::ActorInitializer) -> Rc<ProductionQueue> {
    let faction = init.inits.get::<FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone());
    let valid = info.factions.is_empty() || info.factions.contains(&faction);
    Rc::new_cyclic(|me| ProductionQueue {
        info,
        classic,
        me: me.clone(),
        actor: init.self_.clone(),
        producible: RefCell::new(IndexMap::new()),
        queue: RefCell::new(Vec::new()),
        production_traits: RefCell::new(Vec::new()),
        refs: RefCell::new(None),
        enabled: Cell::new(valid),
        faction: RefCell::new(faction),
        is_valid_faction: Cell::new(valid),
    })
}

impl TraitObject for ProductionQueue {
    fn register(self: Rc<Self>, dict: &rustora_engine::trait_dict::TraitDict, actor: &ActorRef) {
        dict.add::<ProductionQueue>(actor, self.clone());
        dict.add::<dyn IResolveOrder>(actor, self.clone());
        dict.add::<dyn ITick>(actor, self.clone());
        dict.add::<dyn INotifyOwnerChanged>(actor, self.clone());
        dict.add::<dyn INotifyKilled>(actor, self.clone());
        dict.add::<dyn INotifySold>(actor, self.clone());
        dict.add::<dyn ISync>(actor, self.clone());
        dict.add::<dyn INotifyTransform>(actor, self.clone());
        dict.add::<dyn INotifyCreated>(actor, self);
    }
    fn type_name(&self) -> &'static str {
        if self.classic.is_some() { "ClassicProductionQueue" } else { "ProductionQueue" }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

info_impl!(ProductionQueueInfo => |info, init| Some(new_queue(info, None, init)));
info_impl!(ClassicProductionQueueInfo => |info, init| Some(new_queue(Rc::new(info.base.clone()), Some(info), init)));

impl ISync for ProductionQueue {
    fn sync_hash(&self) -> i32 {
        sync_bool(self.enabled.get()) ^ sync_bool(self.is_valid_faction.get())
    }
}

impl INotifyCreated for ProductionQueue {
    fn created(&self, self_: &ActorRef) {
        let pa = self_.owner().player_actor().clone();
        *self.refs.borrow_mut() = Some(QueueOwnerRefs {
            player_power: pa.trait_or_default::<PowerManager>(),
            player_resources: pa.trait_::<PlayerResources>(),
            developer_mode: pa.trait_::<DeveloperMode>(),
            tech_tree: pa.trait_::<TechTree>(),
        });
        *self.production_traits.borrow_mut() = self_
            .traits_implementing::<Production>()
            .into_iter()
            .filter(|p| p.info.produces.contains(&self.info.ty))
            .collect();
        self.cache_producibles();
    }
}

impl ITechTreeElement for ProductionQueue {
    fn prerequisites_available(&self, key: &str) {
        self.producible.borrow()[key].buildable.set(true);
    }
    fn prerequisites_unavailable(&self, key: &str) {
        self.producible.borrow()[key].buildable.set(false);
    }
    fn prerequisites_item_hidden(&self, key: &str) {
        self.producible.borrow()[key].visible.set(false);
    }
    fn prerequisites_item_visible(&self, key: &str) {
        self.producible.borrow()[key].visible.set(true);
    }
}

impl INotifyOwnerChanged for ProductionQueue {
    fn on_owner_changed(&self, self_: &ActorRef, old_owner: &PlayerRef, new_owner: &PlayerRef) {
        self.clear_queue();
        let pa = new_owner.player_actor().clone();
        *self.refs.borrow_mut() = Some(QueueOwnerRefs {
            player_power: pa.trait_or_default::<PowerManager>(),
            player_resources: pa.trait_::<PlayerResources>(),
            developer_mode: pa.trait_::<DeveloperMode>(),
            tech_tree: pa.trait_::<TechTree>(),
        });
        if !self.info.sticky {
            let f = self_.owner().faction.clone();
            self.is_valid_faction.set(self.info.factions.is_empty() || self.info.factions.contains(&f));
            *self.faction.borrow_mut() = f;
        }
        old_owner.player_actor().trait_::<TechTree>().remove_element(self.me.as_ptr() as *const () as usize);
        self.cache_producibles();
        self.tech_tree().update();
    }
}

impl INotifyKilled for ProductionQueue {
    fn killed(&self, killed: &ActorRef, _e: &AttackInfo) {
        if killed.id == self.actor.id {
            self.clear_queue();
            self.enabled.set(false);
        }
    }
}

impl INotifySold for ProductionQueue {
    fn selling(&self, _self_: &ActorRef) {
        self.clear_queue();
        self.enabled.set(false);
    }
    fn sold(&self, _self_: &ActorRef) {}
}

impl INotifyTransform for ProductionQueue {
    fn before_transform(&self, _self_: &ActorRef) {
        self.clear_queue();
        self.enabled.set(false);
    }
    fn on_transform(&self, _self_: &ActorRef) {}
    fn after_transform(&self, _to_actor: &ActorRef) {}
}

impl ProductionQueue {
    fn player_resources(&self) -> Rc<PlayerResources> {
        self.refs.borrow().as_ref().expect("created").player_resources.clone()
    }

    fn developer_mode(&self) -> Rc<DeveloperMode> {
        self.refs.borrow().as_ref().expect("created").developer_mode.clone()
    }

    fn tech_tree(&self) -> Rc<TechTree> {
        self.refs.borrow().as_ref().expect("created").tech_tree.clone()
    }

    fn player_power(&self) -> Option<Rc<PowerManager>> {
        self.refs.borrow().as_ref().expect("created").player_power.clone()
    }

    pub fn enabled(&self) -> bool {
        self.enabled.get()
    }

    pub fn is_valid_faction(&self) -> bool {
        self.is_valid_faction.get()
    }

    pub fn faction(&self) -> String {
        self.faction.borrow().clone()
    }

    pub fn all_queued(&self) -> Vec<Rc<ProductionItem>> {
        self.queue.borrow().clone()
    }

    pub fn current_item(&self) -> Option<Rc<ProductionItem>> {
        self.queue.borrow().first().cloned()
    }

    fn clear_queue(&self) {
        let items = std::mem::take(&mut *self.queue.borrow_mut());
        let pr = self.refs.borrow().as_ref().map(|r| r.player_resources.clone());
        let Some(pr) = pr else { return };
        for item in items {
            if item.resources_paid.get() > 0 {
                pr.give_resources(item.resources_paid.get());
                item.remaining_cost.set(item.remaining_cost.get() + item.resources_paid.get());
            }
            pr.give_cash(item.total_cost - item.remaining_cost.get());
        }
    }

    fn cache_producibles(&self) {
        self.producible.borrow_mut().clear();
        if !self.enabled.get() {
            return;
        }
        let w = self.actor.world();
        let tt = self.tech_tree();
        let me: Rc<dyn ITechTreeElement> = self.me.upgrade().expect("queue alive");
        for (name, a) in w.rules().actors() {
            let Some(bi) = a.trait_info::<BuildableInfo>() else { continue };
            if !bi.queue.contains(&self.info.ty) {
                continue;
            }
            self.producible.borrow_mut().insert(
                name.clone(),
                ProductionState {
                    visible: Cell::new(true),
                    buildable: Cell::new(false),
                },
            );
            tt.add(name, bi.prerequisites.clone(), bi.build_limit, me.clone());
        }
    }

    pub fn is_in_queue(&self, actor: &str) -> bool {
        self.queue.borrow().iter().any(|i| i.item == actor)
    }

    /// `BuildableItems` as names, in producible order.
    pub fn buildable_items(&self) -> Vec<String> {
        if self.classic.is_some() && !self.enabled.get() {
            return Vec::new();
        }
        let traits = self.production_traits.borrow();
        if !traits.is_empty() && traits.iter().all(|p| p.is_trait_disabled()) {
            return Vec::new();
        }
        if !self.enabled.get() {
            return Vec::new();
        }
        let all_tech = self.developer_mode().all_tech();
        let w = self.actor.world();
        let prod = self.producible.borrow();
        let base = prod.iter().filter(|(_, s)| all_tech || s.buildable.get()).map(|(k, _)| k.clone());
        if self.info.pay_up_front {
            let cash = self.player_resources().get_cash_and_resources();
            return base
                .filter(|a| {
                    let ai = w.rules().actor(a).expect("actor");
                    self.get_production_cost(&ai) <= cash || self.is_in_queue(a)
                })
                .collect();
        }
        base.collect()
    }

    /// `AllItems` as names.
    pub fn all_items(&self) -> Vec<String> {
        if self.classic.is_some() && !self.enabled.get() {
            return Vec::new();
        }
        let traits = self.production_traits.borrow();
        if !traits.is_empty() && traits.iter().all(|p| p.is_trait_disabled()) {
            return Vec::new();
        }
        let all_tech = self.developer_mode().all_tech();
        self.producible
            .borrow()
            .iter()
            .filter(|(_, s)| all_tech || s.buildable.get() || s.visible.get())
            .map(|(k, _)| k.clone())
            .collect()
    }

    pub fn can_build(&self, actor: &str) -> bool {
        match self.producible.borrow().get(actor) {
            Some(ps) => ps.buildable.get() || self.developer_mode().all_tech(),
            None => false,
        }
    }

    fn tick_inner(&self, all_paused: bool) {
        self.cancel_unbuildable_items();
        let first = self.queue.borrow().first().cloned();
        if !all_paused {
            if let Some(f) = first {
                f.tick(&self.player_resources());
            }
        }
    }

    fn cancel_unbuildable_items(&self) {
        if self.queue.borrow().is_empty() {
            return;
        }
        let buildable: std::collections::HashSet<String> = self.buildable_items().into_iter().collect();
        let pr = self.player_resources();
        let mut i = self.queue.borrow().len();
        while i > 0 {
            i -= 1;
            let item = self.queue.borrow()[i].clone();
            if buildable.contains(&item.item) {
                continue;
            }
            if item.resources_paid.get() > 0 {
                pr.give_resources(item.resources_paid.get());
                item.remaining_cost.set(item.remaining_cost.get() + item.resources_paid.get());
            }
            pr.give_cash(item.total_cost - item.remaining_cost.get());
            self.end_production(&item);
        }
    }

    pub fn get_build_time(&self, unit: &ActorInfo, bi: &BuildableInfo) -> i32 {
        if self.developer_mode().fast_build() {
            return 0;
        }
        let mut time = bi.build_duration;
        if time == -1 {
            time = self.get_production_cost(unit);
        }
        let tt = self.tech_tree();
        let mut modifiers: Vec<i32> = unit
            .trait_infos::<dyn IProductionTimeModifierInfo>()
            .iter()
            .map(|t| t.get_production_time_modifier(&tt, &self.info.ty))
            .collect();
        modifiers.push(bi.build_duration_modifier);
        modifiers.push(self.info.build_duration_modifier);
        let mut time = util::apply_percentage_modifiers(time, modifiers);
        if let Some(c) = self.classic.as_ref().filter(|c| c.speed_up) {
            let ty = bi.build_at_production_type.as_deref().unwrap_or(&self.info.ty);
            let owner = self.actor.owner();
            let count = self
                .actor
                .world()
                .actors_with_trait::<Production>()
                .iter()
                .filter(|(a, p)| !p.is_trait_disabled() && !p.is_trait_paused() && Rc::ptr_eq(&a.owner(), &owner) && p.info.produces.iter().any(|x| x == ty))
                .count() as i32;
            let m = count.clamp(1, c.build_time_speed_reduction.len() as i32) - 1;
            time = time * c.build_time_speed_reduction[m as usize] / 100;
        }
        time
    }

    pub fn get_production_cost(&self, unit: &ActorInfo) -> i32 {
        let Some(valued) = unit.trait_info::<ValuedInfo>() else { return 0 };
        let tt = self.tech_tree();
        let modifiers: Vec<i32> = unit
            .trait_infos::<dyn IProductionCostModifierInfo>()
            .iter()
            .map(|t| t.get_production_cost_modifier(&tt, &self.info.ty))
            .collect();
        util::apply_percentage_modifiers(valued.cost, modifiers)
    }

    fn pause_production(&self, name: &str, paused: bool) {
        if let Some(i) = self.queue.borrow().iter().find(|a| a.item == name) {
            i.paused.set(paused);
        }
    }

    fn cancel_production(&self, name: &str, number: u32) {
        for _ in 0..number {
            if !self.cancel_production_inner(name) {
                break;
            }
        }
    }

    fn cancel_production_inner(&self, name: &str) -> bool {
        let item = self.queue.borrow().iter().rev().find(|a| a.item == name).cloned();
        let Some(item) = item else { return false };
        if item.infinite.get() {
            item.infinite.set(false);
            for _ in 1..self.info.infinite_build_limit {
                let n = ProductionItem::new(self, &item.item, item.total_cost, self.player_power(), item.on_complete.clone());
                self.queue.borrow_mut().push(n);
            }
        } else {
            let pr = self.player_resources();
            if item.resources_paid.get() > 0 {
                pr.give_resources(item.resources_paid.get());
                item.remaining_cost.set(item.remaining_cost.get() + item.resources_paid.get());
            }
            pr.give_cash(item.total_cost - item.remaining_cost.get());
            self.end_production(&item);
        }
        true
    }

    pub fn end_production(&self, item: &Rc<ProductionItem>) {
        {
            let mut q = self.queue.borrow_mut();
            if let Some(i) = q.iter().position(|x| Rc::ptr_eq(x, item)) {
                q.remove(i);
            }
        }
        if item.infinite.get() {
            let n = ProductionItem::new(self, &item.item, item.total_cost, self.player_power(), item.on_complete.clone());
            n.infinite.set(true);
            self.queue.borrow_mut().push(n);
        }
    }

    fn begin_production(&self, item: Rc<ProductionItem>, has_priority: bool) {
        let pr = self.player_resources();
        if self.info.pay_up_front {
            let res = pr.resources.get();
            if res > 0 && res <= item.total_cost {
                item.resources_paid.set(res);
            } else if res > item.total_cost {
                item.resources_paid.set(item.total_cost);
            }
            pr.take_cash(item.total_cost, false);
            item.remaining_cost.set(0);
        }
        if self.queue.borrow().iter().any(|i| i.item == item.item && i.infinite.get()) {
            return;
        }
        {
            let mut q = self.queue.borrow_mut();
            if has_priority && q.len() > 1 {
                q.insert(1, item.clone());
            } else {
                q.push(item.clone());
            }
        }
        if self.info.infinite_build_limit < 0 {
            return;
        }
        let queued: Vec<Rc<ProductionItem>> = self.queue.borrow().iter().filter(|i| i.item == item.item).cloned().collect();
        if queued.len() as i32 <= self.info.infinite_build_limit {
            return;
        }
        queued[0].infinite.set(true);
        for q in &queued[1..] {
            if q.resources_paid.get() > 0 {
                pr.give_resources(q.resources_paid.get());
                q.remaining_cost.set(q.remaining_cost.get() + q.resources_paid.get());
            }
            pr.give_cash(q.total_cost - q.remaining_cost.get());
            self.end_production(q);
        }
    }

    /// `MostLikelyProducer`.
    pub fn most_likely_producer(&self) -> Option<(ActorRef, Rc<Production>)> {
        if self.classic.is_some() {
            let w = self.actor.world();
            let owner = self.actor.owner();
            let mut v: Vec<(ActorRef, Rc<Production>)> = w
                .actors_with_trait::<Production>()
                .into_iter()
                .filter(|(a, p)| Rc::ptr_eq(&a.owner(), &owner) && !p.is_trait_disabled() && p.info.produces.contains(&self.info.ty))
                .collect();
            v.sort_by_key(|(a, p)| (p.is_trait_paused(), std::cmp::Reverse(is_primary_building(a)), std::cmp::Reverse(a.id)));
            return v.into_iter().next();
        }
        let traits = self.production_traits.borrow();
        let mut v: Vec<&Rc<Production>> = traits.iter().filter(|p| !p.is_trait_disabled() && p.info.produces.contains(&self.info.ty)).collect();
        v.sort_by_key(|p| p.is_trait_paused());
        v.first().map(|p| (self.actor.clone(), (*p).clone()))
    }

    fn build_unit(&self, unit: &Rc<ActorInfo>) -> bool {
        let bi = unit.trait_info::<BuildableInfo>().expect("Buildable");
        let ty = if self.developer_mode().all_tech() {
            self.info.ty.clone()
        } else {
            bi.build_at_production_type.clone().unwrap_or_else(|| self.info.ty.clone())
        };
        if self.classic.is_some() {
            let w = self.actor.world();
            let owner = self.actor.owner();
            let mut producers: Vec<(ActorRef, Rc<Production>)> = w
                .actors_with_trait::<Production>()
                .into_iter()
                .filter(|(a, p)| Rc::ptr_eq(&a.owner(), &owner) && !p.is_trait_disabled() && p.info.produces.contains(&ty))
                .collect();
            producers.sort_by_key(|(a, _)| (std::cmp::Reverse(is_primary_building(a)), std::cmp::Reverse(a.id)));
            let any = !producers.is_empty();
            for (a, p) in producers {
                if p.is_trait_paused() {
                    continue;
                }
                let inits = ActorInits::new().with(OwnerInit::from_player(&owner)).with(FactionInit::new(get_initial_faction(unit, &p.faction())));
                let item = self.first_done(&unit.name);
                if p.produce(&a, unit, &ty, inits, item.total_cost) {
                    self.end_production(&item);
                    return true;
                }
            }
            if !any {
                self.cancel_production(&unit.name, 1);
            }
            return false;
        }
        let producer = self.most_likely_producer();
        let Some((_, p)) = producer.filter(|_| self.actor.is_in_world() && !self.actor.is_dead()) else {
            self.cancel_production(&unit.name, 1);
            return false;
        };
        let inits = ActorInits::new()
            .with(OwnerInit::from_player(&self.actor.owner()))
            .with(FactionInit::new(get_initial_faction(unit, &self.faction())));
        let item = self.first_done(&unit.name);
        if !p.is_trait_paused() && p.produce(&self.actor, unit, &ty, inits, item.total_cost) {
            self.end_production(&item);
            return true;
        }
        false
    }

    fn first_done(&self, name: &str) -> Rc<ProductionItem> {
        self.queue.borrow().iter().find(|i| i.done.get() && i.item == name).cloned().expect("done item")
    }
}

impl ITick for ProductionQueue {
    fn tick(&self, self_: &ActorRef) {
        if self.classic.is_some() {
            self.enabled.set(false);
            let mut is_active = false;
            let owner = self_.owner();
            for (a, p) in self_.world().actors_with_trait::<Production>() {
                if p.is_trait_disabled() {
                    continue;
                }
                if !Rc::ptr_eq(&a.owner(), &owner) || !p.info.produces.contains(&self.info.ty) {
                    continue;
                }
                self.enabled.set(self.enabled.get() | self.is_valid_faction.get());
                is_active |= !p.is_trait_paused();
            }
            if !self.enabled.get() {
                self.clear_queue();
            }
            self.tick_inner(!is_active);
            return;
        }
        let (mut any_enabled, mut any_unpaused) = (false, false);
        for p in self.production_traits.borrow().iter() {
            any_enabled |= !p.is_trait_disabled();
            any_unpaused |= !p.is_trait_paused();
        }
        if !any_enabled {
            self.clear_queue();
        }
        self.enabled.set(self.is_valid_faction.get() && any_enabled);
        self.tick_inner(!any_unpaused);
    }
}

impl IResolveOrder for ProductionQueue {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if !self.enabled.get() {
            return;
        }
        let name = order.target_string.clone().unwrap_or_default();
        match order.order_string.as_str() {
            "StartProduction" => {
                let w = self_.world();
                let unit = w.rules().actor(&name).unwrap_or_else(|| panic!("unknown actor {name}"));
                let bi = unit.trait_info::<BuildableInfo>().expect("Buildable");
                if !bi.queue.contains(&self.info.ty) {
                    return;
                }
                if !self.buildable_items().iter().any(|b| *b == name) {
                    return;
                }
                let mut from_limit = i32::MAX;
                if !self.developer_mode().all_tech() {
                    let in_queue = self.queue.borrow().iter().filter(|i| i.item == name).count() as i32;
                    if self.info.queue_limit > 0 {
                        from_limit = self.info.queue_limit - self.queue.borrow().len() as i32;
                    }
                    if self.info.item_limit > 0 {
                        from_limit = from_limit.min(self.info.item_limit - in_queue);
                    }
                    if bi.build_limit > 0 {
                        let owner = self_.owner();
                        let owned = w
                            .actors_having_trait::<Buildable>()
                            .iter()
                            .filter(|a| a.info.name == name && Rc::ptr_eq(&a.owner(), &owner))
                            .count() as i32;
                        from_limit = from_limit.min(bi.build_limit - (in_queue + owned));
                    }
                    if from_limit <= 0 {
                        return;
                    }
                }
                let cost = self.get_production_cost(&unit);
                let amount = (from_limit as u32).min(order.extra_data);
                for _ in 0..amount {
                    if self.info.pay_up_front && cost > self.player_resources().get_cash_and_resources() {
                        return;
                    }
                    let (weak, u, world) = (self.me.clone(), unit.clone(), Rc::downgrade(&w));
                    let on_complete: Rc<dyn Fn()> = Rc::new(move || {
                        let (weak, u) = (weak.clone(), u.clone());
                        let Some(world) = world.upgrade() else { return };
                        world.add_frame_end_task(Box::new(move |_| {
                            let Some(q) = weak.upgrade() else { return };
                            if !q.queue.borrow().iter().any(|i| i.done.get() && i.item == u.name) {
                                return;
                            }
                            if !u.has_trait_info::<BuildingInfo>() {
                                q.build_unit(&u);
                            }
                        }));
                    });
                    let item = ProductionItem::new(self, &name, cost, self.player_power(), on_complete);
                    self.begin_production(item, !order.queued);
                }
            }
            "PauseProduction" => self.pause_production(&name, order.extra_data != 0),
            "CancelProduction" => self.cancel_production(&name, order.extra_data),
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ProvidesPrerequisiteInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub prerequisite: Option<String>,
    pub requires_prerequisites: Vec<String>,
    pub factions: IndexSet<String>,
    pub reset_on_owner_change: bool,
}

pub struct ProvidesPrerequisite {
    info: Rc<ProvidesPrerequisiteInfo>,
    state: ConditionalState,
    prerequisites: Vec<String>,
    enabled: Cell<bool>,
    tech_tree: RefCell<Option<Rc<TechTree>>>,
    faction: RefCell<String>,
}

register_traits!(ProvidesPrerequisite: ITechTreePrerequisite, INotifyOwnerChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ProvidesPrerequisiteInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    let prerequisites = vec![info.prerequisite.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| init.self_.info.name.clone())];
    let faction = init.inits.get::<FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone());
    Some(Rc::new(ProvidesPrerequisite { info, state, prerequisites, enabled: Cell::new(false), tech_tree: RefCell::new(None), faction: RefCell::new(faction) }))
});

impl ProvidesPrerequisite {
    fn update(&self) {
        self.enabled.set(!self.state.is_trait_disabled());
        if self.state.is_trait_disabled() {
            return;
        }
        if !self.info.factions.is_empty() {
            self.enabled.set(self.info.factions.contains(&*self.faction.borrow()));
        }
        if !self.info.requires_prerequisites.is_empty() && self.enabled.get() {
            let tt = self.tech_tree.borrow().clone().expect("created");
            self.enabled.set(tt.has_prerequisites(&self.info.requires_prerequisites));
        }
    }
}

impl ConditionalHooks for ProvidesPrerequisite {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, self_: &ActorRef) {
        self.update();
        self.tech_tree.borrow().clone().expect("created").actor_changed(self_);
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        self.update();
        self.tech_tree.borrow().clone().expect("created").actor_changed(self_);
    }
}

conditional_plumbing!(ProvidesPrerequisite);

impl INotifyCreated for ProvidesPrerequisite {
    fn created(&self, self_: &ActorRef) {
        *self.tech_tree.borrow_mut() = Some(self_.owner().player_actor().trait_::<TechTree>());
        self.update();
        conditional_created(self, self_);
    }
}

impl ITechTreePrerequisite for ProvidesPrerequisite {
    fn provides_prerequisites(&self) -> Vec<String> {
        if self.enabled.get() { self.prerequisites.clone() } else { Vec::new() }
    }
}

impl INotifyOwnerChanged for ProvidesPrerequisite {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        *self.tech_tree.borrow_mut() = Some(new_owner.player_actor().trait_::<TechTree>());
        if self.info.reset_on_owner_change {
            *self.faction.borrow_mut() = new_owner.faction.clone();
        }
        self.update();
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ProvidesTechPrerequisiteInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub prerequisites: Vec<String>,
}

pub struct ProvidesTechPrerequisite {
    info: Rc<ProvidesTechPrerequisiteInfo>,
    enabled: bool,
}

register_traits!(ProvidesTechPrerequisite: ITechTreePrerequisite);
info_impl!(ProvidesTechPrerequisiteInfo => |info, init| {
    let w = init.world();
    let enabled = w.world_actor().trait_or_default::<MapOptions>().is_some_and(|m| Some(m.tech_level()) == info.id);
    Some(Rc::new(ProvidesTechPrerequisite { info, enabled }))
});

impl ITechTreePrerequisite for ProvidesTechPrerequisite {
    fn provides_prerequisites(&self) -> Vec<String> {
        if self.enabled { self.info.prerequisites.clone() } else { Vec::new() }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct LobbyPrerequisiteCheckboxInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require, name = "ID")]
    pub id: String,
    pub enabled: bool,
    #[field(require)]
    pub prerequisites: IndexSet<String>,
}

pub struct LobbyPrerequisiteCheckbox {
    info: Rc<LobbyPrerequisiteCheckboxInfo>,
    enabled: Cell<bool>,
}

register_traits!(LobbyPrerequisiteCheckbox: INotifyCreated, ITechTreePrerequisite);
info_impl!(LobbyPrerequisiteCheckboxInfo => |info, _| Some(Rc::new(LobbyPrerequisiteCheckbox { info, enabled: Cell::new(false) })));

impl INotifyCreated for LobbyPrerequisiteCheckbox {
    fn created(&self, self_: &ActorRef) {
        self.enabled.set(self_.world().lobby_info.global_settings.option_bool(&self.info.id, self.info.enabled));
    }
}

impl ITechTreePrerequisite for LobbyPrerequisiteCheckbox {
    fn provides_prerequisites(&self) -> Vec<String> {
        if self.enabled.get() { self.info.prerequisites.iter().cloned().collect() } else { Vec::new() }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GrantConditionOnPrerequisiteInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub condition: String,
    #[field(require)]
    pub prerequisites: Vec<String>,
}

pub struct GrantConditionOnPrerequisite {
    info: Rc<GrantConditionOnPrerequisiteInfo>,
    me: Weak<GrantConditionOnPrerequisite>,
    was_available: Cell<bool>,
    global_manager: RefCell<Option<Rc<GrantConditionOnPrerequisiteManager>>>,
    token: Cell<i32>,
}

register_traits!(GrantConditionOnPrerequisite: INotifyCreated, INotifyAddedToWorld, INotifyRemovedFromWorld, INotifyOwnerChanged);
info_impl!(GrantConditionOnPrerequisiteInfo => |info, _| Some(Rc::new_cyclic(|me| GrantConditionOnPrerequisite {
    info,
    me: me.clone(),
    was_available: Cell::new(false),
    global_manager: RefCell::new(None),
    token: Cell::new(INVALID_CONDITION_TOKEN),
})));

impl INotifyCreated for GrantConditionOnPrerequisite {
    fn created(&self, self_: &ActorRef) {
        *self.global_manager.borrow_mut() = Some(self_.owner().player_actor().trait_::<GrantConditionOnPrerequisiteManager>());
    }
}

impl INotifyAddedToWorld for GrantConditionOnPrerequisite {
    fn added_to_world(&self, self_: &ActorRef) {
        if !self.info.prerequisites.is_empty() {
            let m = self.global_manager.borrow().clone().expect("created");
            m.register_upgradable(self_, self.me.upgrade().expect("trait alive"), &self.info.prerequisites);
        }
    }
}

impl INotifyRemovedFromWorld for GrantConditionOnPrerequisite {
    fn removed_from_world(&self, self_: &ActorRef) {
        if !self.info.prerequisites.is_empty() {
            let m = self.global_manager.borrow().clone().expect("created");
            m.unregister(self_, self, &self.info.prerequisites);
        }
    }
}

impl INotifyOwnerChanged for GrantConditionOnPrerequisite {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        *self.global_manager.borrow_mut() = Some(new_owner.player_actor().trait_::<GrantConditionOnPrerequisiteManager>());
    }
}

impl GrantConditionOnPrerequisite {
    fn prerequisites_updated(&self, self_: &ActorRef, available: bool) {
        if available == self.was_available.get() {
            return;
        }
        if available && self.token.get() == INVALID_CONDITION_TOKEN {
            self.token.set(self_.grant_condition(&self.info.condition));
        } else if !available && self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
        self.was_available.set(available);
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GrantConditionOnPrerequisiteManagerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct GrantConditionOnPrerequisiteManager {
    me: Weak<GrantConditionOnPrerequisiteManager>,
    upgradables: RefCell<HashMap<String, Vec<(ActorRef, Rc<GrantConditionOnPrerequisite>)>>>,
    tech_tree: Rc<TechTree>,
}

register_traits!(GrantConditionOnPrerequisiteManager);
info_impl!(GrantConditionOnPrerequisiteManagerInfo => |_, init| Some(Rc::new_cyclic(|me| GrantConditionOnPrerequisiteManager {
    me: me.clone(),
    upgradables: RefCell::new(HashMap::new()),
    tech_tree: init.self_.trait_::<TechTree>(),
})));

fn make_key(prerequisites: &[String]) -> String {
    let mut v: Vec<&str> = prerequisites.iter().map(String::as_str).collect();
    v.sort_unstable();
    format!("condition_{}", v.join("_"))
}

impl GrantConditionOnPrerequisiteManager {
    fn register_upgradable(&self, actor: &ActorRef, u: Rc<GrantConditionOnPrerequisite>, prerequisites: &[String]) {
        let key = make_key(prerequisites);
        let is_new = !self.upgradables.borrow().contains_key(&key);
        if is_new {
            self.upgradables.borrow_mut().insert(key.clone(), Vec::new());
            let me: Rc<dyn ITechTreeElement> = self.me.upgrade().expect("trait alive");
            self.tech_tree.add(&key, prerequisites.to_vec(), 0, me);
        }
        self.upgradables.borrow_mut().get_mut(&key).unwrap().push((actor.clone(), u.clone()));
        u.prerequisites_updated(actor, self.tech_tree.has_prerequisites(prerequisites));
    }

    fn unregister(&self, actor: &ActorRef, u: &GrantConditionOnPrerequisite, prerequisites: &[String]) {
        let key = make_key(prerequisites);
        let mut m = self.upgradables.borrow_mut();
        let list = m.get_mut(&key).expect("registered");
        list.retain(|x| !(x.0.id == actor.id && std::ptr::eq(Rc::as_ptr(&x.1), u)));
        if list.is_empty() {
            m.remove(&key);
            self.tech_tree.remove_key(&key);
        }
    }

    fn notify(&self, key: &str, available: bool) {
        let list = self.upgradables.borrow().get(key).cloned();
        if let Some(list) = list {
            for (a, u) in list {
                u.prerequisites_updated(&a, available);
            }
        }
    }
}

impl ITechTreeElement for GrantConditionOnPrerequisiteManager {
    fn prerequisites_available(&self, key: &str) {
        self.notify(key, true);
    }
    fn prerequisites_unavailable(&self, key: &str) {
        self.notify(key, false);
    }
    fn prerequisites_item_hidden(&self, _key: &str) {}
    fn prerequisites_item_visible(&self, _key: &str) {}
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PrimaryBuildingInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub primary_condition: Option<String>,
    pub production_queues: Vec<String>,
}

pub struct PrimaryBuilding {
    info: Rc<PrimaryBuildingInfo>,
    state: ConditionalState,
    token: Cell<i32>,
    is_primary: Cell<bool>,
}

register_traits!(PrimaryBuilding: IResolveOrder, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(PrimaryBuildingInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(PrimaryBuilding { info, state, token: Cell::new(INVALID_CONDITION_TOKEN), is_primary: Cell::new(false) }))
});

impl ConditionalHooks for PrimaryBuilding {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        if self.is_primary.get() {
            self.set_primary_producer(self_, false);
        }
    }
}

conditional_plumbing!(PrimaryBuilding);

impl INotifyCreated for PrimaryBuilding {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl IResolveOrder for PrimaryBuilding {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "PrimaryProducer" {
            self.set_primary_producer(self_, !self.is_primary.get());
        }
        if RallyPoint::is_force_set(order) && !self.is_primary.get() {
            self.set_primary_producer(self_, true);
        }
    }
}

impl PrimaryBuilding {
    pub fn is_primary(&self) -> bool {
        self.is_primary.get()
    }

    pub fn set_primary_producer(&self, self_: &ActorRef, is_primary: bool) {
        self.is_primary.set(is_primary);
        if is_primary {
            let queues: Vec<String> = if self.info.production_queues.is_empty() {
                self_
                    .traits_implementing::<Production>()
                    .iter()
                    .filter(|t| !t.is_trait_disabled())
                    .flat_map(|p| p.info.produces.clone())
                    .collect()
            } else {
                self.info.production_queues.clone()
            };
            let owner = self_.owner();
            for q in queues {
                for (a, t) in self_.world().actors_with_trait::<PrimaryBuilding>() {
                    if a.id != self_.id
                        && Rc::ptr_eq(&a.owner(), &owner)
                        && t.is_primary.get()
                        && a.traits_implementing::<Production>().iter().filter(|p| !p.is_trait_disabled()).any(|p| p.info.produces.contains(&q))
                    {
                        t.set_primary_producer(&a, false);
                    }
                }
            }
            if self.token.get() == INVALID_CONDITION_TOKEN {
                self.token.set(self_.grant_condition(self.info.primary_condition.as_deref().unwrap_or("")));
            }
        } else if self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
    }
}

pub fn is_primary_building(a: &ActorRef) -> bool {
    a.trait_or_default::<PrimaryBuilding>().is_some_and(|p| p.is_primary())
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct RallyPointInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub path: Vec<CVec>,
    pub force_set_type: Option<String>,
}

pub struct RallyPoint {
    pub info: Rc<RallyPointInfo>,
    pub path: RefCell<Vec<CPos>>,
}

register_traits!(RallyPoint: IResolveOrder, INotifyOwnerChanged);
info_impl!(RallyPointInfo => |info, init| {
    let loc = init.self_.location();
    let path = info.path.iter().map(|p| loc + *p).collect();
    Some(Rc::new(RallyPoint { info, path: RefCell::new(path) }))
});

impl RallyPoint {
    pub fn is_force_set(order: &Order) -> bool {
        order.order_string == "SetRallyPoint" && order.extra_data == 1
    }
}

impl INotifyOwnerChanged for RallyPoint {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, _new: &PlayerRef) {
        let loc = self_.location();
        *self.path.borrow_mut() = self.info.path.iter().map(|p| loc + *p).collect();
    }
}

impl IResolveOrder for RallyPoint {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "Stop" {
            self.path.borrow_mut().clear();
            return;
        }
        if order.order_string != "SetRallyPoint" {
            return;
        }
        if !order.target.is_valid_for(self_) {
            return;
        }
        if !order.queued {
            self.path.borrow_mut().clear();
        }
        self.path.borrow_mut().push(self_.world().map().cell_containing(order.target.center_position()));
    }
}

/// `PlaceBuildingInit`.
pub struct PlaceBuildingInit;

impl ActorInit for PlaceBuildingInit {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PlaceBuildingInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct PlaceBuilding;

register_traits!(PlaceBuilding: IResolveOrder);
info_impl!(PlaceBuildingInfo => |_, _| Some(Rc::new(PlaceBuilding)));

impl IResolveOrder for PlaceBuilding {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        let os = order.order_string.as_str();
        if os != "PlaceBuilding" && os != "LineBuild" && os != "PlacePlug" {
            return;
        }
        let (me, order) = (self_.clone(), order.clone());
        self_.world().add_frame_end_task(Box::new(move |w: &Rc<World>| place_building(&me, &order, w)));
    }
}

fn place_building(self_: &ActorRef, order: &Order, w: &Rc<World>) {
    let os = order.order_string.as_str();
    let target_actor = w.get_actor_by_id(order.extra_data);
    let target_location = w.map().cell_containing(order.target.center_position());
    let Some(target_actor) = target_actor.filter(|a| !a.is_dead()) else { return };
    let name = order.target_string.clone().unwrap_or_default();
    let actor_info = w.rules().actor(&name).unwrap_or_else(|| panic!("unknown actor {name}"));
    let done_item = |q: &ProductionQueue| q.all_queued().into_iter().find(|i| i.done.get() && i.item == name);
    let Some(queue) = target_actor.traits_implementing::<ProductionQueue>().into_iter().find(|q| q.can_build(&name) && done_item(q).is_some()) else {
        return;
    };
    let Some(item) = done_item(&queue) else { return };
    let producer = queue.most_likely_producer();
    let mut faction = producer.as_ref().map_or_else(|| self_.owner().faction.clone(), |p| p.1.faction());
    let building_info = actor_info.trait_info::<BuildingInfo>().expect("Building");
    if let Some(f) = actor_info.trait_info::<BuildableInfo>().and_then(|b| b.force_faction.clone()) {
        faction = f;
    }
    let player = order.player().expect("order player");
    let new_inits = |location: CPos| {
        ActorInits::new()
            .with(LocationInit::new(location))
            .with(OwnerInit::from_player(&player))
            .with(FactionInit::new(faction.clone()))
    };
    if os == "LineBuild" {
        let placed = w.create_actor(&actor_info.name, new_inits(target_location).with(PlaceBuildingInit));
        let line = actor_info.trait_info::<LineBuildInfo>().expect("LineBuild");
        let segment_type = line.segment_type.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| actor_info.name.clone());
        for (cell, actor) in get_line_build_cells(w, target_location, &actor_info, &building_info, &player) {
            if cell == target_location {
                continue;
            }
            let dir = if cell.x() == target_location.x() { LineBuildDirection::Y } else { LineBuildDirection::X };
            w.create_actor(
                &segment_type,
                new_inits(cell)
                    .with(LineBuildDirectionInit::new(dir))
                    .with(LineBuildParentInit {
                        actors: Some(vec![actor, Some(placed.clone())]),
                        names: Vec::new(),
                    })
                    .with(PlaceBuildingInit),
            );
        }
    } else if os == "PlacePlug" {
        return;
    } else {
        if !can_place_building(w, target_location, &building_info, None) || !is_close_enough_to_base(&building_info, w, &player, &actor_info, target_location) {
            return;
        }
        w.create_actor(&actor_info.name, new_inits(target_location).with(PlaceBuildingInit));
    }
    if let Some((a, _)) = &producer {
        for n in a.traits_implementing::<dyn INotifyBuildingPlaced>() {
            n.building_placed(a);
        }
    }
    queue.end_production(&item);
    if building_info.requires_base_provider {
        if let Some(bp) = find_base_provider(&building_info, w, &self_.owner(), target_location) {
            bp.begin_cooldown();
        }
    }
}
