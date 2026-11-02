// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Pathfinder/HierarchicalPathFinder.cs`.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use indexmap::IndexSet;
use rustora_core::world_types::{CPos, CVec};
use rustora_engine::actor::ActorRef;
use rustora_engine::net_collections::NetSet;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::world::World;

use crate::actor_map::{ActorMap, WorldExt};
use crate::locomotor::{BlockedByActor, Locomotor, MOVEMENT_COST_FOR_UNREACHABLE_CELL, building_of};
use crate::pathfinder::*;

const GRID_SIZE: i32 = 10;

fn div_away(a: i32, b: i32) -> i32 {
    let (q, r) = (a / b, a % b);
    if r == 0 { q } else { q + if a.signum() == b.signum() { 1 } else { -1 } }
}

#[derive(Clone, Default)]
struct GridInfo {
    single_abstract_cell: Option<CPos>,
    local_cell_to_abstract_cell: HashMap<CPos, CPos>,
}

impl GridInfo {
    fn copy_abstract_cells_into(&self, set: &mut NetSet<CPos>) {
        if let Some(s) = self.single_abstract_cell {
            set.insert(s);
        }
        for c in self.local_cell_to_abstract_cell.values() {
            set.insert(*c);
        }
    }
}

type AbstractGraph = HashMap<CPos, Vec<GraphConnection>>;

struct AbstractGraphWithInsertedEdges {
    hpf: Rc<HierarchicalPathFinder>,
    changed_edges: HashMap<CPos, Vec<GraphConnection>>,
}

impl AbstractGraphWithInsertedEdges {
    fn new(hpf: Rc<HierarchicalPathFinder>, source_edges: &[GraphEdge], target_edge: Option<GraphEdge>) -> Rc<Self> {
        let mut g = AbstractGraphWithInsertedEdges {
            hpf,
            changed_edges: HashMap::new(),
        };
        for e in source_edges {
            g.insert_connections(e.source, e.destination);
        }
        if let Some(e) = target_edge {
            g.insert_connections(e.source, e.destination);
        }
        Rc::new(g)
    }

    fn insert_connections(&mut self, local: CPos, abs: CPos) {
        let est = self.hpf.cost_estimator.get().copied().expect("estimator");
        let conn = |d: CPos, from: CPos| GraphConnection {
            destination: d,
            cost: est.estimate(from, d),
        };
        let graph = self.hpf.abstract_graph.borrow();
        let edges: Vec<GraphConnection> = graph.get(&abs).cloned().unwrap_or_default();
        let mut local_edges: Vec<GraphConnection> = edges.iter().map(|e| conn(e.destination, local)).collect();
        local_edges.push(conn(abs, local));
        self.changed_edges.insert(local, local_edges);
        let mut abs_edges = self.changed_edges.get(&abs).cloned().unwrap_or_else(|| edges.clone());
        abs_edges.push(conn(local, abs));
        self.changed_edges.insert(abs, abs_edges);
        for e in &edges {
            let mut v = match self.changed_edges.get(&e.destination) {
                Some(x) => x.clone(),
                None => graph[&e.destination].clone(),
            };
            v.push(conn(local, e.destination));
            self.changed_edges.insert(e.destination, v);
        }
    }

    fn get_connections(&self, position: CPos) -> Vec<GraphConnection> {
        if let Some(c) = self.changed_edges.get(&position) {
            return c.clone();
        }
        self.hpf.abstract_graph.borrow().get(&position).cloned().unwrap_or_default()
    }
}

pub struct HierarchicalPathFinder {
    world: Weak<World>,
    locomotor: Rc<Locomotor>,
    actor_map: Rc<ActorMap>,
    cost_estimator: OnceCell<CostEstimator>,
    dirty_grid_indexes: RefCell<NetSet<i32>>,
    /// `None` when blocking actors are not accounted for.
    cells_with_blocking_actor: Option<RefCell<HashSet<CPos>>>,
    map_bottom_right: CPos,
    grid_xs: i32,
    grid_ys: i32,
    grid_infos: RefCell<Vec<GridInfo>>,
    abstract_graph: RefCell<AbstractGraph>,
    abstract_domains: RefCell<HashMap<CPos, u32>>,
    me: Weak<HierarchicalPathFinder>,
}

