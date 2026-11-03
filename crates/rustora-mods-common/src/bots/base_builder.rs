// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `BaseBuilderBotModule.cs` and `BotModuleLogic/BaseBuilderQueueManager.cs`.
//! Plugs and building variants are left out: RA has neither.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use rustora_core::world_types::CPos;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::conditional::*;
use rustora_engine::damage::AttackInfo;
use rustora_engine::net_collections::NetMap;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::Target;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use super::*;
use crate::base::{BaseProvider, GivesBuildableArea, PowerInfo, PowerManager};
use crate::building::{Building, BuildingInfo};
use crate::locomotor::Locomotor;
use crate::mobile::MobileInfo;
use crate::path_finder_trait::PathFinder;
use crate::player::PlayerResources;
use crate::production::{Production, ProductionQueue, RallyPoint};
use crate::resources::ResourceLayer;

pub fn register(r: &mut TraitRegistry) {
    r.register("BaseBuilderBotModule", |n| load_info!(BaseBuilderBotModuleInfo, n));
}

fn set(v: &[&str]) -> IndexSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[derive(Clone, Debug, FieldLoad)]
pub struct BaseBuilderBotModuleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub construction_yard_types: IndexSet<String>,
    pub vehicles_factory_types: IndexSet<String>,
    pub refinery_types: IndexSet<String>,
    pub power_types: IndexSet<String>,
    pub barracks_types: IndexSet<String>,
    pub production_types: IndexSet<String>,
    pub naval_production_types: IndexSet<String>,
    pub silo_types: IndexSet<String>,
    pub defense_types: IndexSet<String>,
    pub building_queues: IndexSet<String>,
    pub defense_queues: IndexSet<String>,
    pub min_base_radius: i32,
    pub max_base_radius: i32,
    pub minimum_excess_power: i32,
    pub maximum_excess_power: i32,
    pub excess_power_increment: i32,
    pub excess_power_increase_threshold: i32,
    pub initital_minimum_refinery_count: i32,
    pub additional_minimum_refinery_count: i32,
    pub structure_production_inactive_delay: i32,
    pub structure_production_active_delay: i32,
    pub structure_production_random_bonus_delay: i32,
    pub structure_production_resume_delay: i32,
    pub maximum_failed_placement_attempts: i32,
    pub max_resource_cells_to_check: i32,
    pub check_for_new_bases_delay: i32,
    pub place_defense_towards_enemy_chance: i32,
    pub minimum_defense_radius: i32,
    pub maximum_defense_radius: i32,
    pub new_production_cash_threshold: i32,
    pub rally_point_scan_radius: i32,
    pub check_for_water_radius: i32,
    pub water_terrain_types: IndexSet<String>,
    pub building_fractions: IndexMap<String, i32>,
    pub building_limits: IndexMap<String, i32>,
    pub building_delays: IndexMap<String, i32>,
    pub production_min_cash_requirement: i32,
    pub assign_rally_points_interval: i32,
}

