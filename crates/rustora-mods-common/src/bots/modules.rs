// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `McvManagerBotModule.cs`, `HarvesterBotModule.cs` and `BuildingRepairBotModule.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_core::world_types::{CPos, WDist, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::*;
use rustora_engine::damage::{AttackInfo, DamageState};
use rustora_engine::net_collections::NetMap;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRelationship;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::Target;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

use super::*;
use crate::actor_map::WorldExt;
use crate::building::BuildingInfo;
use crate::harvester::{DockClientManager, FindAndDeliverResources, Harvester, HarvesterInfo, ResourceClaimLayer};
use crate::locomotor::BlockedByActor;
use crate::mobile::Mobile;
use crate::parachutable::Parachutable;
use crate::resources::ResourceLayer;
use crate::transforms::TransformsInfo;

pub fn register(r: &mut TraitRegistry) {
    r.register("McvManagerBotModule", |n| load_info!(McvManagerBotModuleInfo, n));
    r.register("HarvesterBotModule", |n| load_info!(HarvesterBotModuleInfo, n));
    r.register("BuildingRepairBotModule", |n| load_info!(BuildingRepairBotModuleInfo, n));
    r.register("MinelayerBotModule", |n| load_info!(MinelayerBotModuleInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct McvManagerBotModuleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub mcv_types: IndexSet<String>,
    pub construction_yard_types: IndexSet<String>,
    pub mcv_factory_types: IndexSet<String>,
    pub minimum_construction_yard_count: i32,
    pub scan_for_new_mcv_interval: i32,
    pub min_base_radius: i32,
    pub max_base_radius: i32,
    #[field(name = "RestrictMCVDeploymentFallbackToBase")]
    pub restrict_mcv_deployment_fallback_to_base: bool,
}

impl Default for McvManagerBotModuleInfo {
    fn default() -> Self {
        McvManagerBotModuleInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            mcv_types: IndexSet::new(),
            construction_yard_types: IndexSet::new(),
            mcv_factory_types: IndexSet::new(),
            minimum_construction_yard_count: 1,
            scan_for_new_mcv_interval: 20,
            min_base_radius: 2,
            max_base_radius: 20,
            restrict_mcv_deployment_fallback_to_base: true,
        }
    }
}

pub struct McvManagerBotModule {
    info: Rc<McvManagerBotModuleInfo>,
    state: ConditionalState,
    self_: ActorRef,
    mcvs: ActorIndex,
    construction_yards: ActorIndex,
    mcv_factories: ActorIndex,
    notify_positions_updated: OnceCell<Vec<Rc<dyn IBotPositionsUpdated>>>,
    request_unit_production: OnceCell<Vec<Rc<dyn IBotRequestUnitProduction>>>,
    initial_base_center: Cell<CPos>,
    scan_interval: Cell<i32>,
    first_tick: Cell<bool>,
}

register_traits!(McvManagerBotModule: IBotTick, IBotPositionsUpdated, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(McvManagerBotModuleInfo => |info, init| {
    let (w, p) = (init.world(), init.self_.owner());
    Some(Rc::new(McvManagerBotModule {
        state: ConditionalState::new(&info.base),
        self_: init.self_.clone(),
        mcvs: ActorIndex::owner_and_names_and_trait::<TransformsInfo>(&w, &info.mcv_types, &p),
        construction_yards: ActorIndex::owner_and_names_and_trait::<BuildingInfo>(&w, &info.construction_yard_types, &p),
        mcv_factories: ActorIndex::owner_and_names_and_trait::<BuildingInfo>(&w, &info.mcv_factory_types, &p),
        notify_positions_updated: OnceCell::new(),
        request_unit_production: OnceCell::new(),
        initial_base_center: Cell::new(CPos::ZERO),
        scan_interval: Cell::new(0),
        first_tick: Cell::new(true),
        info,
    }))
});

impl ConditionalHooks for McvManagerBotModule {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        let i = self.info.scan_for_new_mcv_interval;
        self.scan_interval.set(self_.world().bot_random.borrow_mut().next_range(i, i * 2));
    }
}
rustora_engine::conditional_plumbing!(McvManagerBotModule);

impl INotifyCreated for McvManagerBotModule {
    fn created(&self, self_: &ActorRef) {
        let pa = self_.owner().player_actor().clone();
        let _ = self
            .notify_positions_updated
            .set(pa.traits_implementing::<dyn IBotPositionsUpdated>());
        let _ = self
            .request_unit_production
            .set(pa.traits_implementing::<dyn IBotRequestUnitProduction>());
    }
}

impl IBotPositionsUpdated for McvManagerBotModule {
    fn updated_base_center(&self, location: CPos) {
        self.initial_base_center.set(location);
    }

    fn updated_defense_center(&self, _location: CPos) {}
}

impl IBotTick for McvManagerBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn bot_tick(&self, bot: &ModularBot) {
        if self.first_tick.get() {
            self.deploy_mcvs(bot, false);
            self.first_tick.set(false);
        }
        self.scan_interval.set(self.scan_interval.get() - 1);
        if self.scan_interval.get() <= 0 {
            self.scan_interval.set(self.info.scan_for_new_mcv_interval);
            self.deploy_mcvs(bot, true);
            let unit_builder = first_enabled_unit_production(self.request_unit_production.get().expect("created"));
            if let Some(ub) = unit_builder {
                if !self.info.mcv_types.is_empty() && self.should_build_mcv() {
                    let types: Vec<String> = self.info.mcv_types.iter().cloned().collect();
                    let mcv_type = random_or_default(&types, &self.self_.world().bot_random).expect("mcv type");
                    if ub.requested_production_count(bot, &mcv_type) == 0 {
                        ub.request_unit_production(bot, &mcv_type);
                    }
                }
            }
        }
    }
}

