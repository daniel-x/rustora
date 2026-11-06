// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Player/ModularBot.cs`, the bot module interfaces, `ActorIndex.cs` and `AIUtils.cs`.
//! Bots only act through orders and draw their random numbers from `World::bot_random`.

pub mod base_builder;
pub mod fuzzy;
pub mod modules;
pub mod save_data;
pub mod squads;
pub mod support_power;
pub mod unit_builder;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_core::world_types::{CPos, MersenneTwister};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::damage::AttackInfo;
use rustora_engine::net_collections::NetMap;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::interfaces::INotifyDamage;
use crate::production::ProductionQueue;

pub fn register(r: &mut TraitRegistry) {
    r.register("ModularBot", |n| load_info!(ModularBotInfo, n));
    r.register("DummyBot", |n| load_info!(DummyBotInfo, n));
    modules::register(r);
    base_builder::register(r);
    unit_builder::register(r);
    squads::register(r);
    support_power::register(r);
}

pub trait IBotEnabled {
    fn bot_enabled(&self, bot: &Rc<ModularBot>);
}

/// `IBotTick`; `bot_module_enabled` stands for `IsTraitEnabled()` of the implementing conditional trait.
pub trait IBotTick {
    fn bot_tick(&self, bot: &ModularBot);
    fn bot_module_enabled(&self) -> bool;
}

pub trait IBotRespondToAttack {
    fn respond_to_attack(&self, bot: &ModularBot, self_: &ActorRef, e: &AttackInfo);
    fn bot_module_enabled(&self) -> bool;
}

pub trait IBotPositionsUpdated {
    fn updated_base_center(&self, location: CPos);
    fn updated_defense_center(&self, location: CPos);
}

pub trait IBotNotifyIdleBaseUnits {
    fn updated_idle_base_units(&self, idle_units: &[ActorRef]);
}

pub trait IBotRequestUnitProduction {
    fn request_unit_production(&self, bot: &ModularBot, requested_actor: &str);
    fn requested_production_count(&self, bot: &ModularBot, requested_actor: &str) -> i32;
    fn bot_module_enabled(&self) -> bool;
}

pub trait IBotRequestPauseUnitProduction {
    fn pause_unit_production(&self) -> bool;
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ModularBotInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    #[field(name = "Type")]
    pub ty: String,
    pub name: Option<String>,
    pub min_order_quotient_per_tick: i32,
}

impl Default for ModularBotInfo {
    fn default() -> Self {
        ModularBotInfo {
            instance_name: None,
            ty: String::new(),
            name: None,
            min_order_quotient_per_tick: 5,
        }
    }
}

/// `ModularBot`: ticks the bot modules of an activated bot and issues their queued orders.
pub struct ModularBot {
    pub info: Rc<ModularBotInfo>,
    enabled: Cell<bool>,
    orders: RefCell<VecDeque<Order>>,
    player: RefCell<Option<PlayerRef>>,
    tick_modules: RefCell<Vec<Rc<dyn IBotTick>>>,
    attack_response_modules: RefCell<Vec<Rc<dyn IBotRespondToAttack>>>,
}

register_traits!(ModularBot: ITick, INotifyDamage);
info_impl!(ModularBotInfo => |info, _| Some(Rc::new(ModularBot {
    info,
    enabled: Cell::new(false),
    orders: RefCell::new(VecDeque::new()),
    player: RefCell::new(None),
    tick_modules: RefCell::new(Vec::new()),
    attack_response_modules: RefCell::new(Vec::new()),
})));

impl ModularBot {
    /// `ModularBot.Activate`, without the replay check: the replay verifier runs bots while replaying.
    pub fn activate(self: &Rc<Self>, p: &PlayerRef) {
        self.enabled.set(true);
        *self.player.borrow_mut() = Some(p.clone());
        let pa = p.player_actor();
        *self.tick_modules.borrow_mut() = pa.traits_implementing::<dyn IBotTick>();
        *self.attack_response_modules.borrow_mut() = pa.traits_implementing::<dyn IBotRespondToAttack>();
        for m in pa.traits_implementing::<dyn IBotEnabled>() {
            m.bot_enabled(self);
        }
    }