fn grid_top_left_of(c: CPos) -> CPos {
    CPos::new(c.x() / GRID_SIZE * GRID_SIZE, c.y() / GRID_SIZE * GRID_SIZE)
}

impl HierarchicalPathFinder {
    pub fn new(world: &Rc<World>, locomotor: Rc<Locomotor>, check: BlockedByActor) -> Rc<HierarchicalPathFinder> {
        let actor_map = world.actor_map();
        let (mw, mh) = world.map().map_size;
        let grid_xs = div_away(mw, GRID_SIZE);
        let grid_ys = div_away(mh, GRID_SIZE);
        let blocking = (check == BlockedByActor::Immovable).then(|| RefCell::new(HashSet::new()));
        assert!(
            check == BlockedByActor::Immovable || check == BlockedByActor::None,
            "HierarchicalPathFinder supports None and Immovable only"
        );
        let hpf = Rc::new_cyclic(|me: &Weak<HierarchicalPathFinder>| HierarchicalPathFinder {
            world: Rc::downgrade(world),
            locomotor: locomotor.clone(),
            actor_map: actor_map.clone(),
            cost_estimator: OnceCell::new(),
            dirty_grid_indexes: RefCell::new(NetSet::new()),
            cells_with_blocking_actor: blocking,
            map_bottom_right: CPos::new(mw, mh),
            grid_xs,
            grid_ys,
            grid_infos: RefCell::new(vec![GridInfo::default(); (grid_xs * grid_ys) as usize]),
            abstract_graph: RefCell::new(HashMap::new()),
            abstract_domains: RefCell::new(HashMap::new()),
            me: me.clone(),
        });
        if !locomotor.has_terrain_speeds() {
            return hpf;
        }
        if let Some(cells) = &hpf.cells_with_blocking_actor {
            let weak = hpf.me.clone();
            actor_map.on_cell_updated(Rc::new(move |c| {
                if let Some(h) = weak.upgrade() {
                    h.require_blocking_refresh_in_cell(c);
                }
            }));
            let mut set = cells.borrow_mut();
            for a in actor_map.all_actors() {
                if hpf.actor_is_blocking(&a) {
                    for (c, _) in a.cache().occupies_space.as_ref().expect("occupies space").occupied_cells() {
                        if hpf.actor_cell_is_blocking(&a, c) {
                            set.insert(c);
                        }
                    }
                }
            }
        }
        let _ = hpf.cost_estimator.set(CostEstimator::new(&locomotor));
        hpf.build_grids();
        hpf.build_cost_table();
        hpf.rebuild_domains();
        let weak = hpf.me.clone();
        locomotor.on_cell_cost_changed(Rc::new(move |c, old, new| {
            if let Some(h) = weak.upgrade() {
                if (old == MOVEMENT_COST_FOR_UNREACHABLE_CELL) != (new == MOVEMENT_COST_FOR_UNREACHABLE_CELL) {
                    h.dirty_grid_indexes.borrow_mut().insert(h.grid_index(c));
                }
            }
        }));
        hpf
    }

    fn world(&self) -> Rc<World> {
        self.world.upgrade().expect("world alive")
    }

    fn est(&self) -> CostEstimator {
        *self.cost_estimator.get().expect("estimator")
    }

    fn map_bounds_contains(&self, c: CPos) -> bool {
        c.x() >= 0 && c.x() < self.map_bottom_right.x() && c.y() >= 0 && c.y() < self.map_bottom_right.y()
    }

    fn grid_index(&self, c: CPos) -> i32 {
        c.y() / GRID_SIZE * self.grid_xs + c.x() / GRID_SIZE
    }

    fn get_grid(&self, cell_in_grid: CPos) -> Grid {
        let tl = grid_top_left_of(cell_in_grid);
        let width = (self.map_bottom_right.x() - tl.x()).min(GRID_SIZE);
        let height = (self.map_bottom_right.y() - tl.y()).min(GRID_SIZE);
        Grid {
            top_left: tl,
            bottom_right: tl + CVec::new(width, height),
        }
    }