impl McvManagerBotModule {
    fn random_base_center(&self) -> CPos {
        random_or_default(&self.construction_yards.actors(), &self.self_.world().bot_random)
            .map_or(self.initial_base_center.get(), |a| a.location())
    }

    fn should_build_mcv(&self) -> bool {
        if self.mcvs.count_alive() != 0 {
            return false;
        }
        self.construction_yards.count_alive() < self.info.minimum_construction_yard_count && self.mcv_factories.count_alive() > 0
    }

    fn deploy_mcvs(&self, bot: &ModularBot, choose_location: bool) {
        for mcv in self.mcvs.actors().into_iter().filter(|a| a.is_idle()) {
            self.deploy_mcv(bot, &mcv, choose_location);
        }
    }

    fn deploy_mcv(&self, bot: &ModularBot, mcv: &ActorRef, mv: bool) {
        let w = self.self_.world();
        if mv {
            let restrict = self.info.restrict_mcv_deployment_fallback_to_base && self.construction_yards.count_alive() > 0;
            let ti = mcv.info.trait_info::<TransformsInfo>().expect("Transforms");
            let Some(cell) = self.choose_mcv_deploy_location(&ti.into_actor, ti.offset, restrict) else {
                return;
            };
            bot.queue_order(Order::new("Move", Some(mcv.clone()), Target::from_cell(&w, cell, FULL_CELL), true));
        }
        for n in self.notify_positions_updated.get().expect("created") {
            n.updated_base_center(mcv.location());
            n.updated_defense_center(mcv.location());
        }
        bot.queue_order(Order::new("DeployTransform", Some(mcv.clone()), Target::Invalid, true));
    }