    pub fn queue_order(&self, order: Order) {
        self.orders.borrow_mut().push_back(order);
    }

    pub fn player(&self) -> PlayerRef {
        self.player.borrow().clone().expect("activated bot")
    }
}

/// Activates the bot logic of all bot players, like `Player` does on the host.
/// `DummyBot`: a placeholder bot that does nothing, so it needs no instance.
#[derive(Clone, Debug, FieldLoad)]
pub struct DummyBotInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub name: String,
    #[field(name = "Type")]
    pub ty: String,
}

impl Default for DummyBotInfo {
    fn default() -> Self {
        DummyBotInfo {
            instance_name: None,
            name: "Unnamed Bot".into(),
            ty: String::new(),
        }
    }
}

info_impl!(DummyBotInfo => |_info, _| None);

pub fn activate_bots(world: &World) {
    for p in world.players() {
        let Some(ty) = p.bot_type.clone() else { continue };
        if let Some(bot) = p
            .player_actor()
            .traits_implementing::<ModularBot>()
            .into_iter()
            .find(|b| b.info.ty == ty)
        {
            bot.activate(&p);
        }
    }
}

impl ITick for ModularBot {
    fn tick(&self, self_: &ActorRef) {
        if !self.enabled.get() || self_.world().is_loading_game_save() {
            return;
        }
        let modules = self.tick_modules.borrow().clone();
        for m in modules {
            if m.bot_module_enabled() {
                m.bot_tick(self);
            }
        }
        let q = self.info.min_order_quotient_per_tick;
        let count = self.orders.borrow().len() as i32;
        let n = ((count + q - 1) / q).min(count);
        let world = self_.world();
        for _ in 0..n {
            let o = self.orders.borrow_mut().pop_front().expect("queued order");
            world.issue_order(o);
        }
    }
}

impl INotifyDamage for ModularBot {
    fn damaged(&self, self_: &ActorRef, e: &AttackInfo) {
        if !self.enabled.get() || self_.world().is_loading_game_save() {
            return;
        }
        let modules = self.attack_response_modules.borrow().clone();
        for m in modules {
            if m.bot_module_enabled() {
                m.respond_to_attack(self, self_, e);
            }
        }
    }
}

/// `ActorIndex`: actors matching a filter, kept up to date by the world's actor added and removed events.
/// Iterates like the C# `HashSet<Actor>` (actor hash codes are their ids).
pub struct ActorIndex {
    actors: Rc<RefCell<NetMap<u32, ActorRef>>>,
}

impl ActorIndex {
    fn new(world: &World, filter: impl Fn(&ActorRef) -> bool + 'static) -> ActorIndex {
        let filter = Rc::new(filter);
        let mut set = NetMap::new();
        for a in world.actors() {
            if filter(&a) {
                set.insert(a.id, a);
            }
        }
        let actors = Rc::new(RefCell::new(set));
        let (add, f) = (Rc::downgrade(&actors), filter.clone());
        world.on_actor_added(Rc::new(move |a: &ActorRef| {
            if let Some(s) = add.upgrade()
                && f(a)
            {
                s.borrow_mut().insert(a.id, a.clone());
            }
        }));
        let remove = Rc::downgrade(&actors);
        world.on_actor_removed(Rc::new(move |a: &ActorRef| {
            if let Some(s) = remove.upgrade() {
                s.borrow_mut().remove(&a.id);
            }
        }));
        ActorIndex { actors }
    }

    /// `ActorIndex.OwnerAndNames`.
    pub fn owner_and_names(world: &World, names: &IndexSet<String>, owner: &PlayerRef) -> ActorIndex {
        let (names, owner) = (names.clone(), owner.clone());
        ActorIndex::new(world, move |a| {
            a.owner_opt().is_some_and(|o| Rc::ptr_eq(&o, &owner)) && names.contains(&a.info.name)
        })
    }

