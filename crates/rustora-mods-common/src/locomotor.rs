// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/World/Locomotor.cs`.

use rustora_core::hash::{HashMap, HashSet};
use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::{LoadError, get_value};
use rustora_core::hash::IndexMap;
use rustora_core::miniyaml::MiniYaml;
use rustora_core::world_types::CPos;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::bitset::BitSet;
use rustora_engine::damage::DamageTypeTag;
use rustora_engine::map::{CellLayer, Map};
use rustora_engine::player::PlayerRelationship;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::subcell::{ANY, FULL_CELL, INVALID};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::actor_map::WorldExt;
use crate::building::Building;
use crate::interfaces::IMove;
use crate::mobile::Mobile;

pub fn register(r: &mut TraitRegistry) {
    r.register("Locomotor", |n| load_info!(LocomotorInfo, n));
}

pub const MOVEMENT_COST_FOR_UNREACHABLE_CELL: i16 = i16::MAX;

const HAS_MOVING_ACTOR: u8 = 1;
const HAS_STATIONARY_ACTOR: u8 = 2;
const HAS_MOVABLE_ACTOR: u8 = 4;
const HAS_CRUSHABLE_ACTOR: u8 = 8;
const HAS_TEMPORARY_BLOCKER: u8 = 16;
const HAS_TRANSIT_ONLY_ACTOR: u8 = 32;

pub mod movement_type {
    pub const NONE: u8 = 0;
    pub const HORIZONTAL: u8 = 1;
    pub const VERTICAL: u8 = 2;
    pub const TURN: u8 = 4;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlockedByActor {
    None,
    Immovable,
    Stationary,
    All,
}

#[derive(Clone, Copy, Debug)]
pub struct TerrainSpeedInfo {
    pub cost: i16,
    pub speed: i32,
}

const IMPASSABLE: TerrainSpeedInfo = TerrainSpeedInfo {
    cost: MOVEMENT_COST_FOR_UNREACHABLE_CELL,
    speed: 0,
};

#[derive(Clone, Debug, FieldLoad)]
pub struct LocomotorInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub name: String,
    pub wait_average: i32,
    pub wait_spread: i32,
    pub shares_cell: bool,
    pub move_into_shroud: bool,
    pub crushes: BitSet<CrushClass>,
    pub crush_damage_types: BitSet<DamageTypeTag>,
    #[field(loader = "load_speeds")]
    pub terrain_speeds: IndexMap<String, TerrainSpeedInfo>,
}

impl Default for LocomotorInfo {
    fn default() -> Self {
        LocomotorInfo {
            instance_name: None,
            name: "default".into(),
            wait_average: 40,
            wait_spread: 10,
            shares_cell: false,
            move_into_shroud: true,
            crushes: BitSet::default(),
            crush_damage_types: BitSet::default(),
            terrain_speeds: IndexMap::default(),
        }
    }
}

fn load_speeds(y: &MiniYaml) -> Result<IndexMap<String, TerrainSpeedInfo>, LoadError> {
    let mut ret = IndexMap::default();
    let Some(node) = y.node_with_key_or_default("TerrainSpeeds")? else {
        return Err(LoadError("Locomotor lacks TerrainSpeeds".into()));
    };
    for t in &node.value.nodes {
        let speed: i32 = get_value("speed", t.value.value.as_deref())?;
        if speed > 0 {
            let cost = match t.value.node_with_key_or_default("PathingCost")? {
                Some(n) => get_value::<i32>("cost", n.value.value.as_deref())? as i16,
                None => (10000 / speed) as i16,
            };
            ret.insert(t.key.clone(), TerrainSpeedInfo { speed, cost });
        }
    }
    Ok(ret)
}

#[derive(Clone, Copy, Default, rustora_engine::persist::Persist)]
#[persist(value)]
struct CellCache {
    immovable: u64,
    crushable: u64,
    flag: u8,
}