    fn build_grids(&self) {
        let g = self.map_bottom_right;
        let mut x = 0;
        while x < g.x() {
            let mut y = 0;
            while y < g.y() {
                let info = self.build_grid(x, y);
                let i = self.grid_index(CPos::new(x, y)) as usize;
                self.grid_infos.borrow_mut()[i] = info;
                y += GRID_SIZE;
            }
            x += GRID_SIZE;
        }
    }

    fn blocking_custom_cost(&self) -> Option<CustomCost> {
        self.cells_with_blocking_actor.as_ref()?;
        let weak = self.me.clone();
        Some(Rc::new(move |c| {
            let h = weak.upgrade().expect("hpf alive");
            if h.cells_with_blocking_actor.as_ref().expect("blocking set").borrow().contains(&c) {
                PATH_COST_FOR_INVALID_PATH
            } else {
                0
            }
        }))
    }

    fn build_grid(&self, grid_x: i32, grid_y: i32) -> GridInfo {
        let world = self.world();
        let mut info = GridInfo::default();
        let custom_cost = self.blocking_custom_cost();
        let grid = self.get_grid(CPos::new(grid_x, grid_y));
        let mut accessible: NetSet<CPos> = NetSet::new();
        for y in grid_y..grid.bottom_right.y() {
            for x in grid_x..grid.bottom_right.x() {
                let cell = CPos::new(x, y);
                if self.cell_is_accessible(cell) {
                    accessible.insert(cell);
                }
            }
        }
        let mut has_populated = false;
        while !accessible.is_empty() {
            let src = *accessible.iter().next().expect("not empty");
            let mut search = PathSearch::to_target_cell(
                &world,
                &self.locomotor,
                None,
                &[src],
                src,
                BlockedByActor::None,
                100,
                custom_cost.clone(),
                None,
                false,
                false,
                None,
                Some(grid),
            );
            let region = search.expand_all();
            let abstract_cell = abstract_cell_for_local_cells(&region);
            for c in &region {
                accessible.remove(c);
            }
            if !has_populated && accessible.is_empty() {
                info.single_abstract_cell = Some(abstract_cell);
            } else {
                has_populated = true;
                for c in region {
                    info.local_cell_to_abstract_cell.insert(c, abstract_cell);
                }
            }
        }
        info
    }

    fn build_cost_table(&self) {
        let mut x = 0;
        while x < self.map_bottom_right.x() {
            let mut y = 0;
            while y < self.map_bottom_right.y() {
                for (k, v) in self.get_abstract_edges_for_grid(x, y) {
                    self.abstract_graph.borrow_mut().insert(k, v);
                }
                y += GRID_SIZE;
            }
            x += GRID_SIZE;
        }
    }

    fn get_abstract_edges_for_grid(&self, grid_x: i32, grid_y: i32) -> Vec<(CPos, Vec<GraphConnection>)> {
        let mut edges: IndexSet<(CPos, CPos)> = IndexSet::new();
        let mut add = |x_inc: i32, y_inc: i32, adjacent: CVec, off_x: i32, off_y: i32| {
            let (start_y, start_x) = (grid_y + off_y, grid_x + off_x);
            let mut y = start_y;
            while y < start_y + GRID_SIZE {
                let mut x = start_x;
                while x < start_x + GRID_SIZE {
                    let cell = CPos::new(x, y);
                    if self.cell_is_accessible(cell) {
                        let adjacent_cell = cell + adjacent;
                        for i in -1..=1 {
                            let candidate = adjacent_cell + CVec::new(adjacent.y, adjacent.x) * i;
                            if !self.movement_allowed_between_cells(candidate) {
                                continue;
                            }
                            let Some(a) = self.abstract_cell_no_accessible_check(cell) else { continue };
                            let Some(b) = self.abstract_cell_no_accessible_check(candidate) else { continue };
                            edges.insert((a, b));
                        }
                    }
                    x += x_inc;
                }
                y += y_inc;
            }
        };
        add(1, GRID_SIZE, CVec::new(0, -1), 0, 0);
        add(GRID_SIZE, 1, CVec::new(-1, 0), 0, 0);
        add(1, GRID_SIZE, CVec::new(0, 1), 0, GRID_SIZE - 1);
        add(GRID_SIZE, 1, CVec::new(1, 0), GRID_SIZE - 1, 0);

        let est = self.est();
        let mut out: indexmap::IndexMap<CPos, Vec<GraphConnection>> = indexmap::IndexMap::new();
        for (s, d) in edges {
            out.entry(s).or_default().push(GraphConnection {
                destination: d,
                cost: est.estimate(s, d),
            });
        }
        out.into_iter().collect()
    }