impl Default for BaseBuilderBotModuleInfo {
    fn default() -> Self {
        BaseBuilderBotModuleInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            construction_yard_types: IndexSet::new(),
            vehicles_factory_types: IndexSet::new(),
            refinery_types: IndexSet::new(),
            power_types: IndexSet::new(),
            barracks_types: IndexSet::new(),
            production_types: IndexSet::new(),
            naval_production_types: IndexSet::new(),
            silo_types: IndexSet::new(),
            defense_types: IndexSet::new(),
            building_queues: set(&["Building"]),
            defense_queues: set(&["Defense"]),
            min_base_radius: 2,
            max_base_radius: 20,
            minimum_excess_power: 0,
            maximum_excess_power: 0,
            excess_power_increment: 0,
            excess_power_increase_threshold: 1,
            initital_minimum_refinery_count: 1,
            additional_minimum_refinery_count: 1,
            structure_production_inactive_delay: 125,
            structure_production_active_delay: 25,
            structure_production_random_bonus_delay: 10,
            structure_production_resume_delay: 1500,
            maximum_failed_placement_attempts: 3,
            max_resource_cells_to_check: 3,
            check_for_new_bases_delay: 1500,
            place_defense_towards_enemy_chance: 100,
            minimum_defense_radius: 5,
            maximum_defense_radius: 20,
            new_production_cash_threshold: 5000,
            rally_point_scan_radius: 8,
            check_for_water_radius: 8,
            water_terrain_types: set(&["Water"]),
            building_fractions: IndexMap::new(),
            building_limits: IndexMap::new(),
            building_delays: IndexMap::new(),
            production_min_cash_requirement: 500,
            assign_rally_points_interval: 100,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BuildingType {
    Building,
    Defense,
    Refinery,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WaterCheck {
    NotChecked,
    EnoughWater,
    NotEnoughWater,
    DontCheck,
}

/// `BaseBuilderQueueManager`.
struct QueueManager {
    category: String,
    wait_ticks: Cell<i32>,
    player_buildings: RefCell<Vec<ActorRef>>,
    fail_count: Cell<i32>,
    fail_retry_ticks: Cell<i32>,
    check_for_bases_ticks: Cell<i32>,
    cached_bases: Cell<i32>,
    cached_buildings: Cell<i32>,
    minimum_excess_power: Cell<i32>,
    item_queued_this_tick: Cell<bool>,
    water_state: Cell<WaterCheck>,
}

pub struct BaseBuilderBotModule {
    pub info: Rc<BaseBuilderBotModuleInfo>,
    state: ConditionalState,
    self_: ActorRef,
    pub defense_center: Cell<CPos>,
    pub buildings_being_produced: RefCell<NetMap<String, i32>>,
    player_power: OnceCell<Option<Rc<PowerManager>>>,
    player_resources: OnceCell<Rc<PlayerResources>>,
    positions_updated_modules: OnceCell<Vec<Rc<dyn IBotPositionsUpdated>>>,
    initial_base_center: Cell<CPos>,
    rally_points: RefCell<Vec<(ActorRef, Rc<RallyPoint>)>>,
    assign_rally_points_ticks: Cell<i32>,
    builders: OnceCell<Vec<QueueManager>>,
    current_builder_index: Cell<usize>,
    refinery_buildings: ActorIndex,
    power_buildings: ActorIndex,
    construction_yard_buildings: ActorIndex,
    barracks_buildings: ActorIndex,
}

register_traits!(BaseBuilderBotModule: IBotTick, IBotPositionsUpdated, IBotRespondToAttack, IBotRequestPauseUnitProduction, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(BaseBuilderBotModuleInfo => |info, init| {
    let (w, p) = (init.world(), init.self_.owner());
    Some(Rc::new(BaseBuilderBotModule {
        state: ConditionalState::new(&info.base),
        self_: init.self_.clone(),
        defense_center: Cell::new(CPos::ZERO),
        buildings_being_produced: RefCell::new(NetMap::new()),
        player_power: OnceCell::new(),
        player_resources: OnceCell::new(),
        positions_updated_modules: OnceCell::new(),
        initial_base_center: Cell::new(CPos::ZERO),
        rally_points: RefCell::new(Vec::new()),
        assign_rally_points_ticks: Cell::new(0),
        builders: OnceCell::new(),
        current_builder_index: Cell::new(0),
        refinery_buildings: ActorIndex::owner_and_names_and_trait::<BuildingInfo>(&w, &info.refinery_types, &p),
        power_buildings: ActorIndex::owner_and_names_and_trait::<BuildingInfo>(&w, &info.power_types, &p),
        construction_yard_buildings: ActorIndex::owner_and_names_and_trait::<BuildingInfo>(&w, &info.construction_yard_types, &p),
        barracks_buildings: ActorIndex::owner_and_names_and_trait::<BuildingInfo>(&w, &info.barracks_types, &p),
        info,
    }))
});

impl ConditionalHooks for BaseBuilderBotModule {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        self.assign_rally_points_ticks.set(
            self_
                .world()
                .bot_random
                .borrow_mut()
                .next_range(0, self.info.assign_rally_points_interval),
        );
    }
}
rustora_engine::conditional_plumbing!(BaseBuilderBotModule);

impl INotifyCreated for BaseBuilderBotModule {
    fn created(&self, self_: &ActorRef) {
        let pa = self_.owner().player_actor().clone();
        let _ = self.player_power.set(pa.trait_or_default::<PowerManager>());
        let _ = self.player_resources.set(pa.trait_::<PlayerResources>());
        let _ = self
            .positions_updated_modules
            .set(pa.traits_implementing::<dyn IBotPositionsUpdated>());
        let info = &self.info;
        let builders = info
            .building_queues
            .iter()
            .chain(info.defense_queues.iter())
            .map(|c| QueueManager {
                category: c.clone(),
                wait_ticks: Cell::new(0),
                player_buildings: RefCell::new(Vec::new()),
                fail_count: Cell::new(0),
                fail_retry_ticks: Cell::new(info.structure_production_resume_delay),
                check_for_bases_ticks: Cell::new(0),
                cached_bases: Cell::new(0),
                cached_buildings: Cell::new(0),
                minimum_excess_power: Cell::new(info.minimum_excess_power),
                item_queued_this_tick: Cell::new(false),
                water_state: Cell::new(if info.naval_production_types.is_empty() {
                    WaterCheck::DontCheck
                } else {
                    WaterCheck::NotChecked
                }),
            })
            .collect();
        let _ = self.builders.set(builders);
    }
}

impl IBotPositionsUpdated for BaseBuilderBotModule {
    fn updated_base_center(&self, location: CPos) {
        self.initial_base_center.set(location);
    }

    fn updated_defense_center(&self, location: CPos) {
        self.defense_center.set(location);
    }
}

impl IBotRequestPauseUnitProduction for BaseBuilderBotModule {
    fn pause_unit_production(&self) -> bool {
        !self.state.is_trait_disabled() && !self.has_adequate_refinery_count()
    }
}

/// `Exts.Clamp`, which does not require `min <= max`.
fn cs_clamp(v: i32, min: i32, max: i32) -> i32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

/// `Exts.IntegerDivisionRoundingAwayFromZero`.
pub fn div_away_from_zero(dividend: i32, divisor: i32) -> i32 {
    let q = dividend / divisor;
    if dividend % divisor == 0 {
        q
    } else if (dividend < 0) == (divisor < 0) {
        q + 1
    } else {
        q - 1
    }
}

impl IBotTick for BaseBuilderBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn bot_tick(&self, bot: &ModularBot) {
        let player = self.self_.owner();
        let w = self.self_.world();
        self.assign_rally_points_ticks.set(self.assign_rally_points_ticks.get() - 1);
        if self.assign_rally_points_ticks.get() <= 0 {
            self.assign_rally_points_ticks.set(self.info.assign_rally_points_interval.max(2));
            for (a, rp) in w.actors_with_trait::<RallyPoint>() {
                if Rc::ptr_eq(&a.owner(), &player) {
                    self.rally_points.borrow_mut().push((a, rp));
                }
            }
        } else {
            let n = div_away_from_zero(self.rally_points.borrow().len() as i32, self.assign_rally_points_ticks.get());
            for _ in 0..n {
                let rp = self.rally_points.borrow_mut().pop().expect("rally point");
                if Rc::ptr_eq(&rp.0.owner(), &player) && !rp.0.disposed() {
                    self.set_rally_point(bot, &rp);
                }
            }
        }

        self.buildings_being_produced.borrow_mut().clear();
        let mut find_queue = false;
        let queues_by_category = find_queues_by_category(&player);
        let builders = self.builders.get().expect("created");
        let mut builder_index = self.current_builder_index.get();
        for _ in 0..builders.len() {
            builder_index += 1;
            if builder_index >= builders.len() {
                builder_index = 0;
            }
            let b = &builders[builder_index];
            b.wait_ticks.set(b.wait_ticks.get() - 1);
            let queues = queues_of(&queues_by_category, &b.category);
            if !queues.is_empty() {
                if !find_queue {
                    self.current_builder_index.set(builder_index);
                    find_queue = true;
                }
                if self.player_resources().get_cash_and_resources() >= self.info.production_min_cash_requirement {
                    for q in &queues {
                        let Some(producing) = q.all_queued().first().cloned() else {
                            continue;
                        };
                        let mut m = self.buildings_being_produced.borrow_mut();
                        let n = m.get(&producing.item).copied().unwrap_or(0);
                        m.insert(producing.item.clone(), n + 1);
                    }
                }
            }
        }
        self.tick_builder(&builders[self.current_builder_index.get()], bot, &queues_by_category);
    }
}

impl IBotRespondToAttack for BaseBuilderBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn respond_to_attack(&self, _bot: &ModularBot, self_: &ActorRef, e: &AttackInfo) {
        let Some(attacker) = &e.attacker else { return };
        if attacker.disposed() || attacker.owner().relationship_with(Some(&self_.owner())) != PlayerRelationship::ENEMY {
            return;
        }
        if !attacker.info.has_trait_info_assignable_to("ITargetableInfo") {
            return;
        }
        if self_.info.has_trait_info::<BuildingInfo>() {
            for n in self.positions_updated_modules.get().expect("created") {
                n.updated_defense_center(attacker.location());
            }
        }
    }
}

fn power_amount(ai: &ActorInfo) -> i32 {
    ai.trait_infos::<PowerInfo>()
        .iter()
        .filter(|p| p.base.enabled_by_default())
        .map(|p| p.amount)
        .sum()
}

impl BaseBuilderBotModule {
    fn player_resources(&self) -> &Rc<PlayerResources> {
        self.player_resources.get().expect("created")
    }

