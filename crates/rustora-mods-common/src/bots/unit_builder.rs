// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `UnitBuilderBotModule.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::conditional::*;
use rustora_engine::net_collections::NetMap;
use rustora_engine::order::Order;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::Target;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

use super::save_data::{node_value, parse_list};
use super::*;
use crate::aircraft::AircraftInfo;
use crate::building::Building;
use crate::interfaces::IPositionable;
use crate::player::PlayerResources;
use crate::production::{BuildableInfo, ProductionQueue};
use crate::resupply::RearmableInfo;

pub fn register(r: &mut TraitRegistry) {
    r.register("UnitBuilderBotModule", |n| load_info!(UnitBuilderBotModuleInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct UnitBuilderBotModuleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub idle_base_units_maximum: i32,
    pub unit_queues: Vec<String>,
    pub units_to_build: IndexMap<String, i32>,
    pub unit_limits: IndexMap<String, i32>,
    pub unit_delays: IndexMap<String, i32>,
    pub production_min_cash_requirement: i32,
}

impl Default for UnitBuilderBotModuleInfo {
    fn default() -> Self {
        UnitBuilderBotModuleInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            idle_base_units_maximum: -1,
            unit_queues: ["Vehicle", "Infantry", "Plane", "Ship", "Aircraft"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            units_to_build: IndexMap::new(),
            unit_limits: IndexMap::new(),
            unit_delays: IndexMap::new(),
            production_min_cash_requirement: 500,
        }
    }
}

const FEEDBACK_TIME: i32 = 30;

pub struct UnitBuilderBotModule {
    info: Rc<UnitBuilderBotModuleInfo>,
    state: ConditionalState,
    self_: ActorRef,
    queued_build_requests: RefCell<Vec<String>>,
    units_to_build: ActorIndex,
    request_pause: OnceCell<Vec<Rc<dyn IBotRequestPauseUnitProduction>>>,
    player_resources: OnceCell<Rc<PlayerResources>>,
    idle_unit_count: Cell<i32>,
    current_queue_index: Cell<usize>,
    ticks: Cell<i32>,
}

register_traits!(UnitBuilderBotModule: IGameSaveTraitData, IBotTick, IBotNotifyIdleBaseUnits, IBotRequestUnitProduction, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(UnitBuilderBotModuleInfo => |info, init| {
    let names: IndexSet<String> = info.units_to_build.keys().cloned().collect();
    Some(Rc::new(UnitBuilderBotModule {
        state: ConditionalState::new(&info.base),
        self_: init.self_.clone(),
        queued_build_requests: RefCell::new(Vec::new()),
        units_to_build: ActorIndex::owner_and_names(&init.world(), &names, &init.self_.owner()),
        request_pause: OnceCell::new(),
        player_resources: OnceCell::new(),
        idle_unit_count: Cell::new(0),
        current_queue_index: Cell::new(0),
        ticks: Cell::new(0),
        info,
    }))
});

impl ConditionalHooks for UnitBuilderBotModule {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}
rustora_engine::conditional_plumbing!(UnitBuilderBotModule);

impl IGameSaveTraitData for UnitBuilderBotModule {
    fn issue_trait_data(&self, _actor: &ActorRef) -> Option<Vec<MiniYamlNode>> {
        if self.state.is_trait_disabled() {
            return None;
        }
        Some(vec![
            MiniYamlNode::with_value_str("QueuedBuildRequests", self.queued_build_requests.borrow().join(", ")),
            MiniYamlNode::with_value_str("IdleUnitCount", self.idle_unit_count.get().to_string()),
        ])
    }

    fn resolve_trait_data(&self, self_: &ActorRef, data: &MiniYaml) {
        if self_.world().is_replay() {
            return;
        }
        if let Some(v) = node_value(data, "QueuedBuildRequests") {
            *self.queued_build_requests.borrow_mut() = parse_list(v);
        }
        if let Some(v) = node_value(data, "IdleUnitCount").and_then(|v| v.parse().ok()) {
            self.idle_unit_count.set(v);
        }
    }
}

impl INotifyCreated for UnitBuilderBotModule {
    fn created(&self, self_: &ActorRef) {
        let pa = self_.owner().player_actor().clone();
        let _ = self
            .request_pause
            .set(pa.traits_implementing::<dyn IBotRequestPauseUnitProduction>());
        let _ = self.player_resources.set(pa.trait_::<PlayerResources>());
    }
}

impl IBotNotifyIdleBaseUnits for UnitBuilderBotModule {
    fn updated_idle_base_units(&self, idle_units: &[ActorRef]) {
        self.idle_unit_count.set(idle_units.len() as i32);
    }
}

impl IBotRequestUnitProduction for UnitBuilderBotModule {
    fn request_unit_production(&self, _bot: &ModularBot, requested_actor: &str) {
        self.queued_build_requests.borrow_mut().push(requested_actor.to_string());
    }

    fn requested_production_count(&self, _bot: &ModularBot, requested_actor: &str) -> i32 {
        self.queued_build_requests.borrow().iter().filter(|r| *r == requested_actor).count() as i32
    }

    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }
}

fn start_production(queue: &ProductionQueue, item: &str) -> Order {
    let mut o = Order::new("StartProduction", Some(queue.actor.clone()), Target::Invalid, true);
    o.extra_data = 1;
    o.target_string = Some(item.to_string());
    o
}

impl IBotTick for UnitBuilderBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn bot_tick(&self, bot: &ModularBot) {
        let pr = self.player_resources.get().expect("created");
        if pr.get_cash_and_resources() < self.info.production_min_cash_requirement
            || self.request_pause.get().expect("created").iter().any(|r| r.pause_unit_production())
        {
            return;
        }
        self.ticks.set(self.ticks.get() + 1);
        if self.ticks.get() % FEEDBACK_TIME != 0 {
            return;
        }
        let player = self.self_.owner();
        let mut queues_by_category: Option<NetMap<String, Vec<Rc<ProductionQueue>>>> = None;
        let request = self.queued_build_requests.borrow().first().cloned();
        if let Some(r) = request {
            let q = queues_by_category.get_or_insert_with(|| find_queues_by_category(&player));
            self.build_unit(bot, &r, q);
            let mut reqs = self.queued_build_requests.borrow_mut();
            if let Some(i) = reqs.iter().position(|x| *x == r) {
                reqs.remove(i);
            }
        }
        let max = self.info.idle_base_units_maximum;
        if max <= 0 || max > self.idle_unit_count.get() {
            let q = queues_by_category.get_or_insert_with(|| find_queues_by_category(&player));
            let n = self.info.unit_queues.len();
            for _ in 0..n {
                let mut i = self.current_queue_index.get() + 1;
                if i >= n {
                    i = 0;
                }
                self.current_queue_index.set(i);
                let queues = queues_of(q, &self.info.unit_queues[i]);
                if !queues.is_empty() {
                    self.build_random_unit(bot, &queues);
                    break;
                }
            }
        }
    }
}

impl UnitBuilderBotModule {
    fn build_random_unit(&self, bot: &ModularBot, queues: &[Rc<ProductionQueue>]) {
        if self.info.units_to_build.is_empty() {
            return;
        }
        let Some(queue) = queues.iter().find(|q| q.all_queued().is_empty()) else {
            return;
        };
        let Some(unit) = self.choose_random_unit_to_build(queue) else {
            return;
        };
        bot.queue_order(start_production(queue, &unit.name));
    }

    fn build_unit(&self, bot: &ModularBot, name: &str, queues_by_category: &NetMap<String, Vec<Rc<ProductionQueue>>>) {
        let w = self.self_.world();
        let Some(ai) = w.rules().actor(name) else { return };
        let Some(bi) = ai.trait_info::<BuildableInfo>() else { return };
        for pq in &bi.queue {
            if let Some(queue) = queues_of(queues_by_category, pq).into_iter().find(|q| q.all_queued().is_empty()) {
                bot.queue_order(start_production(&queue, name));
                return;
            }
        }
    }

    fn choose_random_unit_to_build(&self, queue: &ProductionQueue) -> Option<Rc<ActorInfo>> {
        let w = self.self_.world();
        let items: Vec<Rc<ActorInfo>> = queue.buildable_items().iter().filter_map(|n| w.rules().actor(n)).collect();
        let buildable: Vec<Rc<ActorInfo>> = shuffle(items, &w.bot_random).collect();
        if buildable.is_empty() {
            return None;
        }
        let all_units: Vec<ActorRef> = self.units_to_build.actors().into_iter().filter(|a| !a.is_dead()).collect();
        let mut desired: Option<Rc<ActorInfo>> = None;
        let mut desired_error = i32::MAX;
        let info = &self.info;
        for unit in buildable {
            let Some(&share) = info.units_to_build.get(&unit.name) else {
                continue;
            };
            if info.unit_delays.get(&unit.name).is_some_and(|&d| d > w.world_tick()) {
                continue;
            }
            let unit_count = all_units.iter().filter(|a| a.info.name == unit.name).count() as i32;
            if info.unit_limits.get(&unit.name).is_some_and(|&c| unit_count >= c) {
                continue;
            }
            let error = if all_units.is_empty() {
                -1
            } else {
                unit_count * 100 / all_units.len() as i32 - share
            };
            if error < 0 {
                return self.has_adequate_air_unit_reload_buildings(&unit).then_some(unit);
            }
            if error < desired_error {
                desired_error = error;
                desired = Some(unit);
            }
        }
        desired.filter(|d| self.has_adequate_air_unit_reload_buildings(d))
    }

    fn has_adequate_air_unit_reload_buildings(&self, ai: &ActorInfo) -> bool {
        if !ai.has_trait_info::<AircraftInfo>() {
            return true;
        }
        let Some(ri) = ai.trait_info::<RearmableInfo>() else { return true };
        let player = self.self_.owner();
        let own_air = count_actors_with_name_and_trait::<dyn IPositionable>(&ai.name, &player);
        let buildings: i32 = ri
            .rearm_actors
            .iter()
            .map(|b| count_actors_with_name_and_trait::<Building>(b, &player))
            .sum();
        own_air < buildings
    }
}