    fn choose_mcv_deploy_location(
        &self,
        actor_type: &str,
        offset: rustora_core::world_types::CVec,
        distance_important: bool,
    ) -> Option<CPos> {
        let w = self.self_.world();
        let ai = w.rules().actor(actor_type)?;
        let bi = ai.trait_info::<BuildingInfo>()?;
        let center = self.random_base_center();
        let max = if distance_important {
            self.info.max_base_radius
        } else {
            rustora_engine::map::MAXIMUM_TILE_SEARCH_RANGE
        };
        let cells: Vec<CPos> = w
            .map()
            .find_tiles_in_annulus(center, self.info.min_base_radius, max, false)
            .collect();
        shuffle(cells, &w.bot_random).find(|&c| crate::base::can_place_building(&w, c + offset, &bi, None))
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct HarvesterBotModuleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub harvester_types: IndexSet<String>,
    pub refinery_types: IndexSet<String>,
    pub scan_for_idle_harvesters_interval: i32,
    pub scan_interval_multipler_when_no_resources: i32,
    pub harvester_enemy_avoidance_radius: WDist,
    pub harvester_enemy_avoidance_cost_multipler: i32,
}

impl Default for HarvesterBotModuleInfo {
    fn default() -> Self {
        HarvesterBotModuleInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            harvester_types: IndexSet::new(),
            refinery_types: IndexSet::new(),
            scan_for_idle_harvesters_interval: 50,
            scan_interval_multipler_when_no_resources: 5,
            harvester_enemy_avoidance_radius: WDist::from_cells(10),
            harvester_enemy_avoidance_cost_multipler: 20,
        }
    }
}

struct HarvesterTraitWrapper {
    actor: ActorRef,
    harvester: Rc<Harvester>,
    dock_client_manager: Rc<DockClientManager>,
    parachutable: Option<Rc<Parachutable>>,
    mobile: Option<Rc<Mobile>>,
    no_resources_cooldown: Cell<i32>,
}

pub struct HarvesterBotModule {
    info: Rc<HarvesterBotModuleInfo>,
    state: ConditionalState,
    self_: ActorRef,
    harvesters: RefCell<NetMap<u32, Rc<HarvesterTraitWrapper>>>,
    harvesters_needing_orders: RefCell<Vec<Rc<HarvesterTraitWrapper>>>,
    refineries: ActorIndex,
    harvesters_index: ActorIndex,
    resource_types_by_cell: Rc<RefCell<NetMap<CPos, String>>>,
    request_unit_production: OnceCell<Vec<Rc<dyn IBotRequestUnitProduction>>>,
    scan_for_idle_harvesters_ticks: Cell<i32>,
}

register_traits!(HarvesterBotModule: IBotTick, IWorldLoaded, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(HarvesterBotModuleInfo => |info, init| {
    let (w, p) = (init.world(), init.self_.owner());
    Some(Rc::new(HarvesterBotModule {
        state: ConditionalState::new(&info.base),
        self_: init.self_.clone(),
        harvesters: RefCell::new(NetMap::new()),
        harvesters_needing_orders: RefCell::new(Vec::new()),
        refineries: ActorIndex::owner_and_names_and_trait::<BuildingInfo>(&w, &info.refinery_types, &p),
        harvesters_index: ActorIndex::owner_and_names_and_trait::<HarvesterInfo>(&w, &info.harvester_types, &p),
        resource_types_by_cell: Rc::new(RefCell::new(NetMap::new())),
        request_unit_production: OnceCell::new(),
        scan_for_idle_harvesters_ticks: Cell::new(0),
        info,
    }))
});

impl ConditionalHooks for HarvesterBotModule {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        self.scan_for_idle_harvesters_ticks.set(
            self_
                .world()
                .bot_random
                .borrow_mut()
                .next_max(self.info.scan_for_idle_harvesters_interval),
        );
    }
}
rustora_engine::conditional_plumbing!(HarvesterBotModule);

impl INotifyCreated for HarvesterBotModule {
    fn created(&self, self_: &ActorRef) {
        let _ = self
            .request_unit_production
            .set(self_.owner().player_actor().traits_implementing::<dyn IBotRequestUnitProduction>());
    }
}

