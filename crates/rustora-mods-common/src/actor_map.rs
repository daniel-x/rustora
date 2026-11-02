// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/World/ActorMap.cs`.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::world_types::{CPos, WDist, WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::map::CellLayer;
use rustora_engine::net_collections::{NetMap, NetSet};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::subcell::{ANY, FIRST, FULL_CELL, INVALID};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("ActorMap", |n| load_info!(ActorMapInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ActorMapInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub bin_size: i32,
}

impl Default for ActorMapInfo {
    fn default() -> Self {
        ActorMapInfo {
            instance_name: None,
            bin_size: 10,
        }
    }
}

type ActorCallback = Rc<dyn Fn(&ActorRef)>;

/// `Actor` by ID for sets.
#[derive(Clone)]
pub struct ById(pub ActorRef);
impl PartialEq for ById {
    fn eq(&self, o: &ById) -> bool {
        self.0.id == o.0.id
    }
}
impl Eq for ById {}
impl std::hash::Hash for ById {
    fn hash<H: std::hash::Hasher>(&self, s: &mut H) {
        self.0.id.hash(s);
    }
}

struct CellTrigger {
    footprint: Vec<CPos>,
    dirty: Cell<bool>,
    on_entered: Option<ActorCallback>,
    on_exited: Option<ActorCallback>,
    old_actors: RefCell<NetSet<ById>>,
    current_actors: RefCell<NetSet<ById>>,
}

struct ProximityTrigger {
    top_left: Cell<WPos>,
    bottom_right: Cell<WPos>,
    dirty: Cell<bool>,
    on_entered: Option<ActorCallback>,
    on_exited: Option<ActorCallback>,
    old_actors: RefCell<NetSet<ById>>,
    current_actors: RefCell<NetSet<ById>>,
    position: Cell<WPos>,
    range: Cell<WDist>,
    v_range: Cell<WDist>,
}

impl ProximityTrigger {
    fn update(&self, pos: WPos, range: WDist, v_range: WDist) {
        self.position.set(pos);
        self.range.set(range);
        self.v_range.set(v_range);
        let offset = WVec::new(range.length, range.length, v_range.length);
        self.top_left.set(pos - offset);
        self.bottom_right.set(pos + offset);
        self.dirty.set(true);
    }
}

#[derive(Default)]
struct Bin {
    actors: Vec<ActorRef>,
    proximity_triggers: Vec<Rc<ProximityTrigger>>,
}

/// Nodes of one cell, oldest first (C# keeps a linked list with the newest at the head).
type CellNodes = Vec<(u8, ActorRef)>;

pub struct ActorMap {
    info: Rc<ActorMapInfo>,
    map: Rc<rustora_engine::map::Map>,
    cell_triggers: RefCell<NetMap<i32, Rc<CellTrigger>>>,
    cell_trigger_influence: RefCell<NetMap<CPos, Vec<Rc<CellTrigger>>>>,
    proximity_triggers: RefCell<NetMap<i32, Rc<ProximityTrigger>>>,
    next_trigger_id: Cell<i32>,
    influence: RefCell<CellLayer<CellNodes>>,
    cell_updated: RefCell<Vec<Rc<dyn Fn(CPos)>>>,
    bins: RefCell<Vec<Bin>>,
    rows: i32,
    cols: i32,
    add_actor_position: RefCell<NetSet<ById>>,
    remove_actor_position: RefCell<NetSet<ById>>,
    pub largest_actor_radius: Cell<WDist>,
    pub largest_blocking_actor_radius: Cell<WDist>,
}

register_traits!(ActorMap: ITick);
info_impl!(ActorMapInfo => |info, init| {
    let world = init.world();
    let map = world.map().clone();
    let (mw, mh) = map.map_size;
    let cols = mw / info.bin_size + 1;
    let rows = mh / info.bin_size + 1;
    let mut bins = Vec::with_capacity((rows * cols) as usize);
    bins.resize_with((rows * cols) as usize, Bin::default);
    Some(Rc::new(ActorMap {
        map,
        cell_triggers: RefCell::new(NetMap::new()),
        cell_trigger_influence: RefCell::new(NetMap::new()),
        proximity_triggers: RefCell::new(NetMap::new()),
        next_trigger_id: Cell::new(0),
        influence: RefCell::new(CellLayer::new(mw, mh, Vec::new())),
        cell_updated: RefCell::new(Vec::new()),
        bins: RefCell::new(bins),
        rows,
        cols,
        add_actor_position: RefCell::new(NetSet::new()),
        remove_actor_position: RefCell::new(NetSet::new()),
        largest_actor_radius: Cell::new(crate::hit_shape::largest_outer_radius(&world, false)),
        largest_blocking_actor_radius: Cell::new(crate::hit_shape::largest_outer_radius(&world, true)),
        info,
    }))
});

impl ITick for ActorMap {
    fn tick(&self, _self_: &ActorRef) {
        if !self.remove_actor_position.borrow().is_empty() {
            let remove = self.remove_actor_position.borrow();
            for bin in self.bins.borrow_mut().iter_mut() {
                let before = bin.actors.len();
                bin.actors.retain(|a| !remove.contains(&ById(a.clone())));
                if bin.actors.len() != before {
                    for t in &bin.proximity_triggers {
                        t.dirty.set(true);
                    }
                }
            }
            drop(remove);
            self.remove_actor_position.borrow_mut().clear();
        }
        for ById(a) in self.add_actor_position.borrow().iter() {
            let pos = a.center_position();
            let col = self.world_to_bin(pos.x).clamp(0, self.cols - 1);
            let row = self.world_to_bin(pos.y).clamp(0, self.rows - 1);
            let mut bins = self.bins.borrow_mut();
            let bin = &mut bins[(row * self.cols + col) as usize];
            bin.actors.push(a.clone());
            for t in &bin.proximity_triggers {
                t.dirty.set(true);
            }
        }
        self.add_actor_position.borrow_mut().clear();

        let triggers: Vec<Rc<CellTrigger>> = self.cell_triggers.borrow().values().cloned().collect();
        for t in triggers {
            self.tick_cell_trigger(&t);
        }
        let triggers: Vec<Rc<ProximityTrigger>> = self.proximity_triggers.borrow().values().cloned().collect();
        for t in triggers {
            self.tick_proximity_trigger(&t);
        }
    }
}

fn notify_diff(old: &NetSet<ById>, current: &NetSet<ById>, entered: &Option<ActorCallback>, exited: &Option<ActorCallback>) {
    if let Some(f) = entered {
        for a in current.iter() {
            if !old.contains(a) {
                f(&a.0);
            }
        }
    }
    if let Some(f) = exited {
        for a in old.iter() {
            if !current.contains(a) {
                f(&a.0);
            }
        }
    }
}

fn union_into(set: &mut NetSet<ById>, from: &NetSet<ById>) {
    for a in from.iter() {
        set.insert(a.clone());
    }
}

impl ActorMap {
    fn tick_cell_trigger(&self, t: &CellTrigger) {
        if !t.dirty.get() {
            return;
        }
        {
            let mut old = t.old_actors.borrow_mut();
            old.clear();
            union_into(&mut old, &t.current_actors.borrow());
            let mut cur = t.current_actors.borrow_mut();
            cur.clear();
            for c in &t.footprint {
                for a in self.get_actors_at(*c) {
                    cur.insert(ById(a));
                }
            }
        }
        let old = t.old_actors.borrow().clone();
        let cur = t.current_actors.borrow().clone();
        notify_diff(&old, &cur, &t.on_entered, &t.on_exited);
        t.dirty.set(false);
    }

    fn tick_proximity_trigger(&self, t: &ProximityTrigger) {
        if !t.dirty.get() {
            return;
        }
        {
            let mut old = t.old_actors.borrow_mut();
            old.clear();
            union_into(&mut old, &t.current_actors.borrow());
            let (pos, range, v_range) = (t.position.get(), t.range.get(), t.v_range.get());
            let delta = WVec::new(range.length, range.length, 0);
            let mut cur = t.current_actors.borrow_mut();
            cur.clear();
            for a in self.actors_in_box(pos - delta, pos + delta) {
                if (a.center_position() - pos).horizontal_length_squared() < range.length_squared()
                    && (v_range.length == 0 || self.map.distance_above_terrain(a.center_position()).length_squared() <= v_range.length_squared())
                {
                    cur.insert(ById(a));
                }
            }
        }
        let old = t.old_actors.borrow().clone();
        let cur = t.current_actors.borrow().clone();
        notify_diff(&old, &cur, &t.on_entered, &t.on_exited);
        t.dirty.set(false);
    }

    fn world_to_bin(&self, world: i32) -> i32 {
        world / 1024 / self.info.bin_size
    }

    pub fn on_cell_updated(&self, f: Rc<dyn Fn(CPos)>) {
        self.cell_updated.borrow_mut().push(f);
    }

    fn fire_cell_updated(&self, c: CPos) {
        let listeners = self.cell_updated.borrow().clone();
        for f in listeners {
            f(c);
        }
    }

    /// Actors in the cell, newest influence first.
    pub fn get_actors_at(&self, a: CPos) -> Vec<ActorRef> {
        let inf = self.influence.borrow();
        if !inf.contains_cell(a) {
            return Vec::new();
        }
        inf.at(a).iter().rev().map(|n| n.1.clone()).collect()
    }

    pub fn get_actors_at_sub(&self, a: CPos, sub: u8) -> Vec<ActorRef> {
        let inf = self.influence.borrow();
        if !inf.contains_cell(a) {
            return Vec::new();
        }
        let always = sub == FULL_CELL || sub == ANY;
        inf.at(a)
            .iter()
            .rev()
            .filter(|n| n.0 == sub || n.0 == FULL_CELL || always)
            .map(|n| n.1.clone())
            .collect()
    }

    pub fn has_free_subcell(&self, cell: CPos, check_transient: bool) -> bool {
        self.free_subcell(cell, ANY, check_transient) != INVALID
    }

    fn nodes_any(nodes: &CellNodes, a: CPos, sub: u8, check_transient: bool) -> bool {
        let always = sub == FULL_CELL || sub == ANY;
        for i in nodes.iter().rev() {
            if always || i.0 == sub || i.0 == FULL_CELL {
                if check_transient {
                    return true;
                }
                let leaving = i.1.cache().occupies_space.as_ref().is_some_and(|o| o.is_leaving_cell(a, i.0));
                if !leaving {
                    return true;
                }
            }
        }
        false
    }

    fn nodes_any_with(nodes: &CellNodes, sub: u8, with_condition: &dyn Fn(&ActorRef) -> bool) -> bool {
        let always = sub == FULL_CELL || sub == ANY;
        nodes.iter().rev().any(|i| (always || i.0 == sub || i.0 == FULL_CELL) && with_condition(&i.1))
    }

    pub fn free_subcell(&self, cell: CPos, preferred: u8, check_transient: bool) -> u8 {
        let inf = self.influence.borrow();
        if !inf.contains_cell(cell) {
            return if preferred != ANY { preferred } else { FIRST };
        }
        let nodes = inf.at(cell);
        if preferred != ANY && !Self::nodes_any(nodes, cell, preferred, check_transient) {
            return preferred;
        }
        if nodes.is_empty() {
            return self.map.default_subcell();
        }
        for i in FIRST..self.map.grid.subcell_offsets.len() as u8 {
            if i != preferred && !Self::nodes_any(nodes, cell, i, check_transient) {
                return i;
            }
        }
        INVALID
    }

    pub fn free_subcell_with(&self, cell: CPos, preferred: u8, check_if_blocker: &dyn Fn(&ActorRef) -> bool) -> u8 {
        let inf = self.influence.borrow();
        if !inf.contains_cell(cell) {
            return if preferred != ANY { preferred } else { FIRST };
        }
        let nodes = inf.at(cell);
        if preferred != ANY && !Self::nodes_any_with(nodes, preferred, check_if_blocker) {
            return preferred;
        }
        if nodes.is_empty() {
            return self.map.default_subcell();
        }
        for i in FIRST..self.map.grid.subcell_offsets.len() as u8 {
            if i != preferred && !Self::nodes_any_with(nodes, i, check_if_blocker) {
                return i;
            }
        }
        INVALID
    }

    pub fn any_actors_at(&self, a: CPos) -> bool {
        let inf = self.influence.borrow();
        inf.contains_cell(a) && !inf.at(a).is_empty()
    }

    pub fn any_actors_at_sub(&self, a: CPos, sub: u8, check_transient: bool) -> bool {
        let inf = self.influence.borrow();
        inf.contains_cell(a) && Self::nodes_any(inf.at(a), a, sub, check_transient)
    }

    pub fn any_actors_at_with(&self, a: CPos, sub: u8, with_condition: &dyn Fn(&ActorRef) -> bool) -> bool {
        let inf = self.influence.borrow();
        inf.contains_cell(a) && Self::nodes_any_with(inf.at(a), sub, with_condition)
    }

    pub fn all_actors(&self) -> Vec<ActorRef> {
        let inf = self.influence.borrow();
        let (w, h) = (inf.width, inf.height);
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                out.extend(inf.at(CPos::new(x, y)).iter().rev().map(|n| n.1.clone()));
            }
        }
        out
    }

    fn mark_triggers(&self, c: CPos) {
        if let Some(triggers) = self.cell_trigger_influence.borrow().get(&c) {
            for t in triggers {
                t.dirty.set(true);
            }
        }
    }

    pub fn add_influence(&self, a: &ActorRef, ios: &dyn IOccupySpace) {
        for (c, sub) in ios.occupied_cells() {
            {
                let mut inf = self.influence.borrow_mut();
                if !inf.contains_cell(c) {
                    continue;
                }
                inf.at_mut(c).push((sub, a.clone()));
            }
            self.mark_triggers(c);
            self.fire_cell_updated(c);
        }
    }

    pub fn remove_influence(&self, a: &ActorRef, ios: &dyn IOccupySpace) {
        for (c, _) in ios.occupied_cells() {
            {
                let mut inf = self.influence.borrow_mut();
                if !inf.contains_cell(c) {
                    continue;
                }
                inf.at_mut(c).retain(|n| n.1.id != a.id);
            }
            self.mark_triggers(c);
            self.fire_cell_updated(c);
        }
    }

    pub fn update_occupied_cells(&self, ios: &dyn IOccupySpace) {
        if self.cell_updated.borrow().is_empty() {
            return;
        }
        for (c, _) in ios.occupied_cells() {
            self.fire_cell_updated(c);
        }
    }

    pub fn add_cell_trigger(&self, cells: Vec<CPos>, on_entry: Option<ActorCallback>, on_exit: Option<ActorCallback>) -> i32 {
        let id = self.next_trigger_id.get();
        self.next_trigger_id.set(id + 1);
        let t = Rc::new(CellTrigger {
            footprint: cells.clone(),
            dirty: Cell::new(true),
            on_entered: on_entry,
            on_exited: on_exit,
            old_actors: RefCell::new(NetSet::new()),
            current_actors: RefCell::new(NetSet::new()),
        });
        self.cell_triggers.borrow_mut().insert(id, t.clone());
        let inf = self.influence.borrow();
        let mut tin = self.cell_trigger_influence.borrow_mut();
        for c in cells {
            if !inf.contains_cell(c) {
                continue;
            }
            if !tin.contains_key(&c) {
                tin.insert(c, Vec::new());
            }
            tin.get_mut(&c).unwrap().push(t.clone());
        }
        id
    }

    /// C# keeps the removed trigger in `cellTriggers`, where it stays ticking.
    pub fn remove_cell_trigger(&self, id: i32) {
        let Some(trigger) = self.cell_triggers.borrow().get(&id).cloned() else { return };
        let mut tin = self.cell_trigger_influence.borrow_mut();
        for c in &trigger.footprint {
            if let Some(v) = tin.get_mut(c) {
                v.retain(|t| !Rc::ptr_eq(t, &trigger));
            }
        }
    }

    fn bin_indices_in_box(&self, a: WPos, b: WPos) -> Vec<usize> {
        let (left, top, right, bottom) = (a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y));
        let min_col = self.world_to_bin(left).clamp(0, self.cols - 1);
        let min_row = self.world_to_bin(top).clamp(0, self.rows - 1);
        let max_col = self.world_to_bin(right).clamp(0, self.cols - 1);
        let max_row = self.world_to_bin(bottom).clamp(0, self.rows - 1);
        let mut out = Vec::new();
        for row in min_row..=max_row {
            for col in min_col..=max_col {
                out.push((row * self.cols + col) as usize);
            }
        }
        out
    }

    pub fn add_proximity_trigger(&self, pos: WPos, range: WDist, v_range: WDist, on_entry: Option<ActorCallback>, on_exit: Option<ActorCallback>) -> i32 {
        let id = self.next_trigger_id.get();
        self.next_trigger_id.set(id + 1);
        let t = Rc::new(ProximityTrigger {
            top_left: Cell::new(pos),
            bottom_right: Cell::new(pos),
            dirty: Cell::new(true),
            on_entered: on_entry,
            on_exited: on_exit,
            old_actors: RefCell::new(NetSet::new()),
            current_actors: RefCell::new(NetSet::new()),
            position: Cell::new(pos),
            range: Cell::new(range),
            v_range: Cell::new(v_range),
        });
        t.update(pos, range, v_range);
        self.proximity_triggers.borrow_mut().insert(id, t.clone());
        let mut bins = self.bins.borrow_mut();
        for i in self.bin_indices_in_box(t.top_left.get(), t.bottom_right.get()) {
            bins[i].proximity_triggers.push(t.clone());
        }
        id
    }

    /// C# leaves the removed trigger in `proximityTriggers`, where it stays ticking.
    pub fn remove_proximity_trigger(&self, id: i32) {
        let Some(t) = self.proximity_triggers.borrow().get(&id).cloned() else { return };
        {
            let mut bins = self.bins.borrow_mut();
            for i in self.bin_indices_in_box(t.top_left.get(), t.bottom_right.get()) {
                remove_first(&mut bins[i].proximity_triggers, &t);
            }
        }
        if let Some(f) = &t.on_exited {
            let cur = t.current_actors.borrow().clone();
            for a in cur.iter() {
                f(&a.0);
            }
        }
    }

    pub fn update_proximity_trigger(&self, id: i32, pos: WPos, range: WDist, v_range: WDist) {
        let Some(t) = self.proximity_triggers.borrow().get(&id).cloned() else { return };
        let mut bins = self.bins.borrow_mut();
        for i in self.bin_indices_in_box(t.top_left.get(), t.bottom_right.get()) {
            remove_first(&mut bins[i].proximity_triggers, &t);
        }
        t.update(pos, range, v_range);
        for i in self.bin_indices_in_box(t.top_left.get(), t.bottom_right.get()) {
            bins[i].proximity_triggers.push(t.clone());
        }
    }

    pub fn add_position(&self, a: &ActorRef) {
        self.add_actor_position.borrow_mut().insert(ById(a.clone()));
    }

    pub fn remove_position(&self, a: &ActorRef) {
        self.remove_actor_position.borrow_mut().insert(ById(a.clone()));
    }

    pub fn update_position(&self, a: &ActorRef) {
        self.remove_position(a);
        self.add_position(a);
    }

    pub fn actors_in_box(&self, a: WPos, b: WPos) -> Vec<ActorRef> {
        let (left, top, right, bottom) = (a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y));
        let bins = self.bins.borrow();
        let mut out = Vec::new();
        for i in self.bin_indices_in_box(a, b) {
            for actor in &bins[i].actors {
                if actor.is_in_world() {
                    let c = actor.center_position();
                    if left <= c.x && c.x <= right && top <= c.y && c.y <= bottom {
                        out.push(actor.clone());
                    }
                }
            }
        }
        out
    }
}