/// What a locomotor knows about a cell: the blocking cache, the terrain cost and whether the blocking cache must be
/// updated, in one record, so that a cost query reads one cache line.
#[derive(Clone, Copy, Default)]
struct CellState {
    immovable: u64,
    crushable: u64,
    cost: i16,
    flag: u8,
    dirty: bool,
}

/// The cell states of a locomotor, set when the world is loaded. Save games write them as the former dirty cell set,
/// cost layer and blocking cache layer.
#[derive(Default)]
struct Cells(OnceCell<RefCell<CellLayer<CellState>>>);

impl rustora_engine::persist::PersistState for Cells {
    fn save_state(&self, w: &mut rustora_engine::persist::SaveWriter) {
        use rustora_engine::persist::Persist;
        let Some(layer) = self.0.get() else {
            HashSet::<CPos>::default().save(w);
            w.bool(false);
            w.bool(false);
            return;
        };
        let l = layer.borrow();
        let (width, height) = (l.width, l.height);
        let mut dirty = HashSet::<CPos>::default();
        let mut cost = Vec::with_capacity(l.raw().len());
        let mut cache = Vec::with_capacity(l.raw().len());
        for (i, c) in l.raw().iter().enumerate() {
            if c.dirty {
                dirty.insert(CPos::new(i as i32 % width, i as i32 / width));
            }
            cost.push(c.cost);
            cache.push(CellCache {
                immovable: c.immovable,
                crushable: c.crushable,
                flag: c.flag,
            });
        }
        dirty.save(w);
        w.bool(true);
        CellLayer::from_raw(width, height, cost).save(w);
        w.bool(true);
        CellLayer::from_raw(width, height, cache).save(w);
    }

    fn load_state(&self, r: &mut rustora_engine::persist::SaveReader) {
        use rustora_engine::persist::Persist;
        let dirty = HashSet::<CPos>::load(r);
        let cost = r.bool().then(|| CellLayer::<i16>::load(r));
        let cache = r.bool().then(|| CellLayer::<CellCache>::load(r));
        let (Some(cost), Some(cache)) = (cost, cache) else {
            return;
        };
        let Some(layer) = self.0.get() else {
            r.fail("the locomotor cells are empty in the loaded world".to_string());
        };
        let mut l = layer.borrow_mut();
        let width = l.width;
        for (i, c) in l.raw_mut().iter_mut().enumerate() {
            let b = cache.raw()[i];
            *c = CellState {
                immovable: b.immovable,
                crushable: b.crushable,
                cost: cost.raw()[i],
                flag: b.flag,
                dirty: dirty.contains(&CPos::new(i as i32 % width, i as i32 / width)),
            };
        }
    }
}

pub type CellCostChanged = Rc<dyn Fn(CPos, i16, i16)>;

#[derive(rustora_engine::persist::Persist)]
pub struct Locomotor {
    pub info: Rc<LocomotorInfo>,
    terrain_infos: Vec<TerrainSpeedInfo>,
    world: std::rc::Weak<World>,
    cells: Cells,
    #[persist(skip)]
    cell_cost_changed: RefCell<Vec<CellCostChanged>>,
    me: std::rc::Weak<Locomotor>,
}

register_traits!(Locomotor: IWorldLoaded);
info_impl!(LocomotorInfo => |info, init| {
    let w = init.world();
    let speeds: HashMap<&str, TerrainSpeedInfo> = info.terrain_speeds.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let terrain_infos = w.map().terrain.terrain_types.iter().map(|t| speeds.get(t.ty.as_str()).copied().unwrap_or(IMPASSABLE)).collect();
    let weak = Rc::downgrade(&w);
    Some(Rc::new_cyclic(|me| Locomotor {
        info,
        terrain_infos,
        world: weak,
        cells: Cells::default(),
        cell_cost_changed: RefCell::new(Vec::new()),
        me: me.clone(),
    }))
});