impl IWorldLoaded for HarvesterBotModule {
    fn world_loaded(&self, w: &Rc<rustora_engine::world::World>) {
        let Some(layer) = w.world_actor().trait_or_default::<ResourceLayer>() else {
            return;
        };
        {
            let mut m = self.resource_types_by_cell.borrow_mut();
            for c in w.map().all_cells() {
                if let Some(t) = layer.get_resource(c).ty {
                    m.insert(c, t.to_string());
                }
            }
        }
        let m = Rc::downgrade(&self.resource_types_by_cell);
        layer.on_cell_changed(Rc::new(move |c, ty| {
            let Some(m) = m.upgrade() else { return };
            match ty {
                None => {
                    m.borrow_mut().remove(&c);
                }
                Some(t) => {
                    m.borrow_mut().insert(c, t.to_string());
                }
            }
        }));
    }
}

impl IBotTick for HarvesterBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn bot_tick(&self, bot: &ModularBot) {
        let w = self.self_.world();
        let Some(layer) = w.world_actor().trait_or_default::<ResourceLayer>() else {
            return;
        };
        if layer.is_empty() {
            return;
        }
        let mut searched = false;
        while !searched {
            let Some(h) = self.harvesters_needing_orders.borrow_mut().pop() else {
                break;
            };
            searched = self.harvest_if_able(bot, &h);
        }
        self.scan_for_idle_harvesters_ticks
            .set(self.scan_for_idle_harvesters_ticks.get() - 1);
        if self.scan_for_idle_harvesters_ticks.get() > 0 {
            return;
        }
        let owner = self.self_.owner();
        let cannot_be_ordered = |a: &ActorRef| !Rc::ptr_eq(&a.owner(), &owner) || a.is_dead() || !a.is_in_world();
        let to_remove: Vec<u32> = self
            .harvesters
            .borrow()
            .values()
            .filter(|h| cannot_be_ordered(&h.actor))
            .map(|h| h.actor.id)
            .collect();
        for id in to_remove {
            self.harvesters.borrow_mut().remove(&id);
        }
        self.scan_for_idle_harvesters_ticks.set(self.info.scan_for_idle_harvesters_interval);
        for a in w.actors_having_trait::<Harvester>() {
            if cannot_be_ordered(&a) || self.harvesters.borrow().contains_key(&a.id) {
                continue;
            }
            let wrapper = HarvesterTraitWrapper {
                harvester: a.trait_::<Harvester>(),
                dock_client_manager: a.trait_::<DockClientManager>(),
                parachutable: a.trait_or_default::<Parachutable>(),
                mobile: a.trait_or_default::<Mobile>(),
                no_resources_cooldown: Cell::new(0),
                actor: a.clone(),
            };
            self.harvesters.borrow_mut().insert(a.id, Rc::new(wrapper));
        }
        let all: Vec<Rc<HarvesterTraitWrapper>> = self.harvesters.borrow().values().cloned().collect();
        *self.harvesters_needing_orders.borrow_mut() = all;
        let unit_builder = first_enabled_unit_production(self.request_unit_production.get().expect("created"));
        if let Some(ub) = unit_builder {
            if !self.info.harvester_types.is_empty() && self.harvesters_index.count_alive() < self.refineries.count_alive() {
                let types: Vec<String> = self.info.harvester_types.iter().cloned().collect();
                let ty = random_or_default(&types, &w.bot_random).expect("harvester type");
                if ub.requested_production_count(bot, &ty) == 0 {
                    ub.request_unit_production(bot, &ty);
                }
            }
        }
    }
}

impl HarvesterBotModule {
    fn harvest_if_able(&self, bot: &ModularBot, h: &HarvesterTraitWrapper) -> bool {
        if h.actor.is_dead() || !h.actor.is_in_world() || h.mobile.is_none() {
            return false;
        }
        if !h.actor.is_idle() {
            let failed = h
                .actor
                .current_activity()
                .and_then(|a| a.as_any().downcast_ref::<FindAndDeliverResources>().map(|f| f.last_search_failed()));
            if failed != Some(true) {
                return false;
            }
        }
        if h.no_resources_cooldown.get() > 1 {
            h.no_resources_cooldown.set(h.no_resources_cooldown.get() - 1);
            return false;
        }
        if h.parachutable.as_ref().is_some_and(|p| p.is_in_air()) {
            return false;
        }
        let target = self.find_next_resource(h);
        if !matches!(target, Target::Invalid) {
            bot.queue_order(Order::new("Harvest", Some(h.actor.clone()), target, false));
        } else {
            h.no_resources_cooldown.set(self.info.scan_interval_multipler_when_no_resources);
        }
        true
    }