    fn cell_is_accessible(&self, cell: CPos) -> bool {
        self.locomotor.movement_cost_for_cell(cell) != MOVEMENT_COST_FOR_UNREACHABLE_CELL
            && !self.cells_with_blocking_actor.as_ref().is_some_and(|s| s.borrow().contains(&cell))
    }

    fn movement_allowed_between_cells(&self, dest: CPos) -> bool {
        self.locomotor
            .movement_cost_to_enter_cell(None, dest, BlockedByActor::None, None, false, FULL_CELL)
            != MOVEMENT_COST_FOR_UNREACHABLE_CELL
            && !self.cells_with_blocking_actor.as_ref().is_some_and(|s| s.borrow().contains(&dest))
    }

    fn require_blocking_refresh_in_cell(&self, cell: CPos) {
        let mut has_blocking = false;
        for actor in self.actor_map.get_actors_at(cell) {
            if self.actor_is_blocking(&actor) && self.actor_cell_is_blocking(&actor, cell) {
                has_blocking = true;
                break;
            }
        }
        let set = self.cells_with_blocking_actor.as_ref().expect("blocking set");
        let changed = if has_blocking { set.borrow_mut().insert(cell) } else { set.borrow_mut().remove(&cell) };
        if changed {
            self.dirty_grid_indexes.borrow_mut().insert(self.grid_index(cell));
        }
    }

    fn actor_is_blocking(&self, actor: &ActorRef) -> bool {
        if crate::mobile::mobile_of(actor).is_some_and(crate::mobile::Mobile::is_movable) {
            return false;
        }
        for c in &actor.cache().crushables {
            if c.crushable_by(actor, self.locomotor.info.crushes) != 0 {
                return false;
            }
        }
        true
    }

    fn actor_cell_is_blocking(&self, actor: &ActorRef, cell: CPos) -> bool {
        if self.locomotor.info.shares_cell && self.actor_map.has_free_subcell(cell, true) {
            return false;
        }
        !building_of(actor).is_some_and(|b| b.transit_only.contains(&cell))
    }

    fn abstract_cell_for_local_cell(&self, c: CPos) -> Option<CPos> {
        let infos = self.grid_infos.borrow();
        let g = &infos[self.grid_index(c) as usize];
        if let Some(a) = g.single_abstract_cell {
            if !self.cell_is_accessible(c) {
                return None;
            }
            return Some(a);
        }
        g.local_cell_to_abstract_cell.get(&c).copied()
    }

    fn abstract_cell_no_accessible_check(&self, c: CPos) -> Option<CPos> {
        let infos = self.grid_infos.borrow();
        let g = &infos[self.grid_index(c) as usize];
        if let Some(a) = g.single_abstract_cell {
            return Some(a);
        }
        g.local_cell_to_abstract_cell.get(&c).copied()
    }