impl IWorldLoaded for Locomotor {
    fn world_loaded(&self, w: &Rc<World>) {
        let map = w.map();
        let (mw, mh) = map.map_size;
        let me = self.me.clone();
        map.on_custom_terrain_changed(Rc::new(move |c| {
            if let Some(m) = me.upgrade() {
                m.update_cell_cost(c);
            }
        }));
        let me = self.me.clone();
        w.actor_map().on_cell_updated(Rc::new(move |c| {
            if let Some(m) = me.upgrade() {
                let mut l = m.cells().borrow_mut();
                if l.contains_cell(c) {
                    l.at_mut(c).dirty = true;
                }
            }
        }));
        let _ = self.cells.0.set(RefCell::new(CellLayer::new(mw, mh, CellState::default())));
        for cell in map.all_cells() {
            self.update_cell_cost(cell);
            self.update_cell_blocking(cell);
        }
    }
}

impl Locomotor {
    fn world(&self) -> Rc<World> {
        self.world.upgrade().expect("world alive")
    }

    fn cells(&self) -> &RefCell<CellLayer<CellState>> {
        self.cells.0.get().expect("locomotor loaded")
    }

    pub fn on_cell_cost_changed(&self, f: CellCostChanged) {
        self.cell_cost_changed.borrow_mut().push(f);
    }

    pub fn movement_cost_for_cell(&self, cell: CPos) -> i16 {
        if !self.world().map().contains(cell) {
            return MOVEMENT_COST_FOR_UNREACHABLE_CELL;
        }
        self.cells().borrow().at(cell).cost
    }

    pub fn movement_speed_for_cell(&self, cell: CPos) -> i32 {
        self.terrain_infos[self.world().map().terrain_index(cell) as usize].speed
    }

    pub fn movement_cost_to_enter_cell(
        &self,
        actor: Option<&ActorRef>,
        dest: CPos,
        check: BlockedByActor,
        ignore_actor: Option<&ActorRef>,
        ignore_self: bool,
        sub: u8,
    ) -> i16 {
        let actor = actor.map(|a| (a, a.with_owner(|o| o.player_mask)));
        self.movement_cost_to_enter_cell_in(self.world().map(), actor, dest, check, ignore_actor, ignore_self, sub)
    }