    /// `ActorIndex.OwnerAndNamesAndTrait<TInfo>`.
    pub fn owner_and_names_and_trait<T: ?Sized + 'static>(world: &World, names: &IndexSet<String>, owner: &PlayerRef) -> ActorIndex {
        let (names, owner) = (names.clone(), owner.clone());
        ActorIndex::new(world, move |a| {
            a.owner_opt().is_some_and(|o| Rc::ptr_eq(&o, &owner)) && names.contains(&a.info.name) && a.info.has_trait_info::<T>()
        })
    }

    /// `ActorIndex.NamesAndTrait<TInfo>`.
    pub fn names_and_trait<T: ?Sized + 'static>(world: &World, names: &IndexSet<String>) -> ActorIndex {
        let names = names.clone();
        ActorIndex::new(world, move |a| names.contains(&a.info.name) && a.info.has_trait_info::<T>())
    }

    pub fn actors(&self) -> Vec<ActorRef> {
        self.actors.borrow().values().cloned().collect()
    }

    /// `AIUtils.CountActorByCommonName`.
    pub fn count_alive(&self) -> i32 {
        self.actors.borrow().values().filter(|a| !a.is_dead()).count() as i32
    }
}

/// `Exts.RandomOrDefault`: draws only if `items` is not empty.
pub fn random_or_default<T: Clone>(items: &[T], r: &RefCell<MersenneTwister>) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let i = r.borrow_mut().next_max(items.len() as i32);
    Some(items[i as usize].clone())
}

/// `Util.Shuffle` as a lazy iterator: each element draws its random number when it is enumerated.
pub struct Shuffle<'a, T> {
    items: Vec<T>,
    i: usize,
    random: &'a RefCell<MersenneTwister>,
}

impl<T: Clone> Iterator for Shuffle<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        let n = self.items.len();
        if self.i + 1 < n {
            let j = self.random.borrow_mut().next_max((n - self.i) as i32) as usize;
            self.items.swap(self.i, self.i + j);
            self.i += 1;
            return Some(self.items[self.i - 1].clone());
        }
        if self.i + 1 == n {
            self.i += 1;
            return Some(self.items[n - 1].clone());
        }
        None
    }
}

pub fn shuffle<T: Clone>(items: Vec<T>, random: &RefCell<MersenneTwister>) -> Shuffle<'_, T> {
    Shuffle { items, i: 0, random }
}

/// `Exts.MaxByOrDefault`: the first element with the largest key.
pub fn max_by_first<T, K: Ord>(items: impl IntoIterator<Item = T>, key: impl Fn(&T) -> K) -> Option<T> {
    let mut best: Option<(T, K)> = None;
    for t in items {
        let k = key(&t);
        if best.as_ref().is_none_or(|(_, bk)| k > *bk) {
            best = Some((t, k));
        }
    }
    best.map(|(t, _)| t)
}

/// `AIUtils.FindQueuesByCategory`: enabled production queues of `player` grouped by type, in actor order.
pub fn find_queues_by_category(player: &PlayerRef) -> NetMap<String, Vec<Rc<ProductionQueue>>> {
    let mut out: NetMap<String, Vec<Rc<ProductionQueue>>> = NetMap::new();
    for (a, q) in player.player_actor().world().actors_with_trait::<ProductionQueue>() {
        if !Rc::ptr_eq(&a.owner(), player) || !q.enabled() {
            continue;
        }
        let ty = q.info.ty.clone();
        if let Some(v) = out.get_mut(&ty) {
            v.push(q);
        } else {
            out.insert(ty, vec![q]);
        }
    }
    out
}

pub fn queues_of(map: &NetMap<String, Vec<Rc<ProductionQueue>>>, category: &str) -> Vec<Rc<ProductionQueue>> {
    map.get(&category.to_string()).cloned().unwrap_or_default()
}

/// `AIUtils.CountActorsWithNameAndTrait<T>`.
pub fn count_actors_with_name_and_trait<T: ?Sized + 'static>(name: &str, owner: &PlayerRef) -> i32 {
    owner
        .player_actor()
        .world()
        .actors_having_trait::<T>()
        .iter()
        .filter(|a| Rc::ptr_eq(&a.owner(), owner) && a.info.name == name)
        .count() as i32
}

/// `IEnumerable<T>.FirstEnabledTraitOrDefault()`.
pub fn first_enabled_unit_production(list: &[Rc<dyn IBotRequestUnitProduction>]) -> Option<Rc<dyn IBotRequestUnitProduction>> {
    list.iter().find(|t| t.bot_module_enabled()).cloned()
}
