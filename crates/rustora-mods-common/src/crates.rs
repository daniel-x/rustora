// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/World/CrateSpawner.cs` and `Traits/Crates`.
//!
//! The C# `CrateAction` subclasses become one struct with an [`ActionKind`], and their infos one union struct.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_core::world_types::{CPos, CVec, WAngle, WDist, WPos, WRot, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_ext::enabled_target_types;
use rustora_engine::actor_info::{ActorInfo, TraitNode};
use rustora_engine::actor_init::{ActorInits, CenterPositionInit, FacingInit, LocationInit, OwnerInit};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::{Damage, DamageTypes};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::Target;
use rustora_engine::terrain::TargetableType;
use rustora_engine::traits::subcell::{ANY, FULL_CELL, INVALID};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::activities::Parachute;
use crate::actor_map::WorldExt;
use crate::aircraft::{AircraftInfo, Fly, RemoveSelf};
use crate::base::BaseBuilding;
use crate::cargo::{Cargo, ParaDrop};
use crate::interfaces::*;
use crate::locomotor::{BlockedByActor, Locomotor, locomotor_by_name};
use crate::mobile::MobileInfo;
use crate::parachutable::Parachutable;
use crate::path_finder_trait::PathFinder;
use crate::player::PlayerResources;
use crate::production::{TechTree, ValuedInfo};
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("CrateSpawner", |n| load_info!(CrateSpawnerInfo, n));
    r.register("Crate", |n| load_info!(CrateInfo, n));
    r.register("CrateAction", |n| load_action(n, ActionKind::Plain));
    r.register("GiveCashCrateAction", |n| load_action(n, ActionKind::GiveCash));
    r.register("LevelUpCrateAction", |n| load_action(n, ActionKind::LevelUp));
    r.register("ExplodeCrateAction", |n| load_action(n, ActionKind::Explode));
    r.register("HideMapCrateAction", |n| load_action(n, ActionKind::HideMap));
    r.register("HealActorsCrateAction", |n| load_action(n, ActionKind::HealActors));
    r.register("RevealMapCrateAction", |n| load_action(n, ActionKind::RevealMap));
    r.register("DuplicateUnitCrateAction", |n| load_action(n, ActionKind::DuplicateUnit));
    r.register("GiveBaseBuilderCrateAction", |n| load_action(n, ActionKind::GiveBaseBuilder));
    r.register("GiveUnitCrateAction", |n| load_action(n, ActionKind::GiveUnit));
    r.register("GrantExternalConditionCrateAction", |n| {
        load_action(n, ActionKind::GrantExternalCondition)
    });
    r.register("SupportPowerCrateAction", |n| load_action(n, ActionKind::SupportPower));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CrateSpawnerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub checkbox_enabled: bool,
    pub minimum: i32,
    pub maximum: i32,
    pub spawn_interval: i32,
    pub initial_spawn_delay: i32,
    pub valid_ground: IndexSet<String>,
    pub valid_water: IndexSet<String>,
    pub water_chance: i32,
    pub crate_actors: Vec<String>,
    pub crate_actor_shares: Vec<i32>,
    pub delivery_aircraft: Option<String>,
    pub quantized_facings: i32,
    pub cordon: WDist,
}