    fn player_power(&self) -> Option<&Rc<PowerManager>> {
        self.player_power.get().expect("created").as_ref()
    }

    pub fn random_base_center(&self) -> CPos {
        random_or_default(&self.construction_yard_buildings.actors(), &self.self_.world().bot_random)
            .map_or(self.initial_base_center.get(), |a| a.location())
    }

    fn set_rally_point(&self, bot: &ModularBot, rp: &(ActorRef, Rc<RallyPoint>)) {
        let first = rp.1.path.borrow().first().copied();
        let needs = match first {
            None => true,
            Some(p) => {
                let locomotors = self.locomotors_for_producibles(&rp.0);
                !self.is_rally_point_valid(rp.0.location(), p, &locomotors, rp.0.info.trait_info::<BuildingInfo>().as_deref())
            }
        };
        if needs {
            let w = self.self_.world();
            let cell = self.choose_rally_location_near(&rp.0);
            bot.queue_order(Order::new(
                "SetRallyPoint",
                Some(rp.0.clone()),
                Target::from_cell(&w, cell, FULL_CELL),
                false,
            ));
        }
    }

    fn choose_rally_location_near(&self, producer: &ActorRef) -> CPos {
        let w = self.self_.world();
        let locomotors = self.locomotors_for_producibles(producer);
        let bi = producer.info.trait_info::<BuildingInfo>();
        let possible: Vec<CPos> = w
            .map()
            .find_tiles_in_circle(producer.location(), self.info.rally_point_scan_radius, false)
            .filter(|&c| self.is_rally_point_valid(producer.location(), c, &locomotors, bi.as_deref()))
            .collect();
        random_or_default(&possible, &w.bot_random).unwrap_or(producer.location())
    }