    fn find_next_resource(&self, h: &HarvesterTraitWrapper) -> Target {
        let w = self.self_.world();
        let actor = &h.actor;
        let scan_from = h
            .dock_client_manager
            .closest_dock(None, Default::default(), false, true)
            .map_or_else(|| actor.clone(), |(a, _)| a);
        let claim = w.world_actor().trait_::<ResourceClaimLayer>();
        let targets: Vec<CPos> = self
            .resource_types_by_cell
            .borrow()
            .iter()
            .filter(|(c, t)| h.harvester.info.resources.contains(t) && claim.can_claim_cell(actor, **c))
            .map(|(c, _)| *c)
            .collect();
        let mobile = h.mobile.as_ref().expect("mobile");
        let cell_radius = self.info.harvester_enemy_avoidance_radius.length / 1024;
        let min_cell_cost = mobile
            .locomotor_info()
            .terrain_speeds
            .values()
            .map(|t| t.cost as i32)
            .min()
            .unwrap_or(0);
        let multiplier = self.info.harvester_enemy_avoidance_cost_multipler;
        let bins: Rc<RefCell<HashMap<(i32, i32), i32>>> = Rc::new(RefCell::new(HashMap::new()));
        let (w2, a2) = (w.clone(), actor.clone());
        let cost: crate::pathfinder::CustomCost = Rc::new(move |loc: CPos| {
            let bin = (loc.x() / cell_radius, loc.y() / cell_radius);
            if let Some(&c) = bins.borrow().get(&bin) {
                return c;
            }
            let r = WDist::from_cells(cell_radius).length;
            let v = WVec::new(r, r, 0);
            let origin = w2.map().center_of_cell(CPos::new(
                bin.0 * cell_radius + cell_radius / 2,
                bin.1 * cell_radius + cell_radius / 2,
            ));
            let threats = w2
                .actor_map()
                .actors_in_box(origin - v, origin + v)
                .into_iter()
                .filter(|u| !u.is_dead() && a2.owner().relationship_with(Some(&u.owner())) == PlayerRelationship::ENEMY)
                .count() as i32;
            let c = threats * min_cell_cost * multiplier;
            bins.borrow_mut().insert(bin, c);
            c
        });
        let path = mobile.path_finder().find_path_to_target_cells(
            actor,
            scan_from.location(),
            &targets,
            BlockedByActor::Stationary,
            Some(cost),
            None,
            false,
        );
        match path.first() {
            Some(&c) => Target::from_cell(&w, c, FULL_CELL),
            None => Target::Invalid,
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct BuildingRepairBotModuleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
}

pub struct BuildingRepairBotModule {
    state: ConditionalState,
}

register_traits!(BuildingRepairBotModule: IBotRespondToAttack, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(BuildingRepairBotModuleInfo => |info, _| Some(Rc::new(BuildingRepairBotModule { state: ConditionalState::new(&info.base) })));
rustora_engine::conditional_simple!(BuildingRepairBotModule);

impl IBotRespondToAttack for BuildingRepairBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn respond_to_attack(&self, bot: &ModularBot, self_: &ActorRef, e: &AttackInfo) {
        let attacker_owner = e.attacker.as_ref().map(|a| a.owner());
        if self_.is_dead() || self_.owner().relationship_with(attacker_owner.as_deref()) == PlayerRelationship::NEUTRAL {
            return;
        }
        let Some(rb) = self_.trait_or_default::<crate::base::RepairableBuilding>() else {
            return;
        };
        if e.damage_state > DamageState::LIGHT && e.previous_damage_state <= DamageState::LIGHT && !rb.repair_active() {
            let pa = self_.owner().player_actor().clone();
            bot.queue_order(Order::new("RepairBuilding", Some(pa), Target::from_actor(Some(self_)), false));
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MinelayerBotModuleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub ignored_enemy_target_types: rustora_engine::bitset::BitSet<rustora_engine::terrain::TargetableType>,
    pub use_enemy_location_target_types: rustora_engine::bitset::BitSet<rustora_engine::terrain::TargetableType>,
    pub minelaying_actor_types: IndexSet<String>,
    pub max_per_assign: i32,
    pub scan_tick: i32,
    pub mine_field_radius: i32,
    pub away_from_allied_target_types: rustora_engine::bitset::BitSet<rustora_engine::terrain::TargetableType>,
    pub away_from_enemy_target_types: rustora_engine::bitset::BitSet<rustora_engine::terrain::TargetableType>,
    pub away_from_cell_distance: i32,
    pub favorite_position_distance: i32,
}

impl Default for MinelayerBotModuleInfo {
    fn default() -> Self {
        MinelayerBotModuleInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            ignored_enemy_target_types: Default::default(),
            use_enemy_location_target_types: Default::default(),
            minelaying_actor_types: IndexSet::new(),
            max_per_assign: 1,
            scan_tick: 320,
            mine_field_radius: 1,
            away_from_allied_target_types: Default::default(),
            away_from_enemy_target_types: Default::default(),
            away_from_cell_distance: 9,
            favorite_position_distance: 6,
        }
    }
}

const MAX_POSITION_CACHE_LENGTH: usize = 5;
const REPEATED_ALERT_TICKS: i32 = 40;

/// `MinelayerBotModule`: lays mines at recent conflict positions and keeps a few favorite positions.
pub struct MinelayerBotModule {
    info: Rc<MinelayerBotModuleInfo>,
    state: ConditionalState,
    self_: ActorRef,
    conflict_positions: RefCell<Vec<CPos>>,
    favorite_positions: RefCell<Vec<CPos>>,
    min_assign_role_delay_ticks: Cell<i32>,
    current_favorite_position_index: Cell<usize>,
    alerted_ticks: Cell<i32>,
}

register_traits!(MinelayerBotModule: IBotTick, IBotRespondToAttack, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(MinelayerBotModuleInfo => |info, init| Some(Rc::new(MinelayerBotModule {
    state: ConditionalState::new(&info.base),
    self_: init.self_.clone(),
    conflict_positions: RefCell::new(Vec::new()),
    favorite_positions: RefCell::new(Vec::new()),
    min_assign_role_delay_ticks: Cell::new(0),
    current_favorite_position_index: Cell::new(0),
    alerted_ticks: Cell::new(0),
    info,
})));

impl ConditionalHooks for MinelayerBotModule {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        self.min_assign_role_delay_ticks
            .set(self_.world().bot_random.borrow_mut().next_range(0, self.info.scan_tick));
        self.alerted_ticks.set(0);
        self.conflict_positions.borrow_mut().clear();
        self.favorite_positions.borrow_mut().clear();
        self.current_favorite_position_index.set(0);
    }
}
rustora_engine::conditional_plumbing!(MinelayerBotModule);

impl INotifyCreated for MinelayerBotModule {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl MinelayerBotModule {
    fn cannot_be_ordered_or_busy(&self, a: &ActorRef) -> bool {
        a.is_dead() || !a.is_in_world() || a.owner_opt().is_none_or(|o| !Rc::ptr_eq(&o, &self.self_.owner())) || !a.is_idle()
    }

    fn is_preferred_enemy_unit(&self, a: Option<&ActorRef>) -> bool {
        let Some(a) = a else { return false };
        let player = self.self_.owner();
        if a.is_dead()
            || player.relationship_with(Some(&a.owner())) != PlayerRelationship::ENEMY
            || a.info.has_trait_info::<crate::death::HuskInfo>()
        {
            return false;
        }
        let types = rustora_engine::actor_ext::enabled_target_types(a);
        !types.is_empty() && !types.overlaps(&self.info.ignored_enemy_target_types)
    }

    /// `HasInvalidActorInCircle`: whether an ally or enemy to keep away from is near, and whether any enemy is near.
    /// Like the C# `Any`, the scan stops at the first invalid actor.
    fn has_invalid_actor_in_circle(&self, pos: rustora_core::world_types::WPos, dist: WDist) -> (bool, bool) {
        let w = self.self_.world();
        let player = self.self_.owner();
        let mut enemy = false;
        for a in w.find_actors_in_circle(pos, dist) {
            let rel = a.owner().relationship_with(Some(&player));
            let away = if rel == PlayerRelationship::ALLY {
                &self.info.away_from_allied_target_types
            } else if rel == PlayerRelationship::ENEMY {
                enemy = true;
                &self.info.away_from_enemy_target_types
            } else {
                continue;
            };
            let types = rustora_engine::actor_ext::enabled_target_types(&a);
            if !types.is_empty() && types.overlaps(away) {
                return (true, enemy);
            }
        }
        (false, enemy)
    }

    fn enqueue_conflict_position(&self, cell: CPos) {
        let mut q = self.conflict_positions.borrow_mut();
        if q.len() < MAX_POSITION_CACHE_LENGTH {
            q.push(cell);
        } else {
            q[MAX_POSITION_CACHE_LENGTH - 1] = cell;
        }
    }

    fn dequeue_first_conflict_position(&self) {
        self.conflict_positions.borrow_mut().remove(0);
    }

    fn delete_current_favorite_position(&self) {
        let mut f = self.favorite_positions.borrow_mut();
        f.remove(self.current_favorite_position_index.get());
        if !f.is_empty() {
            self.current_favorite_position_index
                .set(self.current_favorite_position_index.get() % f.len());
        }
    }

    fn add_position_to_favorite_positions(&self, cell: CPos) {
        let d = self.info.favorite_position_distance;
        let mut f = self.favorite_positions.borrow_mut();
        let (mut closest_index, mut closest) = (0usize, i32::MAX);
        for (i, p) in f.iter().enumerate() {
            let l = (*p - cell).length_squared();
            if l < closest {
                closest_index = i;
                closest = l;
            }
        }
        if closest > d * d && f.len() < MAX_POSITION_CACHE_LENGTH {
            f.push(cell);
        } else {
            let pos = f[closest_index];
            let delta = pos - cell;
            f[closest_index] = cell + rustora_core::world_types::CVec::new(delta.x / 2, delta.y / 2);
        }
    }

    fn minelayers(&self) -> Vec<ActorRef> {
        self.self_
            .world()
            .actors_having_trait::<crate::minelayer::Minelayer>()
            .into_iter()
            .filter(|a| !self.cannot_be_ordered_or_busy(a))
            .collect()
    }

    fn find_path(&self, a: &ActorRef, target: CPos) -> Vec<CPos> {
        let pf = self.self_.world().world_actor().trait_::<crate::path_finder_trait::PathFinder>();
        pf.find_path_to_target_cell(a, &[a.location()], target, BlockedByActor::Immovable, None, None, false)
    }
}

impl IBotTick for MinelayerBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn bot_tick(&self, bot: &ModularBot) {
        if self.alerted_ticks.get() > 0 {
            self.alerted_ticks.set(self.alerted_ticks.get() - 1);
        }
        self.min_assign_role_delay_ticks.set(self.min_assign_role_delay_ticks.get() - 1);
        if self.min_assign_role_delay_ticks.get() > 0 {
            return;
        }
        self.min_assign_role_delay_ticks.set(self.info.scan_tick);
        let w = self.self_.world();
        let away = WDist::from_cells(self.info.away_from_cell_distance);
        let mut position = CPos::ZERO;
        let mut use_favorite = false;
        let mut halfway = false;
        while let Some(&p) = self.conflict_positions.borrow().first() {
            position = p;
            let (invalid, enemy) = self.has_invalid_actor_in_circle(w.map().center_of_cell(p), away);
            if invalid {
                self.dequeue_first_conflict_position();
            } else {
                halfway = enemy;
                break;
            }
        }
        let mut minelayers: Option<Vec<ActorRef>> = None;
        if self.conflict_positions.borrow().is_empty() {
            if self.favorite_positions.borrow().is_empty() {
                let ml = self.minelayers();
                if ml.is_empty() {
                    return;
                }
                let enemies: Vec<ActorRef> = w.actors().into_iter().filter(|a| self.is_preferred_enemy_unit(Some(a))).collect();
                let Some(enemy) = random_or_default(&enemies, &w.bot_random) else {
                    return;
                };
                for m in &ml {
                    let cells = self.find_path(m, enemy.location());
                    if !cells.is_empty() {
                        self.enqueue_conflict_position(cells[cells.len() / 2]);
                        return;
                    }
                }
                return;
            }
            loop {
                let p = {
                    let f = self.favorite_positions.borrow();
                    if f.is_empty() {
                        break;
                    }
                    f[self.current_favorite_position_index.get()]
                };
                position = p;
                let (invalid, enemy) = self.has_invalid_actor_in_circle(w.map().center_of_cell(p), away);
                if invalid {
                    self.delete_current_favorite_position();
                    if self.favorite_positions.borrow().is_empty() {
                        return;
                    }
                } else {
                    halfway = enemy;
                    use_favorite = true;
                    break;
                }
            }
        }
        let ml = minelayers.get_or_insert_with(|| self.minelayers()).clone();
        if ml.is_empty() {
            return;
        }
        let mut ordered: Vec<ActorRef> = Vec::new();
        for m in ml {
            let cells = self.find_path(&m, position);
            if !cells.is_empty() {
                ordered.push(m);
                if halfway {
                    position = cells[cells.len() / 4];
                    halfway = false;
                }
                if ordered.len() as i32 >= self.info.max_per_assign {
                    break;
                }
            }
        }
        if ordered.is_empty() {
            if use_favorite {
                self.delete_current_favorite_position();
            } else {
                self.dequeue_first_conflict_position();
            }
            return;
        }
        if use_favorite {
            let n = self.favorite_positions.borrow().len();
            self.current_favorite_position_index
                .set((self.current_favorite_position_index.get() + 1) % n);
        } else {
            self.dequeue_first_conflict_position();
            self.add_position_to_favorite_positions(position);
        }
        let r = self.info.mine_field_radius;
        let vec = rustora_core::world_types::CVec::new(r, r);
        let grouped: Vec<Option<ActorRef>> = ordered.iter().cloned().map(Some).collect();
        let mut place = Order::new("PlaceMinefield", None, Target::from_cell(&w, position + vec, FULL_CELL), false);
        place.grouped_actors = Some(grouped.clone());
        place.extra_location = position - vec;
        bot.queue_order(place);
        let mut mv = Order::new("Move", None, Target::from_cell(&w, ordered[0].location(), FULL_CELL), true);
        mv.grouped_actors = Some(grouped);
        bot.queue_order(mv);
    }
}

impl IBotRespondToAttack for MinelayerBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn respond_to_attack(&self, _bot: &ModularBot, self_: &ActorRef, e: &AttackInfo) {
        if self.alerted_ticks.get() > 0 || !self.is_preferred_enemy_unit(e.attacker.as_ref()) {
            return;
        }
        self.alerted_ticks.set(REPEATED_ALERT_TICKS);
        if self
            .has_invalid_actor_in_circle(self_.center_position(), WDist::from_cells(self.info.away_from_cell_distance))
            .0
        {
            return;
        }
        let types = rustora_engine::actor_ext::enabled_target_types(self_);
        let pos = if !types.is_empty() && types.overlaps(&self.info.use_enemy_location_target_types) {
            e.attacker.as_ref().expect("attacker").location()
        } else {
            self_.location()
        };
        self.enqueue_conflict_position(pos);
    }
}