impl Default for CrateSpawnerInfo {
    fn default() -> Self {
        CrateSpawnerInfo {
            instance_name: None,
            checkbox_enabled: true,
            minimum: 1,
            maximum: 255,
            spawn_interval: 4500,
            initial_spawn_delay: 0,
            valid_ground: ["Clear", "Rough", "Road", "Ore", "Beach"].iter().map(|s| s.to_string()).collect(),
            valid_water: IndexSet::from(["Water".to_string()]),
            water_chance: 20,
            crate_actors: vec!["crate".to_string()],
            crate_actor_shares: vec![10],
            delivery_aircraft: None,
            quantized_facings: 32,
            cordon: WDist::new(5120),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct CrateSpawner {
    info: Rc<CrateSpawnerInfo>,
    enabled: Cell<bool>,
    crates: Cell<i32>,
    ticks: Cell<i32>,
}

register_traits!(CrateSpawner: ITick, INotifyCreated);
info_impl!(CrateSpawnerInfo => |info, _init| {
    let ticks = info.initial_spawn_delay;
    Some(Rc::new(CrateSpawner { info, enabled: Cell::new(false), crates: Cell::new(0), ticks: Cell::new(ticks) }))
});

impl INotifyCreated for CrateSpawner {
    fn created(&self, self_: &ActorRef) {
        self.enabled.set(
            self_
                .world()
                .lobby_info
                .global_settings
                .option_bool("crates", self.info.checkbox_enabled),
        );
    }
}

impl ITick for CrateSpawner {
    fn tick(&self, self_: &ActorRef) {
        if !self.enabled.get() {
            return;
        }
        self.ticks.set(self.ticks.get() - 1);
        if self.ticks.get() <= 0 {
            self.ticks.set(self.info.spawn_interval);
            let crates = self.crates.get();
            let to_spawn = 0.max(self.info.minimum - crates)
                + if crates < self.info.maximum && self.info.maximum > self.info.minimum {
                    1
                } else {
                    0
                };
            for _ in 0..to_spawn {
                self.spawn_crate(self_);
            }
        }
    }
}

impl CrateSpawner {
    fn spawn_crate(&self, self_: &ActorRef) {
        let w = self_.world();
        let in_water = w.shared_random.borrow_mut().next_max(100) < self.info.water_chance;
        let Some(p) = self.choose_drop_cell(&w, in_water, 100) else {
            return;
        };
        let crate_actor = self.choose_crate_actor(&w);
        let info = self.info.clone();
        let me = self_.clone();
        w.add_frame_end_task(Box::new(move |w| {
            let crate_actor = crate_actor.expect("crate actor share");
            let Some(delivery) = &info.delivery_aircraft else {
                w.create_actor(
                    &crate_actor,
                    ActorInits::new()
                        .with(OwnerInit::from_player(&w.world_actor().owner()))
                        .with(LocationInit::new(p)),
                );
                return;
            };
            let crate_ = w.create_actor_add(
                false,
                &crate_actor,
                ActorInits::new().with(OwnerInit::from_player(&w.world_actor().owner())),
            );
            let facing_index = w.shared_random.borrow_mut().next_max(info.quantized_facings);
            let drop_facing = WAngle::new(1024 * facing_index / info.quantized_facings);
            let delta = WVec::new(0, -1024, 0).rotate(&WRot::from_yaw(drop_facing));
            let altitude = w
                .rules()
                .actor(delivery)
                .expect("delivery aircraft")
                .trait_info::<AircraftInfo>()
                .expect("aircraft info")
                .cruise_altitude
                .length;
            let target: WPos = w.map().center_of_cell(p) + WVec::new(0, 0, altitude);
            let start_edge = target - delta * (w.map().distance_to_edge(target, -delta) + info.cordon).length / 1024;
            let finish_edge = target + delta * (w.map().distance_to_edge(target, delta) + info.cordon).length / 1024;
            let plane = w.create_actor(
                delivery,
                ActorInits::new()
                    .with(CenterPositionInit::new(start_edge))
                    .with(OwnerInit::from_player(&me.owner()))
                    .with(FacingInit::new(drop_facing)),
            );
            plane.trait_::<ParaDrop>().set_lz(p, true);
            plane.trait_::<Cargo>().load(&plane, &crate_);
            plane.queue_activity_queued(false, Fly::new(&plane, Target::from_pos(finish_edge)));
            plane.queue_activity(RemoveSelf::new());
        }));
    }

    fn choose_drop_cell(&self, w: &Rc<World>, in_water: bool, max_tries: i32) -> Option<CPos> {
        for _ in 0..max_tries {
            let p = w.map().choose_random_cell(&mut w.shared_random.borrow_mut());
            let terrain = &w.map().terrain_type_info(p).ty;
            if !(if in_water {
                &self.info.valid_water
            } else {
                &self.info.valid_ground
            })
            .contains(terrain)
            {
                continue;
            }
            if w.actor_map().any_actors_at(p) {
                continue;
            }
            return Some(p);
        }
        None
    }

    fn choose_crate_actor(&self, w: &Rc<World>) -> Option<String> {
        let shares = &self.info.crate_actor_shares;
        let n = w.shared_random.borrow_mut().next_max(shares.iter().sum());
        let mut cumulative = 0;
        for (i, s) in shares.iter().enumerate() {
            cumulative += s;
            if n <= cumulative {
                return Some(self.info.crate_actors[i].clone());
            }
        }
        None
    }

    pub fn increment_crates(&self) {
        self.crates.set(self.crates.get() + 1);
    }

    pub fn decrement_crates(&self) {
        self.crates.set(self.crates.get() - 1);
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CrateInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub duration: i32,
    pub terrain_types: IndexSet<String>,
    pub crush_class: String,
}

impl Default for CrateInfo {
    fn default() -> Self {
        CrateInfo {
            instance_name: None,
            duration: 0,
            terrain_types: IndexSet::new(),
            crush_class: "crate".to_string(),
        }
    }
}

impl CrateInfo {
    pub fn can_exist_in_cell(&self, w: &World, cell: CPos) -> bool {
        w.map().contains(cell) && self.terrain_types.contains(&w.map().terrain_type_info(cell).ty)
    }

    pub fn get_available_subcell(&self, w: &World, cell: CPos, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> u8 {
        if !self.can_exist_in_cell(w, cell) {
            return INVALID;
        }
        if check == BlockedByActor::None {
            return FULL_CELL;
        }
        if w.actor_map()
            .get_actors_at(cell)
            .iter()
            .any(|x| ignore_actor.is_none_or(|i| i.id != x.id))
        {
            INVALID
        } else {
            FULL_CELL
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct Crate {
    info: Rc<CrateInfo>,
    self_: ActorRef,
    collected: Cell<bool>,
    notify_center_position_changed: RefCell<Option<Vec<Rc<dyn INotifyCenterPositionChanged>>>>,
    ticks: Cell<i32>,
    location: Cell<CPos>,
    center_position: Cell<WPos>,
}

register_traits!(
    Crate: ITick,
    IOccupySpace,
    IPositionable,
    ICrushable,
    ISync,
    INotifyCreated,
    INotifyParachute,
    INotifyAddedToWorld,
    INotifyRemovedFromWorld,
    INotifyCrushed
);
info_impl!(CrateInfo => |info, init| Some(Rc::new(Crate {
    info,
    self_: init.self_.clone(),
    collected: Cell::new(false),
    notify_center_position_changed: RefCell::new(None),
    ticks: Cell::new(0),
    location: Cell::new(init.inits.get::<LocationInit>().map_or(CPos::ZERO, |l| l.value)),
    center_position: Cell::new(WPos::ZERO),
})));

impl ISync for Crate {
    fn sync_hash(&self) -> i32 {
        self.ticks.get() ^ self.location.get().hash()
    }
}

impl Crate {
    fn set_location(&self, self_: &ActorRef, cell: CPos) {
        let m = self_.world().actor_map();
        m.remove_influence(self_, self);
        self.location.set(cell);
        m.add_influence(self_, self);
    }

    fn on_crush_inner(&self, crusher: &ActorRef) {
        if self.collected.get() {
            return;
        }
        let self_ = &self.self_;
        let actions = self_.traits_implementing::<CrateAction>();
        self_.dispose();
        self.collected.set(true);
        let shares: Vec<(Rc<CrateAction>, i32)> = actions
            .into_iter()
            .map(|a| {
                let s = a.get_selection_shares_outer(crusher);
                (a, s)
            })
            .collect();
        if shares.is_empty() {
            return;
        }
        let total: i32 = shares.iter().map(|s| s.1).sum();
        let mut n = self_.world().shared_random.borrow_mut().next_max(total);
        for (action, share) in shares {
            if n < share {
                action.activate(crusher);
                return;
            }
            n -= share;
        }
    }
}

impl INotifyCreated for Crate {
    fn created(&self, self_: &ActorRef) {
        self.set_position_cell(self_, self.location.get(), ANY);
        *self.notify_center_position_changed.borrow_mut() = Some(self_.traits_implementing::<dyn INotifyCenterPositionChanged>());
    }
}

impl INotifyCrushed for Crate {
    fn on_crush(&self, _self_: &ActorRef, crusher: &ActorRef, classes: BitSet<CrushClass>) {
        if classes.contains(&self.info.crush_class) {
            self.on_crush_inner(crusher);
        }
    }

    fn warn_crush(&self, _self_: &ActorRef, _crusher: &ActorRef, _classes: BitSet<CrushClass>) {}
}

impl INotifyParachute for Crate {
    fn on_parachute(&self, _self_: &ActorRef) {}

    fn on_landed(&self, self_: &ActorRef) {
        let w = self_.world();
        let mut any_other_actors = false;
        let mut collector = None;
        for other in w.actor_map().get_actors_at(self_.location()) {
            if self_.id == other.id {
                continue;
            }
            any_other_actors = true;
            let Some(mi) = other.info.trait_info::<MobileInfo>() else {
                continue;
            };
            if util::is_at_ground_level(self_) && mi.locomotor_info(&w).crushes.contains(&self.info.crush_class) {
                collector = Some(other);
                break;
            }
        }
        if !any_other_actors {
            return;
        }
        match collector {
            Some(c) => self.on_crush_inner(&c),
            None => self_.dispose(),
        }
    }
}

impl ITick for Crate {
    fn tick(&self, self_: &ActorRef) {
        if self.info.duration != 0 && self_.is_in_world() {
            self.ticks.set(self.ticks.get() + 1);
            if self.ticks.get() >= self.info.duration {
                self_.dispose();
            }
        }
    }
}

impl IOccupySpace for Crate {
    fn center_position(&self) -> WPos {
        self.center_position.get()
    }

    fn top_left(&self) -> CPos {
        self.location.get()
    }

    fn occupied_cells(&self) -> Vec<(CPos, u8)> {
        vec![(self.location.get(), FULL_CELL)]
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_leaving_cell(&self, location: CPos, _sub: u8) -> bool {
        self.self_.location() == location && self.ticks.get() + 1 == self.info.duration
    }
}

impl IPositionable for Crate {
    fn can_exist_in_cell(&self, location: CPos) -> bool {
        self.info.can_exist_in_cell(&self.self_.world(), location)
    }

    fn can_enter_cell(&self, location: CPos, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> bool {
        self.get_available_subcell(location, ANY, ignore_actor, check) != INVALID
    }

    fn get_valid_subcell(&self, _preferred: u8) -> u8 {
        FULL_CELL
    }

    fn get_available_subcell(&self, location: CPos, _preferred: u8, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> u8 {
        self.info.get_available_subcell(&self.self_.world(), location, ignore_actor, check)
    }

    fn set_position_cell(&self, self_: &ActorRef, cell: CPos, _sub: u8) {
        self.set_location(self_, cell);
        self.set_center_position(self_, self_.world().map().center_of_cell(cell));
    }

    fn set_position(&self, self_: &ActorRef, pos: WPos) {
        let w = self_.world();
        let cell = w.map().cell_containing(pos);
        self.set_location(self_, cell);
        let center = w.map().center_of_cell(cell) + WVec::from_dists(WDist::ZERO, WDist::ZERO, w.map().distance_above_terrain(pos));
        self.set_center_position(self_, center);
    }

    fn set_center_position(&self, self_: &ActorRef, pos: WPos) {
        self.center_position.set(pos);
        self_.world().update_maps(self_);
        let list = self.notify_center_position_changed.borrow().clone();
        for n in list.iter().flatten() {
            n.center_position_changed(self_, 0, 0);
        }
    }
}

impl ICrushable for Crate {
    fn crushable_by_actor(&self, _self_: &ActorRef, _crusher: &ActorRef, classes: BitSet<CrushClass>) -> bool {
        classes.contains(&self.info.crush_class)
    }

    fn crushable_by(&self, self_: &ActorRef, classes: BitSet<CrushClass>) -> u64 {
        if classes.contains(&self.info.crush_class) {
            self_.world().all_players_mask.get()
        } else {
            0
        }
    }
}

impl INotifyAddedToWorld for Crate {
    fn added_to_world(&self, self_: &ActorRef) {
        let w = self_.world();
        w.add_to_maps(self_, self);
        if let Some(s) = w.world_actor().trait_or_default::<CrateSpawner>() {
            s.increment_crates();
        }
        if w.map().distance_above_terrain(self.center_position.get()) > WDist::ZERO && self_.trait_or_default::<Parachutable>().is_some() {
            self_.queue_activity(Parachute::new(self_));
        }
    }
}

impl INotifyRemovedFromWorld for Crate {
    fn removed_from_world(&self, self_: &ActorRef) {
        let w = self_.world();
        w.remove_from_maps(self_, self);
        if let Some(s) = w.world_actor().trait_or_default::<CrateSpawner>() {
            s.decrement_crates();
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionKind {
    #[default]
    Plain,
    GiveCash,
    LevelUp,
    Explode,
    HideMap,
    HealActors,
    RevealMap,
    DuplicateUnit,
    GiveBaseBuilder,
    GiveUnit,
    GrantExternalCondition,
    SupportPower,
}

impl ActionKind {
    fn class_name(self) -> &'static str {
        match self {
            ActionKind::Plain => "CrateAction",
            ActionKind::GiveCash => "GiveCashCrateAction",
            ActionKind::LevelUp => "LevelUpCrateAction",
            ActionKind::Explode => "ExplodeCrateAction",
            ActionKind::HideMap => "HideMapCrateAction",
            ActionKind::HealActors => "HealActorsCrateAction",
            ActionKind::RevealMap => "RevealMapCrateAction",
            ActionKind::DuplicateUnit => "DuplicateUnitCrateAction",
            ActionKind::GiveBaseBuilder => "GiveBaseBuilderCrateAction",
            ActionKind::GiveUnit => "GiveUnitCrateAction",
            ActionKind::GrantExternalCondition => "GrantExternalConditionCrateAction",
            ActionKind::SupportPower => "SupportPowerCrateAction",
        }
    }
}

fn load_action(node: &TraitNode, kind: ActionKind) -> Result<Rc<dyn TraitInfo>, String> {
    let mut info = CrateActionInfo::default();
    rustora_core::fieldloader::FieldLoad::load(&mut info, &node.yaml).map_err(|e| format!("Trait name {}: {}", node.display_name(), e))?;
    info.instance_name = node.instance_name.clone();
    info.kind = kind;
    Ok(Rc::new(info))
}

/// Union of `CrateActionInfo` and the info fields of its subclasses.
#[derive(Clone, Debug, FieldLoad)]
pub struct CrateActionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub sound: Option<String>,
    pub notification: Option<String>,
    pub text_notification: Option<String>,
    #[field(ignore)]
    pub kind: ActionKind,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub selection_shares: i32,
    pub time_delay: i32,
    pub prerequisites: Vec<String>,
    pub excluded_actor_types: Vec<String>,
    pub amount: i32,
    pub levels: i32,
    pub range: WDist,
    pub max_extra_collectors: i32,
    pub weapon: Option<String>,
    pub include_allies: bool,
    pub target_types: BitSet<TargetableType>,
    pub max_amount: i32,
    pub min_amount: i32,
    pub max_duplicate_value: i32,
    pub max_radius: i32,
    pub valid_targets: BitSet<TargetableType>,
    pub valid_factions: IndexSet<String>,
    pub owner: Option<String>,
    pub units: Vec<String>,
    pub no_base_selection_shares: i32,
    pub condition: Option<String>,
    pub duration: i32,
    pub proxy: Option<String>,
}

impl Default for CrateActionInfo {
    fn default() -> Self {
        CrateActionInfo {
            sound: None,
            notification: None,
            text_notification: None,
            instance_name: None,
            kind: ActionKind::Plain,
            base: ConditionalTraitInfo::default(),
            selection_shares: 10,
            time_delay: 0,
            prerequisites: Vec::new(),
            excluded_actor_types: Vec::new(),
            amount: 2000,
            levels: 1,
            range: WDist::new(3),
            max_extra_collectors: 4,
            weapon: None,
            include_allies: false,
            target_types: BitSet::default(),
            max_amount: 2,
            min_amount: 1,
            max_duplicate_value: -1,
            max_radius: 4,
            valid_targets: BitSet::new(&["Ground", "Water"]),
            valid_factions: IndexSet::new(),
            owner: None,
            units: Vec::new(),
            no_base_selection_shares: 1000,
            condition: None,
            duration: 0,
            proxy: None,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct CrateAction {
    info: Rc<CrateActionInfo>,
    state: ConditionalState,
    self_: ActorRef,
}

impl TraitObject for CrateAction {
    fn register(self: Rc<Self>, dict: &rustora_engine::trait_dict::TraitDict, actor: &ActorRef) {
        dict.add::<CrateAction>(actor, self.clone());
        dict.add::<dyn INotifyCreated>(actor, self.clone());
        dict.add::<dyn IObservesVariables>(actor, self.clone());
        dict.add::<dyn IDisabledTrait>(actor, self.clone());
        dict.add::<dyn ISync>(actor, self);
    }

    fn type_name(&self) -> &'static str {
        self.info.kind.class_name()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    rustora_engine::trait_object_persist!(CrateAction: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
}

rustora_engine::persist_trait_refs!(CrateAction);

info_impl!(CrateActionInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(CrateAction { info, state, self_: init.self_.clone() }))
});
rustora_engine::conditional_simple!(CrateAction);

fn owner_init(info: &CrateActionInfo, collector: &ActorRef) -> OwnerInit {
    match &info.owner {
        Some(n) => OwnerInit::from_name(n),
        None => OwnerInit::from_player(&collector.owner()),
    }
}

/// Positions the created actor on a free subcell like `GiveUnitCrateAction` and `DuplicateUnitCrateAction`.
fn place_on_free_subcell(actor: &ActorRef, cell: CPos) {
    let positionable = actor.trait_::<dyn IPositionable>();
    let sub = positionable.get_available_subcell(cell, ANY, Some(actor), BlockedByActor::All);
    positionable.set_position_cell(actor, cell, sub);
}

fn info_can_enter_cell(a: &ActorInfo, w: &World, self_: &ActorRef, cell: CPos) -> bool {
    if let Some(m) = a.trait_info::<MobileInfo>() {
        return m.can_enter_cell(w, Some(self_), cell, FULL_CELL, None, BlockedByActor::All);
    }
    if let Some(i) = a.trait_info::<AircraftInfo>() {
        return i.can_enter_cell(w, cell, None, BlockedByActor::All);
    }
    panic!("{} has no supported IPositionableInfo", a.name);
}

impl CrateAction {
    pub fn get_selection_shares_outer(&self, collector: &ActorRef) -> i32 {
        if self.state.is_trait_disabled()
            || self.self_.world().world_tick() < self.info.time_delay
            || self.info.excluded_actor_types.contains(&collector.info.name)
        {
            return 0;
        }
        if !self.info.prerequisites.is_empty()
            && !collector
                .owner()
                .player_actor()
                .trait_::<TechTree>()
                .has_prerequisites(&self.info.prerequisites)
        {
            return 0;
        }
        self.get_selection_shares(collector)
    }

    fn valid_faction(&self, collector: &ActorRef) -> bool {
        !collector.owner().non_combatant
            && (self.info.valid_factions.is_empty() || self.info.valid_factions.contains(&collector.owner().faction))
    }

    fn duplicate_can_give_to(&self, collector: &ActorRef) -> bool {
        if !self.valid_faction(collector) || !self.info.valid_targets.overlaps(&enabled_target_types(collector)) {
            return false;
        }
        let Some(positionable) = collector.trait_or_default::<dyn IPositionable>() else {
            return false;
        };
        collector
            .world()
            .map()
            .find_tiles_in_circle(collector.location(), self.info.max_radius, false)
            .any(|c| positionable.can_enter_cell(c, None, BlockedByActor::All))
    }

    fn suitable_cells(&self, w: &World, near: CPos, unit: &str) -> Vec<CPos> {
        let rules = w.rules().actor(unit).unwrap_or_else(|| panic!("unknown actor {unit}"));
        let locomotor: Option<Rc<Locomotor>> = rules.trait_info::<MobileInfo>().map(|m| locomotor_by_name(w, &m.locomotor));
        let path_finder = w.world_actor().trait_::<PathFinder>();
        let mut out = Vec::new();
        for i in -1..=1 {
            for j in -1..=1 {
                let cell = near + CVec::new(i, j);
                if info_can_enter_cell(&rules, w, &self.self_, cell)
                    && locomotor
                        .as_ref()
                        .is_none_or(|l| path_finder.path_might_exist_for_locomotor_blocked_by_immovable(l, cell, near))
                {
                    out.push(cell);
                }
            }
        }
        out
    }

    fn give_unit_can_give_to(&self, collector: &ActorRef) -> bool {
        if !self.valid_faction(collector) {
            return false;
        }
        let w = collector.world();
        self.info
            .units
            .iter()
            .all(|u| !self.suitable_cells(&w, collector.location(), u).is_empty())
    }

    fn get_selection_shares(&self, collector: &ActorRef) -> i32 {
        let info = &self.info;
        match info.kind {
            ActionKind::GiveCash => {
                let pr = collector.owner().player_actor().trait_::<PlayerResources>();
                if info.amount < 0 && pr.get_cash_and_resources() == 0 {
                    0
                } else {
                    info.selection_shares
                }
            }
            ActionKind::LevelUp => {
                if crate::experience::can_gain_level(collector) {
                    info.selection_shares
                } else {
                    0
                }
            }
            ActionKind::HideMap => {
                let owner = collector.owner();
                let pa = owner.player_actor();
                let prevent_reset = pa
                    .traits_implementing::<dyn crate::gps::IPreventsShroudReset>()
                    .iter()
                    .any(|p| p.prevent_shroud_reset(pa));
                if prevent_reset || owner.shroud().explore_map_enabled() {
                    0
                } else {
                    info.selection_shares
                }
            }
            ActionKind::DuplicateUnit => {
                if self.duplicate_can_give_to(collector) {
                    info.selection_shares
                } else {
                    0
                }
            }
            ActionKind::GiveUnit => {
                if self.give_unit_can_give_to(collector) {
                    info.selection_shares
                } else {
                    0
                }
            }
            ActionKind::GiveBaseBuilder => {
                if !self.give_unit_can_give_to(collector) {
                    return 0;
                }
                let has_base = collector
                    .world()
                    .actors_having_trait::<BaseBuilding>()
                    .iter()
                    .any(|a| Rc::ptr_eq(&a.owner(), &collector.owner()));
                if has_base {
                    info.selection_shares
                } else {
                    info.no_base_selection_shares
                }
            }
            ActionKind::GrantExternalCondition => {
                if crate::experience::accepts_external_condition(collector, info.condition.as_deref().unwrap_or(""), &self.self_) {
                    info.selection_shares
                } else {
                    0
                }
            }
            ActionKind::Plain | ActionKind::Explode | ActionKind::HealActors | ActionKind::RevealMap | ActionKind::SupportPower => {
                info.selection_shares
            }
        }
    }

    pub fn activate(&self, collector: &ActorRef) {
        let info = self.info.clone();
        let w = collector.world();
        if let Some(s) = &info.sound {
            rustora_engine::sound::play_at(&w, rustora_engine::sound::SoundType::World, s, self.self_.center_position(), 1.0);
        }
        rustora_engine::sound::notify_owner(collector, "Speech", info.notification.as_deref(), info.text_notification.as_deref());
        match info.kind {
            ActionKind::Plain => {}
            ActionKind::GiveCash => {
                let c = collector.clone();
                w.add_frame_end_task(Box::new(move |_w| {
                    c.owner().player_actor().trait_::<PlayerResources>().change_cash(info.amount);
                }));
            }
            ActionKind::LevelUp => self.activate_level_up(collector),
            ActionKind::Explode => {
                crate::combat::weapon_impact(
                    info.weapon.as_ref().expect("Weapon"),
                    &Target::from_pos(collector.center_position()),
                    collector,
                );
            }
            ActionKind::HideMap => {
                self.for_players(collector, |p| p.shroud().reset_exploration());
            }
            ActionKind::RevealMap => {
                self.for_players(collector, |p| p.shroud().explore_all());
            }
            ActionKind::HealActors => {
                for (a, h) in w.actors_with_trait::<dyn IHealth>() {
                    if !Rc::ptr_eq(&a.owner(), &collector.owner()) {
                        continue;
                    }
                    if !h.is_dead() && (info.target_types.is_empty() || info.target_types.overlaps(&enabled_target_types(&a))) {
                        h.inflict_damage(&a, Some(&a), &Damage::new(-(h.max_hp() - h.hp()), DamageTypes::default()), true);
                    }
                }
            }
            ActionKind::DuplicateUnit => self.activate_duplicate(collector),
            ActionKind::GiveUnit | ActionKind::GiveBaseBuilder => self.activate_give_unit(collector),
            ActionKind::GrantExternalCondition => self.activate_grant_condition(collector),
            ActionKind::SupportPower => {
                let c = collector.clone();
                w.add_frame_end_task(Box::new(move |w| {
                    w.create_actor(
                        &info.proxy.as_ref().expect("Proxy").to_lowercase(),
                        ActorInits::new().with(OwnerInit::from_player(&c.owner())),
                    );
                }));
            }
        }
    }

    fn for_players(&self, collector: &ActorRef, f: impl Fn(&rustora_engine::player::PlayerRef)) {
        if self.info.include_allies {
            for p in collector.world().players() {
                if p.is_allied_with(Some(&collector.owner())) {
                    f(&p);
                }
            }
        } else {
            f(&collector.owner());
        }
    }

    fn activate_level_up(&self, collector: &ActorRef) {
        let w = collector.world();
        let mut in_range: Vec<ActorRef> = w
            .find_actors_in_circle(self.self_.center_position(), self.info.range)
            .into_iter()
            .filter(|a| a.id != collector.id && Rc::ptr_eq(&a.owner(), &collector.owner()) && crate::experience::can_gain_level(a))
            .collect();
        if self.info.max_extra_collectors > -1 {
            in_range.truncate(self.info.max_extra_collectors as usize);
        }
        in_range.push(collector.clone());
        for recipient in in_range {
            let levels = self.info.levels;
            w.add_frame_end_task(Box::new(move |_w| {
                if !recipient.is_dead() {
                    crate::experience::give_levels(&recipient, levels);
                }
            }));
        }
    }

    fn activate_duplicate(&self, collector: &ActorRef) {
        let info = self.info.clone();
        let c = collector.clone();
        collector.world().add_frame_end_task(Box::new(move |w| {
            let Some(positionable) = c.trait_or_default::<dyn IPositionable>() else {
                return;
            };
            let mut cells: Vec<CPos> = w
                .map()
                .find_tiles_in_circle(c.location(), info.max_radius, false)
                .filter(|x| positionable.can_enter_cell(*x, None, BlockedByActor::All))
                .collect();
            if let Some(m) = c.info.trait_info::<MobileInfo>() {
                let locomotor = locomotor_by_name(w, &m.locomotor);
                let path_finder = w.world_actor().trait_::<PathFinder>();
                cells.retain(|x| path_finder.path_might_exist_for_locomotor_blocked_by_immovable(&locomotor, *x, c.location()));
            }
            let shuffled = util::shuffle(&cells, &mut w.shared_random.borrow_mut());
            let mut duplicates = (shuffled.len() as i32).min(info.max_amount);
            if info.max_duplicate_value > 0
                && let Some(v) = c.info.trait_info::<ValuedInfo>().filter(|v| v.cost > 0)
            {
                duplicates = duplicates.min(info.max_duplicate_value / v.cost);
            }
            for cell in shuffled.iter().take(duplicates.max(0) as usize) {
                let actor = w.create_actor(
                    &c.info.name,
                    ActorInits::new().with(LocationInit::new(*cell)).with(owner_init(&info, &c)),
                );
                place_on_free_subcell(&actor, actor.location());
            }
        }));
    }

    fn activate_give_unit(&self, collector: &ActorRef) {
        let info = self.info.clone();
        let c = collector.clone();
        let me = self
            .self_
            .traits_implementing::<CrateAction>()
            .into_iter()
            .find(|a| Rc::ptr_eq(&a.info, &info))
            .expect("crate action");
        collector.world().add_frame_end_task(Box::new(move |w| {
            for unit in &info.units {
                let cells = me.suitable_cells(w, c.location(), unit);
                if cells.is_empty() {
                    continue;
                }
                let index = w.shared_random.borrow_mut().next_max(cells.len() as i32) as usize;
                let location = cells[index];
                let actor = w.create_actor(
                    unit,
                    ActorInits::new().with(LocationInit::new(location)).with(owner_init(&info, &c)),
                );
                place_on_free_subcell(&actor, location);
            }
        }));
    }

    fn activate_grant_condition(&self, collector: &ActorRef) {
        let info = &self.info;
        let condition = info.condition.as_deref().unwrap_or("");
        let crate_ = &self.self_;
        if collector.is_in_world() && !collector.is_dead() {
            crate::experience::grant_external_condition(collector, condition, info.levels, info.duration, crate_);
        }
        let center = crate_.center_position();
        let mut actors: Vec<ActorRef> = crate_
            .world()
            .find_actors_in_circle(center, info.range)
            .into_iter()
            .filter(|a| {
                a.id != crate_.id
                    && a.id != collector.id
                    && a.is_in_world()
                    && !a.is_dead()
                    && Rc::ptr_eq(&a.owner(), &collector.owner())
                    && crate::experience::accepts_external_condition(a, condition, crate_)
            })
            .collect();
        actors.sort_by_key(|a| (a.center_position() - center).length_squared());
        if info.max_extra_collectors > -1 {
            actors.truncate(info.max_extra_collectors as usize);
        }
        for a in actors {
            crate::experience::grant_external_condition(&a, condition, info.levels, info.duration, crate_);
        }
    }
}