    fn locomotors_for_producibles(&self, producer: &ActorRef) -> Vec<Rc<Locomotor>> {
        let w = self.self_.world();
        let mut productions = producer.traits_implementing::<Production>();
        if productions.is_empty() {
            productions = w
                .actors_with_trait::<Production>()
                .into_iter()
                .filter(|(a, _)| !Rc::ptr_eq(&a.owner(), &producer.owner()))
                .map(|(_, p)| p)
                .collect();
        }
        let produces: IndexSet<String> = productions.iter().flat_map(|p| p.info.produces.iter().cloned()).collect();
        if produces.is_empty() {
            return Vec::new();
        }
        let mut queues = producer.traits_implementing::<ProductionQueue>();
        if queues.is_empty() {
            queues = producer.owner().player_actor().traits_implementing::<ProductionQueue>();
        }
        let names: IndexSet<String> = queues
            .iter()
            .filter(|q| produces.contains(&q.info.ty))
            .flat_map(|q| q.buildable_items())
            .filter_map(|n| w.rules().actor(&n))
            .filter_map(|ai| ai.trait_info::<MobileInfo>().map(|m| m.locomotor.clone()))
            .collect();
        if names.is_empty() {
            return Vec::new();
        }
        w.world_actor()
            .traits_implementing::<Locomotor>()
            .into_iter()
            .filter(|l| names.contains(&l.info.name))
            .collect()
    }