    /// `movement_cost_to_enter_cell` with the map and the owner mask of the actor given by the caller, for path
    /// searches, which ask for many cells in a row.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn movement_cost_to_enter_cell_in(
        &self,
        map: &Map,
        actor: Option<(&ActorRef, u64)>,
        dest: CPos,
        check: BlockedByActor,
        ignore_actor: Option<&ActorRef>,
        ignore_self: bool,
        sub: u8,
    ) -> i16 {
        if !map.contains(dest) {
            return MOVEMENT_COST_FOR_UNREACHABLE_CELL;
        }
        let mut state = *self.cells().borrow().at(dest);
        if state.cost == MOVEMENT_COST_FOR_UNREACHABLE_CELL {
            return MOVEMENT_COST_FOR_UNREACHABLE_CELL;
        }
        if check == BlockedByActor::None {
            return state.cost;
        }
        if state.dirty {
            state = self.get_cache(dest);
        }
        if !self.can_move_freely_into(state, actor, dest, sub, check, ignore_actor, ignore_self) {
            return MOVEMENT_COST_FOR_UNREACHABLE_CELL;
        }
        state.cost
    }

    #[allow(clippy::too_many_arguments)]
    fn can_move_freely_into(
        &self,
        cache: CellState,
        actor: Option<(&ActorRef, u64)>,
        cell: CPos,
        sub: u8,
        check: BlockedByActor,
        ignore_actor: Option<&ActorRef>,
        ignore_self: bool,
    ) -> bool {
        if check == BlockedByActor::None {
            return true;
        }
        let flag = cache.flag;
        if flag == 0 {
            return true;
        }
        let Some((actor, owner_mask)) = actor else { return false };
        if cache.crushable & owner_mask != 0 {
            return true;
        }
        if check <= BlockedByActor::Stationary && flag & HAS_STATIONARY_ACTOR == 0 {
            return true;
        }
        if check <= BlockedByActor::Immovable && cache.immovable & owner_mask == 0 {
            return true;
        }
        if ignore_actor.is_none() && !ignore_self && sub == FULL_CELL && flag & (HAS_TEMPORARY_BLOCKER | HAS_TRANSIT_ONLY_ACTOR) == 0 {
            if check == BlockedByActor::All {
                return false;
            }
            if flag & HAS_CRUSHABLE_ACTOR == 0 {
                return false;
            }
            if flag & HAS_MOVABLE_ACTOR == 0 {
                return false;
            }
            if check == BlockedByActor::Stationary && flag & HAS_MOVING_ACTOR == 0 {
                return false;
            }
        }
        let w = self.world();
        let am = w.actor_map();
        let others = am.actors_at_inline(cell, sub);
        if ignore_self {
            let mut is_blocked = false;
            for other in &others {
                if actor.id == other.id {
                    return true;
                }
                is_blocked = is_blocked || self.is_blocked_by(actor, other, ignore_actor, cell, check, flag);
            }
            !is_blocked
        } else {
            !others.iter().any(|o| self.is_blocked_by(actor, o, ignore_actor, cell, check, flag))
        }
    }

    pub fn can_stay_in_cell(&self, cell: CPos) -> bool {
        if !self.world().map().contains(cell) {
            return false;
        }
        self.get_cache(cell).flag & HAS_TRANSIT_ONLY_ACTOR == 0
    }

    pub fn get_available_subcell(
        &self,
        self_: &ActorRef,
        cell: CPos,
        check: BlockedByActor,
        preferred: u8,
        ignore_actor: Option<&ActorRef>,
    ) -> u8 {
        if self.movement_cost_for_cell(cell) == MOVEMENT_COST_FOR_UNREACHABLE_CELL {
            return INVALID;
        }
        let w = self.world();
        let am = w.actor_map();
        if check > BlockedByActor::None {
            let flag = self.get_cache(cell).flag;
            let check_transient = |o: &ActorRef| self.is_blocked_by(self_, o, ignore_actor, cell, check, flag);
            if !self.info.shares_cell {
                return if am.any_actors_at_with(cell, FULL_CELL, &check_transient) {
                    INVALID
                } else {
                    FULL_CELL
                };
            }
            return am.free_subcell_with(cell, preferred, &check_transient);
        }
        if !self.info.shares_cell {
            return if am.any_actors_at_sub(cell, FULL_CELL, true) {
                INVALID
            } else {
                FULL_CELL
            };
        }
        am.free_subcell(cell, preferred, true)
    }

    fn is_blocked_by(
        &self,
        actor: &ActorRef,
        other: &ActorRef,
        ignore_actor: Option<&ActorRef>,
        cell: CPos,
        check: BlockedByActor,
        flag: u8,
    ) -> bool {
        if ignore_actor.is_some_and(|i| i.id == other.id) {
            return false;
        }
        let other_mobile = crate::mobile::mobile_of(other);
        let other_is_movable = other_mobile.is_some_and(Mobile::is_movable);
        let other_is_moving = other_is_movable && other_mobile.unwrap().current_movement_types() & movement_type::HORIZONTAL != 0;
        if check <= BlockedByActor::Immovable
            && flag & HAS_MOVABLE_ACTOR != 0
            && other_is_movable
            && actor.with_owner(|owner_| owner_.relationship_with(Some(&other.owner()))) == PlayerRelationship::ALLY
        {
            return false;
        }
        if check <= BlockedByActor::Stationary && flag & HAS_MOVING_ACTOR != 0 && other_is_moving {
            return false;
        }
        if flag & HAS_TEMPORARY_BLOCKER != 0
            && other
                .trait_or_default::<dyn crate::interfaces::ITemporaryBlocker>()
                .is_some_and(|t| t.can_remove_blockage(other, actor))
        {
            return false;
        }
        if flag & HAS_TRANSIT_ONLY_ACTOR != 0
            && let Some(b) = building_of(other)
            && b.transit_only.contains(&cell)
        {
            return false;
        }
        if flag & HAS_CRUSHABLE_ACTOR == 0 || self.info.crushes.is_empty() {
            return true;
        }
        for c in &other.cache().crushables {
            if c.crushable_by_actor(other, actor, self.info.crushes) {
                return false;
            }
        }
        true
    }

    fn get_cache(&self, cell: CPos) -> CellState {
        let dirty = std::mem::take(&mut self.cells().borrow_mut().at_mut(cell).dirty);
        if dirty {
            self.update_cell_blocking(cell);
        }
        *self.cells().borrow().at(cell)
    }

    fn update_cell_cost(&self, cell: CPos) {
        let w = self.world();
        let index = w.map().terrain_index(cell);
        let cost = if index != u8::MAX {
            self.terrain_infos[index as usize].cost
        } else {
            MOVEMENT_COST_FOR_UNREACHABLE_CELL
        };
        let old = {
            let mut c = self.cells().borrow_mut();
            let s = c.at_mut(cell);
            std::mem::replace(&mut s.cost, cost)
        };
        let listeners = self.cell_cost_changed.borrow().clone();
        for f in listeners {
            f(cell, old, cost);
        }
    }

    fn update_cell_blocking(&self, cell: CPos) {
        let w = self.world();
        let am = w.actor_map();
        let all = w.all_players_mask.get();
        let mut flag = 0u8;
        let mut immovable = 0u64;
        let mut crushable = all;
        if self.info.shares_cell && am.has_free_subcell(cell, true) {
            self.set_blocking(cell, immovable, crushable, flag);
            return;
        }
        for actor in am.actors_at_inline(cell, FULL_CELL) {
            let mut actor_immovable = all;
            let mut actor_crushable = 0u64;
            let mobile = crate::mobile::mobile_of(&actor);
            let is_movable = mobile.is_some_and(Mobile::is_movable);
            let is_moving = is_movable && mobile.unwrap().current_movement_types() & movement_type::HORIZONTAL != 0;
            if building_of(&actor).is_some_and(|b| b.transit_only.contains(&cell)) {
                flag |= HAS_TRANSIT_ONLY_ACTOR;
            }
            for c in &actor.cache().crushables {
                flag |= HAS_CRUSHABLE_ACTOR;
                actor_crushable |= c.crushable_by(&actor, self.info.crushes);
            }
            if is_moving {
                flag |= HAS_MOVING_ACTOR;
            } else {
                flag |= HAS_STATIONARY_ACTOR;
            }
            if is_movable {
                flag |= HAS_MOVABLE_ACTOR;
                actor_immovable &= !actor.with_owner(|owner_| owner_.allied_players_mask.get());
            }
            if actor.trait_or_default::<dyn crate::interfaces::ITemporaryBlocker>().is_some() {
                flag |= HAS_TEMPORARY_BLOCKER;
            }
            crushable &= actor_crushable;
            immovable |= actor_immovable;
        }
        self.set_blocking(cell, immovable, crushable, flag);
    }

    fn set_blocking(&self, cell: CPos, immovable: u64, crushable: u64, flag: u8) {
        let mut l = self.cells().borrow_mut();
        let s = l.at_mut(cell);
        s.immovable = immovable;
        s.crushable = crushable;
        s.flag = flag;
    }

    /// Lowest cost over the terrain speeds, for the path cost estimator.
    pub fn min_terrain_cost(&self) -> i16 {
        self.info.terrain_speeds.values().map(|t| t.cost).min().unwrap_or(0)
    }

    pub fn has_terrain_speeds(&self) -> bool {
        !self.info.terrain_speeds.is_empty()
    }
}

pub fn building_of(a: &ActorRef) -> Option<&Building> {
    a.cache().occupies_space.as_ref()?.as_any().downcast_ref::<Building>()
}

pub fn any_sub() -> u8 {
    ANY
}

/// `world.WorldActor.TraitsImplementing<Locomotor>().Single(l => l.Info.Name == name)`.
pub fn locomotor_by_name(w: &World, name: &str) -> Rc<Locomotor> {
    w.world_actor()
        .traits_implementing::<Locomotor>()
        .into_iter()
        .find(|l| l.info.name == name)
        .unwrap_or_else(|| panic!("no locomotor named {name}"))
}
