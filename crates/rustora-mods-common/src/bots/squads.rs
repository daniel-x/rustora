// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `SquadManagerBotModule.cs`, `Squads/Squad.cs`, `Squads/StateMachine.cs` and the ground, air and
//! protection squad states.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_core::world_types::{CPos, WDist, WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::AttackInfo;
use rustora_engine::net_collections::NetMap;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRelationship;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::terrain::TargetableType;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

use super::fuzzy::AttackOrFleeFuzzy;
use super::save_data::{format_cpos, format_ids, format_wvec, node_value, parse_cpos, parse_ids, parse_wvec};
use super::*;
use crate::actor_map::WorldExt;
use crate::aircraft::AircraftInfo;
use crate::attack::{Armament, AttackBase, AttackBaseInfo};
use crate::building::{Building, BuildingInfo};
use crate::death::HuskInfo;
use crate::interfaces::IPositionable;
use crate::mobile::Mobile;
use crate::path_finder_trait::PathFinder;
use crate::resupply::{AmmoPool, Rearmable};

pub fn register(r: &mut TraitRegistry) {
    r.register("SquadManagerBotModule", |n| load_info!(SquadManagerBotModuleInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct SquadManagerBotModuleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub naval_units_types: IndexSet<String>,
    pub air_units_types: IndexSet<String>,
    pub exclude_from_squads_types: IndexSet<String>,
    pub construction_yard_types: IndexSet<String>,
    pub naval_production_types: IndexSet<String>,
    pub protection_types: IndexSet<String>,
    pub aircraft_target_type: BitSet<TargetableType>,
    pub squad_size: i32,
    pub squad_size_random_bonus: i32,
    pub assign_roles_interval: i32,
    pub rush_interval: i32,
    pub attack_force_interval: i32,
    pub minimum_attack_force_delay: i32,
    pub rush_attack_scan_radius: i32,
    pub protect_unit_scan_radius: i32,
    pub max_base_radius: i32,
    pub idle_scan_radius: i32,
    pub danger_scan_radius: i32,
    pub attack_scan_radius: i32,
    pub protection_scan_radius: i32,
    pub ignored_enemy_target_types: BitSet<TargetableType>,
}

impl Default for SquadManagerBotModuleInfo {
    fn default() -> Self {
        SquadManagerBotModuleInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            naval_units_types: IndexSet::new(),
            air_units_types: IndexSet::new(),
            exclude_from_squads_types: IndexSet::new(),
            construction_yard_types: IndexSet::new(),
            naval_production_types: IndexSet::new(),
            protection_types: IndexSet::new(),
            aircraft_target_type: BitSet::new(&["Air"]),
            squad_size: 8,
            squad_size_random_bonus: 30,
            assign_roles_interval: 50,
            rush_interval: 600,
            attack_force_interval: 75,
            minimum_attack_force_delay: 0,
            rush_attack_scan_radius: 15,
            protect_unit_scan_radius: 15,
            max_base_radius: 20,
            idle_scan_radius: 10,
            danger_scan_radius: 10,
            attack_scan_radius: 12,
            protection_scan_radius: 8,
            ignored_enemy_target_types: BitSet::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, rustora_engine::persist::Persist)]
#[persist(value)]
pub enum SquadType {
    Assault,
    Air,
    Rush,
    Protection,
    Naval,
}

/// The squad states. Ground states carry the elected leader and the stall detection of the attack states.
#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
enum State {
    GroundIdle {
        leader: Option<ActorRef>,
    },
    GroundAttackMove {
        leader: Option<ActorRef>,
        last_updated_tick: i32,
        last_leader_location: Option<CPos>,
        last_target: Option<u32>,
    },
    GroundAttack {
        leader: Option<ActorRef>,
        last_updated_tick: i32,
        last_leader_location: Option<CPos>,
        last_target: Option<u32>,
    },
    GroundFlee,
    AirIdle,
    AirAttack,
    AirFlee,
    ProtectionIdle,
    ProtectionAttack {
        leader: Option<ActorRef>,
        backoff: i32,
    },
    ProtectionFlee,
}

const BACKOFF_TICKS: i32 = 4;
const MISSILE_UNIT_MULTIPLIER: i32 = 3;

impl State {
    fn ground_attack_move() -> State {
        State::GroundAttackMove {
            leader: None,
            last_updated_tick: 0,
            last_leader_location: None,
            last_target: None,
        }
    }

    fn ground_attack() -> State {
        State::GroundAttack {
            leader: None,
            last_updated_tick: 0,
            last_leader_location: None,
            last_target: None,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(shared)]
pub struct Squad {
    pub units: RefCell<NetMap<u32, ActorRef>>,
    pub ty: SquadType,
    target: RefCell<Target>,
    target_actor: RefCell<Option<ActorRef>>,
    state: RefCell<State>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct SquadManagerBotModule {
    pub info: Rc<SquadManagerBotModuleInfo>,
    state: ConditionalState,
    self_: ActorRef,
    units_hanging_around_the_base: RefCell<Vec<ActorRef>>,
    active_units: RefCell<NetMap<u32, ActorRef>>,
    pub squads: RefCell<Vec<Rc<Squad>>>,
    squads_pending_update: RefCell<Vec<Rc<Squad>>>,
    construction_yard_buildings: ActorIndex,
    notify_positions_updated: OnceCell<Vec<Rc<dyn IBotPositionsUpdated>>>,
    notify_idle_base_units: OnceCell<Vec<Rc<dyn IBotNotifyIdleBaseUnits>>>,
    initial_base_center: Cell<CPos>,
    rush_ticks: Cell<i32>,
    assign_roles_ticks: Cell<i32>,
    attack_force_ticks: Cell<i32>,
    min_attack_force_delay_ticks: Cell<i32>,
}

register_traits!(SquadManagerBotModule: IGameSaveTraitData, IBotEnabled, IBotTick, IBotRespondToAttack, IBotPositionsUpdated, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(SquadManagerBotModuleInfo => |info, init| {
    Some(Rc::new(SquadManagerBotModule {
        state: ConditionalState::new(&info.base),
        self_: init.self_.clone(),
        units_hanging_around_the_base: RefCell::new(Vec::new()),
        active_units: RefCell::new(NetMap::new()),
        squads: RefCell::new(Vec::new()),
        squads_pending_update: RefCell::new(Vec::new()),
        construction_yard_buildings: ActorIndex::names_and_trait::<BuildingInfo>(&init.world(), &info.construction_yard_types),
        notify_positions_updated: OnceCell::new(),
        notify_idle_base_units: OnceCell::new(),
        initial_base_center: Cell::new(CPos::ZERO),
        rush_ticks: Cell::new(0),
        assign_roles_ticks: Cell::new(0),
        attack_force_ticks: Cell::new(0),
        min_attack_force_delay_ticks: Cell::new(0),
        info,
    }))
});

impl ConditionalHooks for SquadManagerBotModule {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        let w = self_.world();
        let mut r = w.bot_random.borrow_mut();
        let small = self.info.rush_interval / 20;
        self.rush_ticks
            .set(r.next_range(self.info.rush_interval - small, self.info.rush_interval + small));
        self.assign_roles_ticks.set(r.next_range(0, self.info.assign_roles_interval));
        self.attack_force_ticks.set(r.next_range(0, self.info.attack_force_interval));
        self.min_attack_force_delay_ticks
            .set(r.next_range(0, self.info.minimum_attack_force_delay));
    }
}
rustora_engine::conditional_plumbing!(SquadManagerBotModule);

impl IGameSaveTraitData for SquadManagerBotModule {
    fn issue_trait_data(&self, _actor: &ActorRef) -> Option<Vec<MiniYamlNode>> {
        if self.state.is_trait_disabled() {
            return None;
        }
        let squads = self
            .squads
            .borrow()
            .iter()
            .map(|s| MiniYamlNode::new("Squad", s.serialize()))
            .collect();
        let orderable = |a: &&ActorRef| !self.unit_cannot_be_ordered(a);
        let hanging_around = format_ids(self.units_hanging_around_the_base.borrow().iter().filter(orderable).map(|a| a.id));
        let active = format_ids(self.active_units.borrow().values().filter(orderable).map(|a| a.id));
        let int = |key: &str, value: i32| MiniYamlNode::with_value_str(key, value.to_string());
        Some(vec![
            MiniYamlNode::new("Squads", MiniYaml::new(Some(String::new()), squads)),
            MiniYamlNode::with_value_str("InitialBaseCenter", format_cpos(self.initial_base_center.get())),
            MiniYamlNode::with_value_str("UnitsHangingAroundTheBase", hanging_around),
            MiniYamlNode::with_value_str("ActiveUnits", active),
            int("RushTicks", self.rush_ticks.get()),
            int("AssignRolesTicks", self.assign_roles_ticks.get()),
            int("AttackForceTicks", self.attack_force_ticks.get()),
            int("MinAttackForceDelayTicks", self.min_attack_force_delay_ticks.get()),
        ])
    }

    fn resolve_trait_data(&self, self_: &ActorRef, data: &MiniYaml) {
        let world = self_.world();
        if world.is_replay() {
            return;
        }
        let actors = |key: &str| node_value(data, key).map(|v| parse_ids(v).filter_map(|id| world.get_actor_by_id(id)).collect::<Vec<_>>());
        if let Some(c) = node_value(data, "InitialBaseCenter").and_then(parse_cpos) {
            self.initial_base_center.set(c);
        }
        if let Some(units) = actors("UnitsHangingAroundTheBase") {
            *self.units_hanging_around_the_base.borrow_mut() = units;
        }
        if let Some(units) = actors("ActiveUnits") {
            let mut active = self.active_units.borrow_mut();
            active.clear();
            for a in units {
                active.insert(a.id, a);
            }
        }
        let set_int = |key: &str, cell: &Cell<i32>| {
            if let Some(v) = node_value(data, key).and_then(|v| v.parse().ok()) {
                cell.set(v);
            }
        };
        set_int("RushTicks", &self.rush_ticks);
        set_int("AssignRolesTicks", &self.assign_roles_ticks);
        set_int("AttackForceTicks", &self.attack_force_ticks);
        set_int("MinAttackForceDelayTicks", &self.min_attack_force_delay_ticks);
        if let Ok(Some(squads)) = data.node_with_key_or_default("Squads") {
            self.squads.borrow_mut().clear();
            for n in &squads.value.nodes {
                self.deserialize_squad(&world, &n.value);
            }
        }
    }
}

impl INotifyCreated for SquadManagerBotModule {
    fn created(&self, self_: &ActorRef) {
        let pa = self_.owner().player_actor().clone();
        let _ = self
            .notify_positions_updated
            .set(pa.traits_implementing::<dyn IBotPositionsUpdated>());
        let _ = self
            .notify_idle_base_units
            .set(pa.traits_implementing::<dyn IBotNotifyIdleBaseUnits>());
    }
}

impl IBotEnabled for SquadManagerBotModule {
    fn bot_enabled(&self, _bot: &Rc<ModularBot>) {}
}

impl IBotPositionsUpdated for SquadManagerBotModule {
    fn updated_base_center(&self, location: CPos) {
        self.initial_base_center.set(location);
    }

    fn updated_defense_center(&self, _location: CPos) {}
}

impl IBotTick for SquadManagerBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn bot_tick(&self, bot: &ModularBot) {
        self.assign_roles_to_idle_units(bot);
    }
}

impl IBotRespondToAttack for SquadManagerBotModule {
    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn respond_to_attack(&self, bot: &ModularBot, self_: &ActorRef, e: &AttackInfo) {
        let Some(attacker) = e.attacker.clone() else { return };
        if !self.is_preferred_enemy_unit(Some(&attacker)) {
            return;
        }
        if self.info.protection_types.contains(&self_.info.name) {
            for n in self.notify_positions_updated.get().expect("created") {
                n.updated_defense_center(attacker.location());
            }
            self.protect_own(bot, &attacker);
        }
    }
}

/// `WPos.Average`.
fn average(positions: impl Iterator<Item = WPos>) -> WPos {
    let (mut n, mut x, mut y, mut z) = (0i64, 0i64, 0i64, 0i64);
    for p in positions {
        n += 1;
        x += p.x as i64;
        y += p.y as i64;
        z += p.z as i64;
    }
    if n == 0 {
        return WPos::new(0, 0, 0);
    }
    WPos::new((x / n) as i32, (y / n) as i32, (z / n) as i32)
}

/// `Exts.MultiplyBySqrtTwoOverTwo`.
fn sqrt_two_over_two(n: i32) -> i32 {
    (n as i64 * 23170 / 32768) as i32
}

fn eight_offsets(range: i32) -> [WVec; 9] {
    let d = sqrt_two_over_two(range);
    [
        WVec::new(0, 0, 0),
        WVec::new(range, 0, 0),
        WVec::new(-range, 0, 0),
        WVec::new(0, range, 0),
        WVec::new(0, -range, 0),
        WVec::new(d, d, 0),
        WVec::new(-d, d, 0),
        WVec::new(-d, -d, 0),
        WVec::new(d, -d, 0),
    ]
}

impl SquadManagerBotModule {
    fn unit_cannot_be_ordered(&self, a: &ActorRef) -> bool {
        a.owner_opt().is_none_or(|o| !Rc::ptr_eq(&o, &self.self_.owner())) || a.is_dead() || !a.is_in_world()
    }

    pub fn random_base_center(&self) -> CPos {
        let player = self.self_.owner();
        let yards: Vec<ActorRef> = self
            .construction_yard_buildings
            .actors()
            .into_iter()
            .filter(|a| a.is_owned_by(&player))
            .collect();
        random_or_default(&yards, &self.self_.world().bot_random).map_or(self.initial_base_center.get(), |a| a.location())
    }

    pub fn is_preferred_enemy_unit(&self, a: Option<&ActorRef>) -> bool {
        let Some(a) = a else { return false };
        let player = self.self_.owner();
        if a.is_dead() || player.relationship_with(Some(&a.owner())) != PlayerRelationship::ENEMY || a.info.has_trait_info::<HuskInfo>() {
            return false;
        }
        let types = rustora_engine::actor_ext::enabled_target_types(a);
        if types.is_empty() || types.overlaps(&self.info.ignored_enemy_target_types) {
            return false;
        }
        let mods = a.traits_implementing::<dyn IVisibilityModifier>();
        mods.is_empty() || mods.iter().any(|v| v.is_visible(a, Some(&player)))
    }

    /// `FindEnemies`: preferred enemies attackable by `source`, with the reachable offset closest to them.
    fn find_enemies(&self, actors: impl IntoIterator<Item = ActorRef>, source: &ActorRef) -> Vec<(ActorRef, WVec)> {
        let attack_bases: Vec<Rc<AttackBase>> = source
            .traits_implementing::<AttackBase>()
            .into_iter()
            .filter(|ab| !ab.is_trait_disabled())
            .collect();
        let mut ranged: NetMap<u32, (ActorRef, i32)> = NetMap::new();
        for a in actors {
            if !self.is_preferred_enemy_unit(Some(&a)) {
                continue;
            }
            let t = Target::from_actor(Some(&a));
            let valid: Vec<&Rc<AttackBase>> = attack_bases
                .iter()
                .filter(|ab| ab.has_any_valid_weapons(&t, false, false))
                .collect();
            if valid.is_empty() {
                continue;
            }
            let range = valid
                .iter()
                .map(|ab| ab.get_maximum_range_versus_target(&Target::from_actor(Some(&a))).length)
                .max()
                .expect("attack base");
            if !ranged.contains_key(&a.id) {
                ranged.insert(a.id, (a, range));
            }
        }
        let w = source.world();
        let is_air = source.info.has_trait_info::<AircraftInfo>();
        let mobile = source.trait_or_default::<Mobile>();
        if !is_air && mobile.is_none() {
            return Vec::new();
        }
        let pf = w.world_actor().trait_::<PathFinder>();
        let map = w.map();
        let mut out = Vec::new();
        for (a, range) in ranged.values() {
            let offsets = eight_offsets(*range);
            let reachable: Vec<WVec> = if is_air {
                offsets.to_vec()
            } else {
                let l = mobile.as_ref().expect("mobile").locomotor();
                offsets
                    .iter()
                    .copied()
                    .filter(|o| {
                        pf.path_exists_for_locomotor(
                            &l,
                            map.cell_containing(source.center_position()),
                            map.cell_containing(a.center_position() + *o),
                        )
                    })
                    .collect()
            };
            if reachable.is_empty() {
                continue;
            }
            let best = reachable.iter().copied().min_by_key(|o| o.length_squared()).expect("offset");
            out.push((a.clone(), best));
        }
        out
    }

    fn find_closest_enemy_in(&self, actors: Vec<ActorRef>, source: &ActorRef) -> Option<(ActorRef, WVec)> {
        let p = source.center_position();
        self.find_enemies(actors, source)
            .into_iter()
            .min_by_key(|(a, _)| (a.center_position() - p).length_squared())
    }

    fn find_closest_enemy(&self, source: &ActorRef) -> Option<(ActorRef, WVec)> {
        self.find_closest_enemy_in(self.self_.world().actors(), source)
    }

    fn find_closest_enemy_within(&self, source: &ActorRef, radius: WDist) -> Option<(ActorRef, WVec)> {
        self.find_closest_enemy_in(self.self_.world().find_actors_in_circle(source.center_position(), radius), source)
    }

    fn clean_squads(&self) {
        for s in self.squads.borrow().iter() {
            let dead: Vec<u32> = s
                .units
                .borrow()
                .values()
                .filter(|a| self.unit_cannot_be_ordered(a))
                .map(|a| a.id)
                .collect();
            for id in dead {
                s.units.borrow_mut().remove(&id);
            }
        }
        self.squads.borrow_mut().retain(|s| s.is_valid());
    }

    fn squad_of_type(&self, ty: SquadType) -> Option<Rc<Squad>> {
        self.squads.borrow().iter().find(|s| s.ty == ty).cloned()
    }

    fn register_new_squad(&self, ty: SquadType, target: Option<(ActorRef, WVec)>) -> Rc<Squad> {
        let state = match ty {
            SquadType::Assault | SquadType::Rush | SquadType::Naval => State::GroundIdle { leader: None },
            SquadType::Air => State::AirIdle,
            SquadType::Protection => State::ProtectionIdle,
        };
        let s = Rc::new(Squad {
            units: RefCell::new(NetMap::new()),
            ty,
            target: RefCell::new(Target::Invalid),
            target_actor: RefCell::new(None),
            state: RefCell::new(state),
        });
        s.set_actor_to_target(target);
        self.squads.borrow_mut().push(s.clone());
        s
    }

    /// `Squad.Deserialize`.
    fn deserialize_squad(&self, world: &World, yaml: &MiniYaml) {
        let ty = node_value(yaml, "Type").and_then(SquadType::parse).unwrap_or(SquadType::Rush);
        let actor = node_value(yaml, "ActorToTarget")
            .and_then(|v| v.parse().ok())
            .and_then(|id| world.get_actor_by_id(id));
        let offset = node_value(yaml, "TargetOffset").and_then(parse_wvec);
        let squad = self.register_new_squad(ty, actor.zip(offset));
        if let Some(v) = node_value(yaml, "Units") {
            let mut units = squad.units.borrow_mut();
            for a in parse_ids(v).filter_map(|id| world.get_actor_by_id(id)) {
                units.insert(a.id, a);
            }
        }
    }

    fn unregister_squad(&self, squad: &Squad) {
        let ids: Vec<u32> = squad.units.borrow().keys().copied().collect();
        let mut active = self.active_units.borrow_mut();
        for id in ids {
            active.remove(&id);
        }
        squad.units.borrow_mut().clear();
    }

    fn notify_idle(&self) {
        let units = self.units_hanging_around_the_base.borrow().clone();
        for n in self.notify_idle_base_units.get().expect("created") {
            n.updated_idle_base_units(&units);
        }
    }

    fn assign_roles_to_idle_units(&self, bot: &ModularBot) {
        self.clean_squads();
        let dead: Vec<u32> = self
            .active_units
            .borrow()
            .values()
            .filter(|a| self.unit_cannot_be_ordered(a))
            .map(|a| a.id)
            .collect();
        for id in dead {
            self.active_units.borrow_mut().remove(&id);
        }
        self.units_hanging_around_the_base
            .borrow_mut()
            .retain(|a| !self.unit_cannot_be_ordered(a));
        self.notify_idle();

        self.rush_ticks.set(self.rush_ticks.get() - 1);
        if self.rush_ticks.get() <= 0 {
            self.rush_ticks.set(self.info.rush_interval);
            self.try_to_rush_attack();
        }
        self.attack_force_ticks.set(self.attack_force_ticks.get() - 1);
        if self.attack_force_ticks.get() <= 0 {
            self.attack_force_ticks.set(self.info.attack_force_interval);
            let squads = self.squads.borrow().clone();
            self.squads_pending_update.borrow_mut().extend(squads);
        }
        let n = super::base_builder::div_away_from_zero(self.squads_pending_update.borrow().len() as i32, self.attack_force_ticks.get());
        for _ in 0..n {
            let s = self.squads_pending_update.borrow_mut().pop().expect("pending squad");
            if s.is_valid() {
                self.update_squad(bot, &s);
            }
        }
        self.assign_roles_ticks.set(self.assign_roles_ticks.get() - 1);
        if self.assign_roles_ticks.get() <= 0 {
            self.assign_roles_ticks.set(self.info.assign_roles_interval);
            self.find_new_units();
        }
        self.min_attack_force_delay_ticks.set(self.min_attack_force_delay_ticks.get() - 1);
        if self.min_attack_force_delay_ticks.get() <= 0 {
            self.min_attack_force_delay_ticks.set(self.info.minimum_attack_force_delay);
            self.create_attack_force();
        }
    }

    fn find_new_units(&self) {
        let player = self.self_.owner();
        let w = self.self_.world();
        for a in w.actors_having_trait::<dyn IPositionable>() {
            if !a.is_owned_by(&player)
                || self.info.exclude_from_squads_types.contains(&a.info.name)
                || self.active_units.borrow().contains_key(&a.id)
            {
                continue;
            }
            if self.info.air_units_types.contains(&a.info.name) {
                let s = self
                    .squad_of_type(SquadType::Air)
                    .unwrap_or_else(|| self.register_new_squad(SquadType::Air, None));
                s.units.borrow_mut().insert(a.id, a.clone());
            } else if self.info.naval_units_types.contains(&a.info.name) {
                let s = self
                    .squad_of_type(SquadType::Naval)
                    .unwrap_or_else(|| self.register_new_squad(SquadType::Naval, None));
                s.units.borrow_mut().insert(a.id, a.clone());
            } else {
                self.units_hanging_around_the_base.borrow_mut().push(a.clone());
            }
            self.active_units.borrow_mut().insert(a.id, a);
        }
        self.notify_idle();
    }

    fn create_attack_force(&self) {
        let size = self.info.squad_size
            + self
                .self_
                .world()
                .bot_random
                .borrow_mut()
                .next_max(self.info.squad_size_random_bonus);
        if self.units_hanging_around_the_base.borrow().len() as i32 >= size {
            let s = self.register_new_squad(SquadType::Assault, None);
            let units = std::mem::take(&mut *self.units_hanging_around_the_base.borrow_mut());
            for a in units {
                s.units.borrow_mut().insert(a.id, a);
            }
            self.notify_idle();
        }
    }

    fn try_to_rush_attack(&self) {
        let info = &self.info;
        let w = self.self_.world();
        let own: Vec<ActorRef> = self
            .active_units
            .borrow()
            .values()
            .filter(|u| {
                u.is_idle()
                    && u.info.has_trait_info::<AttackBaseInfo>()
                    && !info.air_units_types.contains(&u.info.name)
                    && !info.naval_units_types.contains(&u.info.name)
                    && !info.exclude_from_squads_types.contains(&u.info.name)
            })
            .cloned()
            .collect();
        if (own.len() as i32) < info.squad_size {
            return;
        }
        let bases = self.find_enemies(self.construction_yard_buildings.actors(), &own[0]);
        for base in bases {
            let candidates: Vec<ActorRef> = w
                .find_actors_in_circle(base.0.center_position(), WDist::from_cells(info.rush_attack_scan_radius))
                .into_iter()
                .filter(|u| {
                    u.info.has_trait_info::<AttackBaseInfo>()
                        && !info.air_units_types.contains(&u.info.name)
                        && !info.naval_units_types.contains(&u.info.name)
                })
                .collect();
            let enemies = self.find_enemies(candidates, &own[0]);
            let enemy_actors: Vec<ActorRef> = enemies.iter().map(|e| e.0.clone()).collect();
            if AttackOrFleeFuzzy::Rush.can_attack(&own, &enemy_actors) {
                let target = random_or_default(&enemies, &w.bot_random).unwrap_or(base);
                let rush = self
                    .squad_of_type(SquadType::Rush)
                    .unwrap_or_else(|| self.register_new_squad(SquadType::Rush, Some(target)));
                for a in &own {
                    rush.units.borrow_mut().insert(a.id, a.clone());
                }
                return;
            }
        }
    }

    fn protect_own(&self, _bot: &ModularBot, attacker: &ActorRef) {
        let w = self.self_.world();
        let sq = self
            .squad_of_type(SquadType::Protection)
            .unwrap_or_else(|| self.register_new_squad(SquadType::Protection, Some((attacker.clone(), WVec::new(0, 0, 0)))));
        let dead: Vec<u32> = sq
            .units
            .borrow()
            .values()
            .filter(|a| self.unit_cannot_be_ordered(a))
            .map(|a| a.id)
            .collect();
        for id in dead {
            sq.units.borrow_mut().remove(&id);
        }
        if sq.is_valid() {
            let center = sq.center_unit();
            if !self.is_target_valid(&sq, center.as_ref()) {
                sq.set_actor_to_target(Some((attacker.clone(), WVec::new(0, 0, 0))));
            }
        }
        if !sq.is_valid() {
            let player = self.self_.owner();
            let pf = w.world_actor().trait_::<PathFinder>();
            let map = w.map();
            let target_cell = map.cell_containing(attacker.center_position());
            for u in w.find_actors_in_circle(
                map.center_of_cell(self.random_base_center()),
                WDist::from_cells(self.info.protect_unit_scan_radius),
            ) {
                if !u.is_owned_by(&player)
                    || self.info.protection_types.contains(&u.info.name)
                    || !u.info.has_trait_info::<AttackBaseInfo>()
                {
                    continue;
                }
                let reachable = u.info.has_trait_info::<AircraftInfo>()
                    || u.trait_or_default::<Mobile>().is_some_and(|m| {
                        pf.path_exists_for_locomotor(&m.locomotor(), target_cell, map.cell_containing(u.center_position()))
                    });
                if reachable {
                    sq.units.borrow_mut().insert(u.id, u);
                }
            }
        }
    }

    /// `Squad.IsTargetValid`.
    fn is_target_valid(&self, sq: &Squad, squad_unit: Option<&ActorRef>) -> bool {
        let ta = sq.target_actor.borrow().clone();
        let Some(ta) = ta else { return false };
        let target = sq.target.borrow().clone();
        let valid = ta.is_in_world()
            && !ta.is_dead()
            && sq.units.borrow().values().any(|u| target.is_valid_for(u))
            && !ta.info.has_trait_info::<HuskInfo>();
        if !valid {
            return false;
        }
        let found = squad_unit.and_then(|u| self.find_enemies(vec![ta], u).into_iter().next());
        sq.set_actor_to_target(found.clone());
        found.is_some()
    }

    fn change_state(&self, sq: &Squad, new: State) {
        let old = std::mem::replace(&mut *sq.state.borrow_mut(), new);
        if matches!(old, State::GroundFlee | State::ProtectionFlee) {
            self.unregister_squad(sq);
        }
    }

    fn update_squad(&self, bot: &ModularBot, sq: &Rc<Squad>) {
        if !sq.is_valid() {
            return;
        }
        let is = |f: fn(&State) -> bool| f(&sq.state.borrow());
        if is(|s| matches!(s, State::GroundIdle { .. })) {
            self.ground_idle_tick(bot, sq);
        } else if is(|s| matches!(s, State::GroundAttackMove { .. })) {
            self.ground_attack_move_tick(bot, sq);
        } else if is(|s| matches!(s, State::GroundAttack { .. })) {
            self.ground_attack_tick(bot, sq);
        } else if is(|s| matches!(s, State::GroundFlee)) {
            self.go_to_random_own_building(bot, sq);
            self.change_state(sq, State::GroundIdle { leader: None });
        } else if is(|s| matches!(s, State::AirIdle)) {
            self.air_idle_tick(sq);
        } else if is(|s| matches!(s, State::AirAttack)) {
            self.air_attack_tick(bot, sq);
        } else if is(|s| matches!(s, State::AirFlee)) {
            self.air_flee_tick(bot, sq);
        } else if is(|s| matches!(s, State::ProtectionIdle)) {
            self.change_state(
                sq,
                State::ProtectionAttack {
                    leader: None,
                    backoff: BACKOFF_TICKS,
                },
            );
        } else if is(|s| matches!(s, State::ProtectionAttack { .. })) {
            self.protection_attack_tick(bot, sq);
        } else {
            self.go_to_random_own_building(bot, sq);
            self.change_state(sq, State::ProtectionIdle);
        }
    }

    /// `GroundStateBase.Leader`: the stored leader of the current state, elected again when it left the squad.
    fn leader(&self, sq: &Squad) -> ActorRef {
        let current = match &*sq.state.borrow() {
            State::GroundIdle { leader }
            | State::GroundAttackMove { leader, .. }
            | State::GroundAttack { leader, .. }
            | State::ProtectionAttack { leader, .. } => leader.clone(),
            _ => None,
        };
        if let Some(l) = current.filter(|l| sq.units.borrow().contains_key(&l.id)) {
            return l;
        }
        let l = new_leader(sq);
        match &mut *sq.state.borrow_mut() {
            State::GroundIdle { leader }
            | State::GroundAttackMove { leader, .. }
            | State::GroundAttack { leader, .. }
            | State::ProtectionAttack { leader, .. } => *leader = Some(l.clone()),
            _ => {}
        }
        l
    }

    fn clear_leader(&self, sq: &Squad) {
        if let State::GroundIdle { leader }
        | State::GroundAttackMove { leader, .. }
        | State::GroundAttack { leader, .. }
        | State::ProtectionAttack { leader, .. } = &mut *sq.state.borrow_mut()
        {
            *leader = None;
        }
    }

    fn new_leader_and_find_closest_enemy(&self, sq: &Squad) -> Option<(ActorRef, WVec)> {
        self.clear_leader(sq);
        let l = self.leader(sq);
        self.find_closest_enemy(&l)
    }

    /// `StateBase.ShouldFlee`.
    fn should_flee(&self, sq: &Squad, flee: impl Fn(&[ActorRef]) -> bool) -> bool {
        if !sq.is_valid() {
            return false;
        }
        let w = self.self_.world();
        let player = self.self_.owner();
        let units = w.find_actors_in_circle(sq.center_position(), WDist::from_cells(self.info.danger_scan_radius));
        if units
            .iter()
            .any(|u| u.is_owned_by(&player) && u.info.has_trait_info::<BuildingInfo>())
        {
            return false;
        }
        let enemies: Vec<ActorRef> = units
            .into_iter()
            .filter(|u| self.is_preferred_enemy_unit(Some(u)) && u.info.has_trait_info::<AttackBaseInfo>())
            .collect();
        if enemies.is_empty() {
            return false;
        }
        flee(&enemies)
    }

    fn ground_should_flee(&self, sq: &Squad) -> bool {
        let own = sq.unit_list();
        self.should_flee(sq, |enemies| !AttackOrFleeFuzzy::Default.can_attack(&own, enemies))
    }

    fn random_building_location(&self, sq: &Squad) -> CPos {
        let _ = sq;
        let w = self.self_.world();
        let player = self.self_.owner();
        let location = self.random_base_center();
        let buildings: Vec<ActorRef> = w
            .actors_having_trait::<Building>()
            .into_iter()
            .filter(|a| a.is_owned_by(&player))
            .collect();
        random_or_default(&buildings, &w.bot_random).map_or(location, |b| b.location())
    }

    fn go_to_random_own_building(&self, bot: &ModularBot, sq: &Squad) {
        let w = self.self_.world();
        let loc = self.random_building_location(sq);
        for a in sq.unit_list() {
            bot.queue_order(Order::new("Move", Some(a), Target::from_cell(&w, loc, FULL_CELL), false));
        }
    }

    fn grouped_attack_move(&self, bot: &ModularBot, target: Target, units: Vec<ActorRef>) {
        let mut o = Order::new("AttackMove", None, target, false);
        o.grouped_actors = Some(units.into_iter().map(Some).collect());
        bot.queue_order(o);
    }

    fn ground_idle_tick(&self, bot: &ModularBot, sq: &Rc<Squad>) {
        let leader = self.leader(sq);
        if !self.is_target_valid(sq, Some(&leader)) {
            let closest = self.new_leader_and_find_closest_enemy(sq);
            sq.set_actor_to_target(closest.clone());
            if closest.is_none() {
                return;
            }
        }
        let w = self.self_.world();
        let center = sq.target.borrow().center_position();
        let around = w.find_actors_in_circle(center, WDist::from_cells(self.info.idle_scan_radius));
        let leader = self.leader(sq);
        let enemies: Vec<ActorRef> = self.find_enemies(around, &leader).into_iter().map(|e| e.0).collect();
        if enemies.is_empty() {
            return;
        }
        if AttackOrFleeFuzzy::Default.can_attack(&sq.unit_list(), &enemies) {
            let t = sq.target.borrow().clone();
            self.grouped_attack_move(bot, t, sq.unit_list());
            self.change_state(sq, State::ground_attack_move());
        } else {
            self.change_state(sq, State::GroundFlee);
        }
    }

    /// The shared start of the attack move and attack states. Returns the leader, or `None` when the state changed.
    fn ground_attack_prelude(&self, sq: &Rc<Squad>) -> Option<ActorRef> {
        let leader = self.leader(sq);
        if !self.is_target_valid(sq, Some(&leader)) {
            let closest = self.new_leader_and_find_closest_enemy(sq);
            sq.set_actor_to_target(closest.clone());
            if closest.is_none() {
                self.change_state(sq, State::GroundFlee);
                return None;
            }
        }
        let leader = self.leader(sq);
        let tick = self.self_.world().world_tick();
        let target_id = sq.target_actor.borrow().as_ref().map(|a| a.id);
        let stalled = match &mut *sq.state.borrow_mut() {
            State::GroundAttackMove {
                last_updated_tick,
                last_leader_location,
                last_target,
                ..
            }
            | State::GroundAttack {
                last_updated_tick,
                last_leader_location,
                last_target,
                ..
            } => {
                if Some(leader.location()) != *last_leader_location {
                    *last_leader_location = Some(leader.location());
                    *last_updated_tick = tick;
                }
                if target_id != *last_target {
                    *last_target = target_id;
                    *last_updated_tick = tick;
                }
                tick > *last_updated_tick + 63
            }
            _ => false,
        };
        if stalled {
            self.change_state(sq, State::GroundIdle { leader: None });
            return None;
        }
        Some(leader)
    }

    fn ground_attack_move_tick(&self, bot: &ModularBot, sq: &Rc<Squad>) {
        let Some(leader) = self.ground_attack_prelude(sq) else { return };
        let w = self.self_.world();
        let radius = WDist::new(WDist::from_cells(sq.units.borrow().len() as i32).length / 3);
        let own: IndexSet<u32> = w
            .find_actors_in_circle(leader.center_position(), radius)
            .into_iter()
            .filter(|a| sq.units.borrow().contains_key(&a.id))
            .map(|a| a.id)
            .collect();
        if own.len() < sq.units.borrow().len() {
            bot.queue_order(Order::new("Stop", Some(leader.clone()), Target::Invalid, false));
            let units: Vec<ActorRef> = sq.unit_list().into_iter().filter(|a| !own.contains(&a.id)).collect();
            self.grouped_attack_move(bot, Target::from_cell(&w, leader.location(), FULL_CELL), units);
        } else {
            match self.find_closest_enemy_within(&leader, WDist::from_cells(self.info.attack_scan_radius)) {
                Some(t) => {
                    sq.set_actor_to_target(Some(t));
                    self.change_state(sq, State::ground_attack());
                }
                None => {
                    let t = sq.target.borrow().clone();
                    self.grouped_attack_move(bot, t, sq.unit_list());
                }
            }
        }
        if self.ground_should_flee(sq) {
            self.change_state(sq, State::GroundFlee);
        }
    }

    fn ground_attack_tick(&self, bot: &ModularBot, sq: &Rc<Squad>) {
        if self.ground_attack_prelude(sq).is_none() {
            return;
        }
        for a in sq.unit_list() {
            if !busy_attack(&a) {
                let t = sq.target.borrow().clone();
                bot.queue_order(Order::new("AttackMove", Some(a), t, false));
            }
        }
        if self.ground_should_flee(sq) {
            self.change_state(sq, State::GroundFlee);
        }
    }

    fn protection_attack_tick(&self, bot: &ModularBot, sq: &Rc<Squad>) {
        let leader = self.leader(sq);
        if !self.is_target_valid(sq, Some(&leader)) {
            let t = self.find_closest_enemy_within(&leader, WDist::from_cells(self.info.protection_scan_radius));
            sq.set_actor_to_target(t.clone());
            if t.is_none() {
                self.change_state(sq, State::ProtectionFlee);
                return;
            }
        }
        let player = self.self_.owner();
        let visible = sq
            .target_actor
            .borrow()
            .as_ref()
            .is_some_and(|a| a.can_be_viewed_by_player(Some(&player)));
        if !visible {
            let backoff = match &*sq.state.borrow() {
                State::ProtectionAttack { backoff, .. } => *backoff,
                _ => 0,
            };
            if backoff < 0 {
                self.change_state(sq, State::ProtectionFlee);
                return;
            }
            if let State::ProtectionAttack { backoff, .. } = &mut *sq.state.borrow_mut() {
                *backoff -= 1;
            }
        } else {
            let t = sq.target.borrow().clone();
            self.grouped_attack_move(bot, t, sq.unit_list());
        }
    }

    fn count_anti_air_units(&self, units: &[ActorRef]) -> i32 {
        let mut count = 0;
        for u in units {
            if u.info.has_trait_info::<AircraftInfo>() {
                continue;
            }
            for ab in u.traits_implementing::<AttackBase>() {
                if ab.is_trait_disabled() || ab.is_trait_paused() {
                    continue;
                }
                if ab
                    .armaments()
                    .iter()
                    .any(|a| a.weapon.is_valid_target(&self.info.aircraft_target_type))
                {
                    count += 1;
                }
            }
        }
        count
    }

    fn near_to_pos_safely(&self, sq: &Squad, loc: WPos) -> (bool, Option<ActorRef>) {
        let w = self.self_.world();
        let around: Vec<ActorRef> = w
            .find_actors_in_circle(loc, WDist::from_cells(self.info.danger_scan_radius))
            .into_iter()
            .filter(|u| self.is_preferred_enemy_unit(Some(u)))
            .collect();
        if around.is_empty() {
            return (true, None);
        }
        if self.count_anti_air_units(&around) * MISSILE_UNIT_MULTIPLIER < sq.units.borrow().len() as i32 {
            return (true, random_or_default(&around, &w.bot_random));
        }
        (false, None)
    }

    fn find_defenseless_target(&self, sq: &Squad) -> Option<ActorRef> {
        let w = self.self_.world();
        let map = w.map();
        let r = self.info.danger_scan_radius;
        let (cols, rows) = ((map.map_size.0 + r - 1) / r, (map.map_size.1 + r - 1) / r);
        for i in shuffle((0..cols * rows).collect(), &w.bot_random) {
            let pos = rustora_core::world_types::MPos::new(i % cols * r + r / 2, i / cols * r + r / 2).to_cpos(map.grid.grid_type);
            let (safe, target) = self.near_to_pos_safely(sq, map.center_of_cell(pos));
            if safe {
                if target.is_none() {
                    continue;
                }
                return target;
            }
        }
        None
    }

    fn air_should_flee(&self, sq: &Squad) -> bool {
        let n = sq.units.borrow().len() as i32;
        self.should_flee(sq, |enemies| self.count_anti_air_units(enemies) * MISSILE_UNIT_MULTIPLIER > n)
    }

    fn air_idle_tick(&self, sq: &Rc<Squad>) {
        if self.air_should_flee(sq) {
            self.change_state(sq, State::AirFlee);
            return;
        }
        let Some(e) = self.find_defenseless_target(sq) else { return };
        sq.set_actor_to_target(Some((e, WVec::new(0, 0, 0))));
        self.change_state(sq, State::AirAttack);
    }

    fn air_attack_tick(&self, bot: &ModularBot, sq: &Rc<Squad>) {
        let leader = sq.center_unit();
        if !self.is_target_valid(sq, leader.as_ref()) {
            let closest = leader.as_ref().and_then(|l| self.find_closest_enemy(l));
            sq.set_actor_to_target(closest.clone());
            if closest.is_none() {
                self.change_state(sq, State::AirFlee);
                return;
            }
        }
        let center = sq.target.borrow().center_position();
        if !self.near_to_pos_safely(sq, center).0 {
            self.change_state(sq, State::AirFlee);
            return;
        }
        let target_actor = sq.target_actor.borrow().clone();
        for a in sq.unit_list() {
            if busy_attack(&a) {
                continue;
            }
            let pools = a.traits_implementing::<AmmoPool>();
            if !reloads_automatically(&pools, a.trait_or_default::<Rearmable>().as_deref()) {
                if is_rearming(&a) {
                    continue;
                }
                if !pools.iter().all(|p| p.has_ammo()) {
                    bot.queue_order(Order::new("ReturnToBase", Some(a), Target::Invalid, false));
                    continue;
                }
            }
            if target_actor.as_ref().is_some_and(|t| can_attack_target(&a, t)) {
                let t = sq.target.borrow().clone();
                bot.queue_order(Order::new("Attack", Some(a), t, false));
            }
        }
    }

    fn air_flee_tick(&self, bot: &ModularBot, sq: &Rc<Squad>) {
        let w = self.self_.world();
        for a in sq.unit_list() {
            let pools = a.traits_implementing::<AmmoPool>();
            if !reloads_automatically(&pools, a.trait_or_default::<Rearmable>().as_deref()) && !pools.iter().all(|p| p.has_full_ammo()) {
                if is_rearming(&a) {
                    continue;
                }
                bot.queue_order(Order::new("ReturnToBase", Some(a), Target::Invalid, false));
                continue;
            }
            let loc = self.random_building_location(sq);
            bot.queue_order(Order::new("Move", Some(a), Target::from_cell(&w, loc, FULL_CELL), false));
        }
        self.change_state(sq, State::AirIdle);
    }
}

/// `GroundStateBase.NewLeader`: among the units with the fewest terrain speeds, the one closest to their center.
fn new_leader(sq: &Squad) -> ActorRef {
    let units = sq.unit_list();
    let count = |a: &ActorRef| a.trait_or_default::<Mobile>().map(|m| m.locomotor().info.terrain_speeds.len());
    let least = units.iter().filter_map(count).min();
    let candidates: Vec<ActorRef> = match least {
        Some(l) => units.iter().filter(|a| count(a) == Some(l)).cloned().collect(),
        None => units,
    };
    let center = average(candidates.iter().map(|a| a.center_position()));
    candidates
        .into_iter()
        .min_by_key(|a| (a.center_position() - center).length_squared())
        .expect("squad unit")
}

impl SquadType {
    fn parse(text: &str) -> Option<SquadType> {
        match text {
            "Assault" => Some(SquadType::Assault),
            "Air" => Some(SquadType::Air),
            "Rush" => Some(SquadType::Rush),
            "Protection" => Some(SquadType::Protection),
            "Naval" => Some(SquadType::Naval),
            _ => None,
        }
    }
}

impl Squad {
    /// `Squad.Serialize`.
    fn serialize(&self) -> MiniYaml {
        let mut nodes = vec![
            MiniYamlNode::with_value_str("Type", format!("{:?}", self.ty)),
            MiniYamlNode::with_value_str("Units", format_ids(self.units.borrow().values().map(|a| a.id))),
        ];
        let target = self.target.borrow();
        if let (true, Some(actor)) = (target.target_type() != TargetType::Invalid, self.target_actor.borrow().as_ref()) {
            nodes.push(MiniYamlNode::with_value_str("ActorToTarget", actor.id.to_string()));
            nodes.push(MiniYamlNode::with_value_str(
                "TargetOffset",
                format_wvec(target.center_position() - actor.center_position()),
            ));
        }
        MiniYaml::new(Some(String::new()), nodes)
    }

    pub fn is_valid(&self) -> bool {
        !self.units.borrow().is_empty()
    }

    fn unit_list(&self) -> Vec<ActorRef> {
        self.units.borrow().values().cloned().collect()
    }

    fn set_actor_to_target(&self, target: Option<(ActorRef, WVec)>) {
        let Some((a, offset)) = target else {
            *self.target_actor.borrow_mut() = None;
            *self.target.borrow_mut() = Target::Invalid;
            return;
        };
        *self.target.borrow_mut() = if offset == WVec::new(0, 0, 0) {
            Target::from_actor(Some(&a))
        } else {
            Target::from_pos(a.center_position() + offset)
        };
        *self.target_actor.borrow_mut() = Some(a);
    }

    fn center_position(&self) -> WPos {
        average(self.units.borrow().values().map(|a| a.center_position()))
    }

    fn center_unit(&self) -> Option<ActorRef> {
        let c = self.center_position();
        self.units
            .borrow()
            .values()
            .min_by_key(|a| (a.center_position() - c).length_squared())
            .cloned()
    }
}

/// `StateBase.BusyAttack`.
fn busy_attack(a: &ActorRef) -> bool {
    if a.is_idle() {
        return false;
    }
    let Some(act) = a.current_activity() else { return false };
    let is_attack = |t: &str| t == "Attack" || t == "FlyAttack";
    if is_attack(act.type_name()) {
        return true;
    }
    act.base().next_activity().is_some_and(|n| is_attack(n.type_name()))
}

/// `StateBase.CanAttackTarget`.
fn can_attack_target(a: &ActorRef, target: &ActorRef) -> bool {
    if !a.info.has_trait_info::<AttackBaseInfo>() {
        return false;
    }
    let types = rustora_engine::actor_ext::enabled_target_types(target);
    if types.is_empty() {
        return false;
    }
    a.traits_implementing::<Armament>()
        .iter()
        .any(|arm| !arm.is_trait_disabled() && arm.weapon.is_valid_target(&types))
}

fn has_activity(a: Option<rustora_engine::activity::ActivityRef>, name: &str) -> bool {
    let Some(a) = a else { return false };
    has_activity(a.base().child_activity(), name) || a.type_name() == name || has_activity(a.base().next_activity(), name)
}

/// `StateBase.IsRearming`.
fn is_rearming(a: &ActorRef) -> bool {
    !a.is_idle() && (has_activity(a.current_activity(), "Resupply") || has_activity(a.current_activity(), "ReturnToBase"))
}

/// `StateBase.ReloadsAutomatically`.
fn reloads_automatically(pools: &[Rc<AmmoPool>], rearmable: Option<&Rearmable>) -> bool {
    let Some(r) = rearmable else { return true };
    pools.iter().all(|p| r.info.ammo_pools.contains(&p.info.name))
}