    fn is_rally_point_valid(&self, producer_location: CPos, rally: CPos, locomotors: &[Rc<Locomotor>], bi: Option<&BuildingInfo>) -> bool {
        let w = self.self_.world();
        let pf = w.world_actor().trait_or_default::<PathFinder>();
        pf.is_none_or(|pf| {
            locomotors
                .iter()
                .all(|l| pf.path_might_exist_for_locomotor_blocked_by_immovable(l, producer_location, rally))
        }) && bi.is_none_or(|bi| crate::base::is_cell_buildable(&w, rally, bi, None))
    }

    pub fn has_adequate_refinery_count(&self) -> bool {
        self.info.refinery_types.is_empty()
            || self.refinery_buildings.count_alive() >= self.minimum_refinery_count()
            || self.power_buildings.count_alive() == 0
            || self.construction_yard_buildings.count_alive() == 0
    }

    fn minimum_refinery_count(&self) -> i32 {
        if self.barracks_buildings.count_alive() > 0 {
            self.info.initital_minimum_refinery_count + self.info.additional_minimum_refinery_count
        } else {
            self.info.initital_minimum_refinery_count
        }
    }

    fn count_owned<T: ?Sized + 'static>(w: &World, player: &PlayerRef) -> i32 {
        w.actors_having_trait::<T>()
            .iter()
            .filter(|a| Rc::ptr_eq(&a.owner(), player))
            .count() as i32
    }