fn remove_first(v: &mut Vec<Rc<ProximityTrigger>>, t: &Rc<ProximityTrigger>) {
    if let Some(i) = v.iter().position(|x| Rc::ptr_eq(x, t)) {
        v.remove(i);
    }
}

/// World helpers that need the actor map: `AddToMaps`, `UpdateMaps`, `RemoveFromMaps`.
pub trait WorldExt {
    fn actor_map(&self) -> Rc<ActorMap>;
    fn add_to_maps(&self, a: &ActorRef, ios: &dyn IOccupySpace);
    fn update_maps(&self, a: &ActorRef);
    fn remove_from_maps(&self, a: &ActorRef, ios: &dyn IOccupySpace);
}

impl WorldExt for World {
    fn actor_map(&self) -> Rc<ActorMap> {
        let any: &Rc<dyn Any> = self.actor_map_slot.get_or_init(|| self.trait_dict().get::<ActorMap>(self.world_actor()) as Rc<dyn Any>);
        any.clone().downcast::<ActorMap>().expect("actor map type")
    }

    fn add_to_maps(&self, a: &ActorRef, ios: &dyn IOccupySpace) {
        let m = self.actor_map();
        m.add_influence(a, ios);
        m.add_position(a);
    }

    fn update_maps(&self, a: &ActorRef) {
        if !a.is_in_world() {
            return;
        }
        self.actor_map().update_position(a);
    }

    fn remove_from_maps(&self, a: &ActorRef, ios: &dyn IOccupySpace) {
        let m = self.actor_map();
        m.remove_influence(a, ios);
        m.remove_position(a);
    }
}