    fn edge_from_local_to_abstract(&self, local: CPos, abs: CPos) -> Option<GraphEdge> {
        if local == abs {
            return None;
        }
        Some(GraphEdge {
            source: local,
            destination: abs,
            cost: self.est().estimate(local, abs),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn find_path_multi(
        self: &Rc<Self>,
        self_: &ActorRef,
        sources: &[CPos],
        target: CPos,
        check: BlockedByActor,
        weight: i32,
        custom_cost: Option<CustomCost>,
        ignore_actor: Option<&ActorRef>,
        in_reverse: bool,
        lane_bias: bool,
    ) -> Vec<CPos> {
        if self.cost_estimator.get().is_none() {
            return Vec::new();
        }
        let world = self.world();
        if !world.map().contains(target) {
            return Vec::new();
        }
        self.rebuild_dirty_grids();
        let Some(target_abstract) = self.abstract_cell_for_local_cell(target) else { return Vec::new() };
        self.rebuild_domains();
        let target_domain = self.domain(target_abstract);

        let mut pathable: IndexSet<CPos> = IndexSet::new();
        let mut source_edges = Vec::new();
        let mut unpathable: Vec<CPos> = Vec::new();
        for &source in sources {
            if !world.map().contains(source) {
                continue;
            }
            if let Some(a) = self.abstract_cell_for_local_cell(source) {
                if self.domain(a) != target_domain {
                    continue;
                }
                pathable.insert(source);
                if let Some(e) = self.edge_from_local_to_abstract(source, a) {
                    source_edges.push(e);
                }
                continue;
            }
            for dir in CVec::DIRECTIONS {
                let adjacent = source + dir;
                if !self.movement_allowed_between_cells(adjacent) {
                    continue;
                }
                let Some(a) = self.abstract_cell_for_local_cell(adjacent) else { continue };
                if self.domain(a) != target_domain {
                    unpathable.push(adjacent);
                    continue;
                }
                pathable.insert(source);
                if let Some(e) = self.edge_from_local_to_abstract(adjacent, a) {
                    source_edges.push(e);
                }
            }
        }
        if pathable.is_empty() {
            return Vec::new();
        }
        let target_edge = self.edge_from_local_to_abstract(target, target_abstract);
        let full = AbstractGraphWithInsertedEdges::new(self.clone(), &source_edges, target_edge);
        let f2 = full.clone();
        let reverse = PathSearch::to_target_cell_over_graph(Rc::new(move |c| f2.get_connections(c)), &self.locomotor, target, target);
        let srcs: Vec<CPos> = pathable.iter().copied().collect();
        let heuristic = self.make_heuristic(reverse, Some(pathable), (!unpathable.is_empty()).then_some(unpathable));
        let mut from_src = PathSearch::to_target_cell(
            &world,
            &self.locomotor,
            Some(self_),
            &srcs,
            target,
            check,
            weight,
            custom_cost,
            ignore_actor,
            lane_bias,
            in_reverse,
            Some(heuristic),
            None,
        );
        from_src.find_path()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn find_path(
        self: &Rc<Self>,
        self_: &ActorRef,
        source: CPos,
        target: CPos,
        check: BlockedByActor,
        weight: i32,
        custom_cost: Option<CustomCost>,
        ignore_actor: Option<&ActorRef>,
        in_reverse: bool,
        lane_bias: bool,
    ) -> Vec<CPos> {
        if self.cost_estimator.get().is_none() {
            return Vec::new();
        }
        let world = self.world();
        const CLOSE_GRID_DISTANCE: i32 = 2;
        if (target - source).length_squared() < GRID_SIZE * GRID_SIZE * CLOSE_GRID_DISTANCE * CLOSE_GRID_DISTANCE {
            let grid_to_search = Grid {
                top_left: CPos::new(source.x().min(target.x()) - GRID_SIZE / 2, source.y().min(target.y()) - GRID_SIZE / 2),
                bottom_right: CPos::new(source.x().max(target.x()) + GRID_SIZE / 2, source.y().max(target.y()) + GRID_SIZE / 2),
            };
            let mut search = PathSearch::to_target_cell(
                &world,
                &self.locomotor,
                Some(self_),
                &[source],
                target,
                check,
                weight,
                custom_cost.clone(),
                ignore_actor,
                lane_bias,
                in_reverse,
                None,
                Some(grid_to_search),
            );
            let local_path = search.find_path();
            if !local_path.is_empty() {
                return local_path;
            }
        }
        self.rebuild_dirty_grids();
        let Some(target_abstract) = self.abstract_cell_for_local_cell(target) else { return Vec::new() };
        let Some(source_abstract) = self.abstract_cell_for_local_cell(source) else {
            return self.find_path_multi(self_, &[source], target, check, weight, custom_cost, ignore_actor, in_reverse, lane_bias);
        };
        self.rebuild_domains();
        if self.domain(source_abstract) != self.domain(target_abstract) {
            return Vec::new();
        }
        let target_edge = self.edge_from_local_to_abstract(target, target_abstract);
        let source_edge = self.edge_from_local_to_abstract(source, source_abstract);
        let source_edges: Vec<GraphEdge> = source_edge.into_iter().collect();
        let full = AbstractGraphWithInsertedEdges::new(self.clone(), &source_edges, target_edge);
        let f2 = full.clone();
        let mut forward = PathSearch::to_target_cell_over_graph(Rc::new(move |c| f2.get_connections(c)), &self.locomotor, source, target);
        if !forward.expand_to_target() {
            return Vec::new();
        }
        let f3 = full.clone();
        let mut reverse = PathSearch::to_target_cell_over_graph(Rc::new(move |c| f3.get_connections(c)), &self.locomotor, target, source);
        reverse.expand_to_target();
        let mut from_src = PathSearch::to_target_cell(
            &world,
            &self.locomotor,
            Some(self_),
            &[source],
            target,
            check,
            weight,
            custom_cost.clone(),
            ignore_actor,
            lane_bias,
            in_reverse,
            Some(self.make_heuristic(reverse, None, None)),
            None,
        );
        let mut from_dest = PathSearch::to_target_cell(
            &world,
            &self.locomotor,
            Some(self_),
            &[target],
            source,
            check,
            weight,
            custom_cost,
            ignore_actor,
            lane_bias,
            !in_reverse,
            Some(self.make_heuristic(forward, None, None)),
            None,
        );
        PathSearch::find_bidi_path(&mut from_dest, &mut from_src)
    }

    pub fn path_exists(&self, source: CPos, target: CPos) -> bool {
        if self.cost_estimator.get().is_none() {
            return false;
        }
        let w = self.world();
        if !w.map().contains(source) || !w.map().contains(target) {
            return false;
        }
        self.rebuild_domains();
        let Some(abs_target) = self.abstract_cell_for_local_cell(target) else { return false };
        let target_domain = self.domain(abs_target);
        if let Some(a) = self.abstract_cell_for_local_cell(source) {
            return self.domain(a) == target_domain;
        }
        for dir in CVec::DIRECTIONS {
            let adjacent = source + dir;
            if !self.movement_allowed_between_cells(adjacent) {
                continue;
            }
            let Some(a) = self.abstract_cell_for_local_cell(adjacent) else { continue };
            if self.domain(a) == target_domain {
                return true;
            }
        }
        false
    }

    fn domain(&self, c: CPos) -> u32 {
        self.abstract_domains.borrow()[&c]
    }

    fn rebuild_dirty_grids(&self) {
        if self.dirty_grid_indexes.borrow().is_empty() {
            return;
        }
        self.abstract_domains.borrow_mut().clear();
        let dirty = self.dirty_grid_indexes.borrow().to_vec();
        for grid_index in dirty {
            let old = self.grid_infos.borrow()[grid_index as usize].clone();
            let x = grid_index % self.grid_xs * GRID_SIZE;
            let y = grid_index / self.grid_xs * GRID_SIZE;
            let info = self.build_grid(x, y);
            self.grid_infos.borrow_mut()[grid_index as usize] = info;
            self.rebuild_cost_table(x, y, &old);
        }
        self.dirty_grid_indexes.borrow_mut().clear();
    }

    fn rebuild_cost_table(&self, grid_x: i32, grid_y: i32, old: &GridInfo) {
        let mut nodes = NetSet::new();
        old.copy_abstract_cells_into(&mut nodes);
        for n in nodes.iter() {
            self.abstract_graph.borrow_mut().remove(n);
        }
        nodes.clear();
        for (k, v) in self.get_abstract_edges_for_grid(grid_x, grid_y) {
            self.abstract_graph.borrow_mut().insert(k, v);
        }
        for direction in CVec::DIRECTIONS {
            let adjacent = CPos::new(grid_x, grid_y) + direction * GRID_SIZE;
            if !self.map_bounds_contains(adjacent) {
                continue;
            }
            self.grid_infos.borrow()[self.grid_index(adjacent) as usize].copy_abstract_cells_into(&mut nodes);
            for (k, v) in self.get_abstract_edges_for_grid(adjacent.x(), adjacent.y()) {
                nodes.remove(&k);
                self.abstract_graph.borrow_mut().insert(k, v);
            }
            for n in nodes.iter() {
                self.abstract_graph.borrow_mut().remove(n);
            }
            nodes.clear();
        }
    }

    fn rebuild_domains(&self) {
        self.rebuild_dirty_grids();
        if !self.abstract_domains.borrow().is_empty() {
            return;
        }
        let mut domain = 0u32;
        let mut cells: NetSet<CPos> = NetSet::new();
        for g in self.grid_infos.borrow().iter() {
            g.copy_abstract_cells_into(&mut cells);
        }
        while !cells.is_empty() {
            let search_cell = *cells.iter().next().expect("not empty");
            let weak = self.me.clone();
            let mut search = PathSearch::to_target_cell_over_graph(
                Rc::new(move |c| {
                    let h = weak.upgrade().expect("hpf alive");
                    let g = h.abstract_graph.borrow();
                    g.get(&c).cloned().unwrap_or_default()
                }),
                &self.locomotor,
                search_cell,
                search_cell,
            );
            let searched = search.expand_all();
            let mut d = self.abstract_domains.borrow_mut();
            for c in searched {
                d.insert(c, domain);
                cells.remove(&c);
            }
            domain += 1;
        }
    }

    fn make_heuristic(self: &Rc<Self>, mut abstract_search: PathSearch, sources: Option<IndexSet<CPos>>, unpathable: Option<Vec<CPos>>) -> Heuristic {
        let hpf = self.clone();
        let mut node_for_cost_lookup: HashMap<CPos, CPos> = HashMap::new();
        Box::new(move |cell, known_accessible| {
            if unpathable.as_ref().is_some_and(|u| u.contains(&cell)) {
                return PATH_COST_FOR_INVALID_PATH;
            }
            let mut maybe = if known_accessible {
                hpf.abstract_cell_no_accessible_check(cell)
            } else {
                hpf.abstract_cell_for_local_cell(cell)
            };
            if maybe.is_none() {
                if sources.as_ref().is_some_and(|s| s.contains(&cell)) {
                    for dir in CVec::DIRECTIONS {
                        let adjacent = cell + dir;
                        if !hpf.movement_allowed_between_cells(adjacent) || unpathable.as_ref().is_some_and(|u| u.contains(&adjacent)) {
                            continue;
                        }
                        maybe = hpf.abstract_cell_for_local_cell(adjacent);
                        if maybe.is_some() {
                            break;
                        }
                    }
                }
                assert!(
                    maybe.is_some(),
                    "The abstract path should never be searched for an unreachable point. Cell {cell} failed lookup for an abstract cell."
                );
            }
            let abstract_cell = maybe.unwrap();
            let mut info = abstract_search.graph.get(abstract_cell);
            if info.status != CellStatus::Closed {
                abstract_search.target_predicate = Box::new(move |c| c == abstract_cell);
                assert!(
                    abstract_search.expand_to_target(),
                    "The abstract path should never be searched for an unreachable point. Abstract cell {abstract_cell} failed to route."
                );
                info = abstract_search.graph.get(abstract_cell);
            }
            let abstract_node = info.previous_node;
            let node_for_cost = *node_for_cost_lookup
                .entry(abstract_node)
                .or_insert_with(|| hpf.abstract_node_for_cost(&abstract_search.graph, abstract_cell, abstract_node));
            abstract_search.graph.get(node_for_cost).cost_so_far.wrapping_add(hpf.est().estimate(cell, node_for_cost))
        })
    }

    fn abstract_node_for_cost(&self, graph: &Graph, abstract_cell: CPos, mut abstract_node: CPos) -> CPos {
        let mut along: Vec<CPos> = Vec::new();
        loop {
            let previous = graph.get(abstract_node).previous_node;
            if previous == abstract_node {
                break;
            }
            along.push(abstract_node);
            if !along.iter().all(|n| self.get_grid(*n).intersects_line(abstract_cell, previous)) {
                break;
            }
            abstract_node = previous;
        }
        abstract_node
    }
}

fn abstract_cell_for_local_cells(cells: &[CPos]) -> CPos {
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for c in cells {
        min_x = min_x.min(c.x());
        min_y = min_y.min(c.y());
        max_x = max_x.max(c.x());
        max_y = max_y.max(c.y());
    }
    let desired = CPos::new(min_x + (max_x - min_x) / 2, min_y + (max_y - min_y) / 2);
    let mut best = desired;
    let mut distance = i32::MAX;
    for c in cells {
        let d = (*c - desired).length_squared();
        if distance > d {
            distance = d;
            best = *c;
        }
    }
    best
}