    /// `AIUtils.IsAreaAvailable<T>`.
    fn is_area_available<T: ?Sized + 'static>(&self, radius: i32) -> bool {
        let w = self.self_.world();
        let player = self.self_.owner();
        let map = w.map();
        let types = &self.info.water_terrain_types;
        let ok = |c: CPos| map.contains(c) && types.contains(&map.terrain_type_info(c).ty);
        w.actors_having_trait::<T>()
            .iter()
            .filter(|a| Rc::ptr_eq(&a.owner(), &player))
            .any(|a| {
                map.find_tiles_in_circle(a.location(), radius, false)
                    .filter(|&c| {
                        ok(c)
                            && crate::util::adjacent_cells(&w, &Target::from_cell(&w, c, FULL_CELL))
                                .iter()
                                .all(|&ac| ok(ac))
                    })
                    .count()
                    > 0
            })
    }

    /// `BaseBuilderQueueManager.Tick`.
    fn tick_builder(&self, b: &QueueManager, bot: &ModularBot, queues_by_category: &NetMap<String, Vec<Rc<ProductionQueue>>>) {
        let w = self.self_.world();
        let player = self.self_.owner();
        let info = &self.info;
        if b.fail_count.get() >= info.maximum_failed_placement_attempts {
            b.fail_retry_ticks.set(b.fail_retry_ticks.get() - 1);
            if b.fail_retry_ticks.get() <= 0 {
                let current = Self::count_owned::<Building>(&w, &player);
                let providers = Self::count_owned::<BaseProvider>(&w, &player);
                if current < b.cached_buildings.get() || providers > b.cached_bases.get() {
                    b.fail_count.set(0);
                } else {
                    b.fail_retry_ticks.set(info.structure_production_resume_delay);
                }
            }
        }
        if b.water_state.get() == WaterCheck::NotChecked {
            if self.is_area_available::<BaseProvider>(info.max_base_radius) {
                b.water_state.set(WaterCheck::EnoughWater);
            } else {
                b.water_state.set(WaterCheck::NotEnoughWater);
                b.check_for_bases_ticks.set(info.check_for_new_bases_delay);
            }
        }
        if b.water_state.get() == WaterCheck::NotEnoughWater {
            b.check_for_bases_ticks.set(b.check_for_bases_ticks.get() - 1);
            if b.check_for_bases_ticks.get() <= 0 {
                let current = Self::count_owned::<BaseProvider>(&w, &player);
                if current > b.cached_bases.get() {
                    b.cached_bases.set(current);
                    b.water_state.set(WaterCheck::NotChecked);
                }
            }
        }
        if b.wait_ticks.get() > 0 {
            return;
        }
        let buildings: Vec<ActorRef> = w
            .actors_having_trait::<Building>()
            .into_iter()
            .filter(|a| Rc::ptr_eq(&a.owner(), &player))
            .collect();
        let bonus = info.excess_power_increment * (buildings.len() as i32 / info.excess_power_increase_threshold.max(1));
        *b.player_buildings.borrow_mut() = buildings;
        b.minimum_excess_power.set(cs_clamp(
            info.minimum_excess_power + bonus,
            info.minimum_excess_power,
            info.maximum_excess_power,
        ));
        b.item_queued_this_tick.set(false);
        let mut active = false;
        for q in queues_of(queues_by_category, &b.category) {
            if self.tick_queue(b, bot, &q) {
                active = true;
            }
        }
        let random_factor = w
            .bot_random
            .borrow_mut()
            .next_range(0, info.structure_production_random_bonus_delay);
        b.wait_ticks.set(
            if active {
                info.structure_production_active_delay
            } else {
                info.structure_production_inactive_delay
            } + random_factor,
        );
    }

    fn tick_queue(&self, b: &QueueManager, bot: &ModularBot, queue: &Rc<ProductionQueue>) -> bool {
        let w = self.self_.world();
        let player = self.self_.owner();
        let info = &self.info;
        let current = queue.all_queued().first().cloned();
        match current {
            None if b.fail_count.get() < info.maximum_failed_placement_attempts => {
                if self.player_resources().get_cash_and_resources() < info.production_min_cash_requirement || b.item_queued_this_tick.get()
                {
                    return false;
                }
                let Some(item) = self.choose_building_to_build(b, queue) else {
                    return false;
                };
                let mut o = Order::new("StartProduction", Some(queue.actor.clone()), Target::Invalid, true);
                o.extra_data = 1;
                o.target_string = Some(item.name.clone());
                bot.queue_order(o);
                b.item_queued_this_tick.set(true);
            }
            Some(item) if item.done.get() => {
                let ai = w.rules().actor(&item.item).expect("produced actor");
                let mut ty = BuildingType::Building;
                if info.defense_types.contains(&ai.name)
                    && w.bot_random.borrow_mut().next_max(100) < info.place_defense_towards_enemy_chance
                {
                    ty = BuildingType::Defense;
                } else if info.refinery_types.contains(&ai.name) {
                    ty = BuildingType::Refinery;
                }
                match self.choose_build_location(&item.item, true, ty) {
                    None => {
                        let mut o = Order::new("CancelProduction", Some(queue.actor.clone()), Target::Invalid, false);
                        o.extra_data = 1;
                        o.target_string = Some(item.item.clone());
                        bot.queue_order(o);
                        b.fail_count.set(b.fail_count.get() + b.fail_count.get());
                        if b.fail_count.get() == info.maximum_failed_placement_attempts {
                            b.cached_buildings.set(Self::count_owned::<Building>(&w, &player));
                            b.cached_bases.set(Self::count_owned::<BaseProvider>(&w, &player));
                        }
                    }
                    Some(location) => {
                        b.fail_count.set(0);
                        let mut o = Order::new(
                            "PlaceBuilding",
                            Some(player.player_actor().clone()),
                            Target::from_cell(&w, location, FULL_CELL),
                            false,
                        );
                        o.target_string = Some(item.item.clone());
                        o.extra_location = CPos::new(0, 0);
                        o.extra_data = queue.actor.id;
                        bot.queue_order(o);
                        return true;
                    }
                }
            }
            _ => {}
        }
        true
    }

    fn producible_building(
        &self,
        b: &QueueManager,
        actors: &IndexSet<String>,
        buildables: &[Rc<ActorInfo>],
        by_power: bool,
    ) -> Option<Rc<ActorInfo>> {
        let pb = b.player_buildings.borrow();
        let available: Vec<Rc<ActorInfo>> = buildables
            .iter()
            .filter(|a| {
                actors.contains(&a.name)
                    && self
                        .info
                        .building_limits
                        .get(&a.name)
                        .is_none_or(|&l| (pb.iter().filter(|x| x.info.name == a.name).count() as i32) < l)
            })
            .cloned()
            .collect();
        if by_power {
            return max_by_first(available, |a| power_amount(a));
        }
        random_or_default(&available, &self.self_.world().bot_random)
    }

    fn has_sufficient_power_for_actor(&self, ai: &ActorInfo) -> bool {
        self.player_power()
            .is_none_or(|pp| power_amount(ai) + pp.excess_power() >= self.info.minimum_excess_power)
    }

    /// The priority overrides of `ChooseBuildingToBuild` for one building class.
    fn priority(
        &self,
        b: &QueueManager,
        types: &IndexSet<String>,
        buildables: &[Rc<ActorInfo>],
        power: &Option<Rc<ActorInfo>>,
    ) -> Option<Rc<ActorInfo>> {
        let candidate = self.producible_building(b, types, buildables, false);
        if let Some(c) = &candidate {
            if self.has_sufficient_power_for_actor(c) {
                return candidate;
            }
            if power.is_some() {
                return power.clone();
            }
        }
        None
    }

    fn choose_building_to_build(&self, b: &QueueManager, queue: &Rc<ProductionQueue>) -> Option<Rc<ActorInfo>> {
        let w = self.self_.world();
        let info = &self.info;
        let buildables: Vec<Rc<ActorInfo>> = queue.buildable_items().iter().filter_map(|n| w.rules().actor(n)).collect();
        let power = self.producible_building(b, &info.power_types, &buildables, true);
        if let (Some(pp), Some(p)) = (self.player_power(), &power) {
            if pp.excess_power() < b.minimum_excess_power.get() && power_amount(p) > 0 {
                return power;
            }
        }
        if !self.has_adequate_refinery_count() {
            if let Some(r) = self.priority(b, &info.refinery_types, &buildables, &power) {
                return Some(r);
            }
        }
        let cash = self.player_resources().get_cash_and_resources();
        if info.new_production_cash_threshold > 0 && cash > info.new_production_cash_threshold {
            if let Some(r) = self.priority(b, &info.production_types, &buildables, &power) {
                return Some(r);
            }
        }
        if b.water_state.get() == WaterCheck::EnoughWater
            && info.new_production_cash_threshold > 0
            && cash > info.new_production_cash_threshold
            && self.is_area_available::<GivesBuildableArea>(info.check_for_water_radius)
        {
            if let Some(r) = self.priority(b, &info.naval_production_types, &buildables, &power) {
                return Some(r);
            }
        }
        let pr = self.player_resources();
        if pr.resources.get() as f64 > 0.8 * pr.resource_capacity.get() as f64 {
            if let Some(r) = self.priority(b, &info.silo_types, &buildables, &power) {
                return Some(r);
            }
        }
        let fractions: Vec<(String, i32)> = info.building_fractions.iter().map(|(k, v)| (k.clone(), *v)).collect();
        let pb = b.player_buildings.borrow().clone();
        for (name, frac) in shuffle(fractions, &w.bot_random) {
            if info.building_delays.get(&name).is_some_and(|&d| d > w.world_tick()) {
                continue;
            }
            if !buildables.iter().any(|x| x.name == name) {
                continue;
            }
            let count = pb.iter().filter(|a| a.info.name == name).count() as i32
                + self.buildings_being_produced.borrow().get(&name).copied().unwrap_or(0);
            if count * 100 > frac * pb.len() as i32 {
                continue;
            }
            if info.building_limits.get(&name).is_some_and(|&l| l <= count) {
                continue;
            }
            if info.naval_production_types.contains(&name)
                && (b.water_state.get() == WaterCheck::NotEnoughWater
                    || !self.is_area_available::<GivesBuildableArea>(info.check_for_water_radius))
            {
                continue;
            }
            let actor = w.rules().actor(&name).expect("building");
            if let Some(pp) = self.player_power() {
                if (pp.excess_power() < b.minimum_excess_power.get() || !self.has_sufficient_power_for_actor(&actor))
                    && power.as_ref().is_some_and(|p| power_amount(p) > 0)
                {
                    return power;
                }
            }
            return Some(actor);
        }
        None
    }

    fn choose_build_location(&self, actor_type: &str, distance_important: bool, ty: BuildingType) -> Option<CPos> {
        let w = self.self_.world();
        let player = self.self_.owner();
        let info = &self.info;
        let ai = w.rules().actor(actor_type)?;
        let bi = ai.trait_info::<BuildingInfo>()?;
        let find_pos = |center: CPos, target: CPos, min: i32, max: i32| -> Option<CPos> {
            let mut cells: Vec<CPos> = w.map().find_tiles_in_annulus(center, min, max, false).collect();
            let fits = |c: &CPos| {
                crate::base::can_place_building(&w, *c, &bi, None)
                    && (!distance_important || crate::base::is_close_enough_to_base(&bi, &w, &player, &ai, *c))
            };
            if center != target {
                cells.sort_by_key(|&c| (c - target).length_squared());
                cells.into_iter().find(fits)
            } else {
                shuffle(cells, &w.bot_random).find(fits)
            }
        };
        let base_center = self.random_base_center();
        match ty {
            BuildingType::Defense => {
                let dc = w.map().center_of_cell(self.defense_center.get());
                let closest = w
                    .actors_having_trait::<Building>()
                    .into_iter()
                    .filter(|a| !a.disposed() && player.relationship_with(Some(&a.owner())) == PlayerRelationship::ENEMY)
                    .min_by_key(|a| (a.center_position() - dc).length_squared());
                let target = closest.map_or(base_center, |a| a.location());
                find_pos(
                    self.defense_center.get(),
                    target,
                    info.minimum_defense_radius,
                    info.maximum_defense_radius,
                )
            }
            BuildingType::Refinery => {
                if let Some(layer) = w.world_actor().trait_or_default::<ResourceLayer>() {
                    let cells: Vec<CPos> = w
                        .map()
                        .find_tiles_in_annulus(base_center, info.min_base_radius, info.max_base_radius, false)
                        .filter(|&c| layer.get_resource(c).ty.is_some())
                        .collect();
                    for r in shuffle(cells, &w.bot_random).take(info.max_resource_cells_to_check.max(0) as usize) {
                        if let Some(found) = find_pos(base_center, r, info.min_base_radius, info.max_base_radius) {
                            return Some(found);
                        }
                    }
                }
                find_pos(base_center, base_center, info.min_base_radius, info.max_base_radius)
            }
            BuildingType::Building => find_pos(
                base_center,
                base_center,
                info.min_base_radius,
                if distance_important {
                    info.max_base_radius
                } else {
                    rustora_engine::map::MAXIMUM_TILE_SEARCH_RANGE
                },
            ),
        }
    }
}
