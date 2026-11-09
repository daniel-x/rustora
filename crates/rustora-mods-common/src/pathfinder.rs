// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Mods.Common/Pathfinder` (searches) and `Primitives/PriorityQueue.cs`.
//! Custom movement layers are not used by RA and left out.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::world_types::{CPos, CVec, Int2};
use rustora_engine::actor::ActorRef;
use rustora_engine::map::Map;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::world::World;

use crate::locomotor::{BlockedByActor, Locomotor, MOVEMENT_COST_FOR_UNREACHABLE_CELL};

pub const PATH_COST_FOR_INVALID_PATH: i32 = i32::MAX;
const LANE_BIAS_COST: i32 = 1;

pub type CustomCost = Rc<dyn Fn(CPos) -> i32>;
pub type Heuristic = Box<dyn FnMut(CPos, bool) -> i32>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CellStatus {
    #[default]
    Unvisited,
    Open,
    Closed,
}

#[derive(Clone, Copy, Debug)]
pub struct CellInfo {
    pub status: CellStatus,
    pub cost_so_far: i32,
    pub estimated_total_cost: i32,
    pub previous_node: CPos,
}

impl Default for CellInfo {
    fn default() -> Self {
        CellInfo {
            status: CellStatus::Unvisited,
            cost_so_far: 0,
            estimated_total_cost: 0,
            previous_node: CPos::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rustora_engine::persist::Persist)]
#[persist(value)]
pub struct GraphConnection {
    pub destination: CPos,
    pub cost: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct GraphEdge {
    pub source: CPos,
    pub destination: CPos,
    pub cost: i32,
}

/// Binary min-heap with the exact sift behavior of the C# `PriorityQueue`.
pub struct PriorityQueue {
    items: Vec<GraphConnection>,
    level: i32,
    index: i32,
}

const EMPTY_CONN: GraphConnection = GraphConnection {
    destination: CPos::ZERO,
    cost: 0,
};

fn idx(level: i32, index: i32) -> usize {
    ((1i32 << level) - 1 + index) as usize
}

fn above_idx(level: i32, index: i32) -> usize {
    ((1i32 << (level - 1)) - 1 + (index >> 1)) as usize
}

impl PriorityQueue {
    pub fn new() -> Self {
        PriorityQueue {
            items: vec![EMPTY_CONN; 1],
            level: 0,
            index: 0,
        }
    }

    pub fn add(&mut self, item: GraphConnection) {
        let mut add_level = self.level;
        let mut add_index = self.index;
        while add_level >= 1 {
            let above = self.items[above_idx(add_level, add_index)];
            if above.cost > item.cost {
                self.items[idx(add_level, add_index)] = above;
                add_level -= 1;
                add_index >>= 1;
            } else {
                break;
            }
        }
        self.items[idx(add_level, add_index)] = item;
        self.index += 1;
        if self.index >= 1 << self.level {
            self.index = 0;
            self.level += 1;
            let count = 2 * (1usize << self.level);
            if count > self.items.len() {
                self.items.resize(count, EMPTY_CONN);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.level == 0
    }

    pub fn peek(&self) -> GraphConnection {
        assert!(self.level > 0 || self.index > 0, "PriorityQueue empty.");
        self.items[0]
    }

    fn index_last(&self) -> usize {
        let mut last_level = self.level;
        let mut last_index = self.index - 1;
        if last_index < 0 {
            last_level -= 1;
            last_index = (1 << last_level) - 1;
        }
        idx(last_level, last_index)
    }

    pub fn pop(&mut self) -> GraphConnection {
        let ret = self.peek();
        let last = self.items[self.index_last()];
        self.bubble_into(0, 0, last);
        self.index -= 1;
        if self.index < 0 {
            self.level -= 1;
            self.index = (1 << self.level) - 1;
        }
        ret
    }

    fn bubble_into(&mut self, mut into_level: i32, mut into_index: i32, val: GraphConnection) {
        loop {
            let down_level = into_level + 1;
            let mut down_index = into_index << 1;
            if down_level > self.level || (down_level == self.level && down_index >= self.index) {
                self.items[idx(into_level, into_index)] = val;
                return;
            }
            let mut down = self.items[idx(down_level, down_index)];
            if down_level < self.level || (down_level == self.level && down_index + 1 < self.index) {
                let down2 = self.items[idx(down_level, down_index + 1)];
                if down.cost >= down2.cost {
                    down_index += 1;
                    down = down2;
                }
            }
            if val.cost <= down.cost {
                self.items[idx(into_level, into_index)] = val;
                return;
            }
            self.items[idx(into_level, into_index)] = down;
            into_level = down_level;
            into_index = down_index;
        }
    }
}

impl Default for PriorityQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub top_left: CPos,
    pub bottom_right: CPos,
}

fn winding(v0: Int2, v1: Int2, p: Int2) -> i32 {
    ((v1.x - v0.x) * (p.y - v0.y) - (p.x - v0.x) * (v1.y - v0.y)).signum()
}

fn lines_intersect(a: Int2, b: Int2, c: Int2, d: Int2) -> bool {
    winding(c, d, a) != winding(c, d, b) && winding(a, b, c) != winding(a, b, d)
}

impl Grid {
    pub fn width(&self) -> i32 {
        self.bottom_right.x() - self.top_left.x()
    }

    pub fn height(&self) -> i32 {
        self.bottom_right.y() - self.top_left.y()
    }

    pub fn contains(&self, c: CPos) -> bool {
        c.x() >= self.top_left.x() && c.x() < self.bottom_right.x() && c.y() >= self.top_left.y() && c.y() < self.bottom_right.y()
    }

    pub fn intersects_line(&self, start: CPos, end: CPos) -> bool {
        let s = Int2::new(start.x(), start.y());
        let e = Int2::new(end.x(), end.y());
        let tl = Int2::new(self.top_left.x(), self.top_left.y());
        let tr = Int2::new(self.bottom_right.x(), self.top_left.y());
        let bl = Int2::new(self.top_left.x(), self.bottom_right.y());
        let br = Int2::new(self.bottom_right.x(), self.bottom_right.y());
        lines_intersect(s, e, tl, tr) || lines_intersect(s, e, tl, bl) || lines_intersect(s, e, bl, br) || lines_intersect(s, e, tr, br)
    }
}

thread_local! {
    static LAYER_POOL: RefCell<Vec<Vec<CellInfo>>> = const { RefCell::new(Vec::new()) };
}

const MAX_POOL_SIZE: usize = 4;

fn layer_from_pool(len: usize) -> Vec<CellInfo> {
    let reused = LAYER_POOL.with(|p| p.borrow_mut().pop());
    match reused {
        Some(mut l) if l.len() == len => {
            l.iter_mut().for_each(|c| *c = CellInfo::default());
            l
        }
        _ => vec![CellInfo::default(); len],
    }
}

const fn v(x: i32, y: i32) -> CVec {
    CVec::new(x, y)
}

static DIRECTED_NEIGHBORS: [&[CVec]; 9] = [
    &[v(-1, -1), v(0, -1), v(1, -1), v(-1, 0), v(-1, 1)],
    &[v(-1, -1), v(0, -1), v(1, -1)],
    &[v(-1, -1), v(0, -1), v(1, -1), v(1, 0), v(1, 1)],
    &[v(-1, -1), v(-1, 0), v(-1, 1)],
    &CVec::DIRECTIONS,
    &[v(1, -1), v(1, 0), v(1, 1)],
    &[v(-1, -1), v(-1, 0), v(-1, 1), v(0, 1), v(1, 1)],
    &[v(-1, 1), v(0, 1), v(1, 1)],
    &[v(1, -1), v(1, 0), v(-1, 1), v(0, 1), v(1, 1)],
];

pub fn multiply_by_sqrt_two(n: i16) -> i32 {
    (n as i32).wrapping_mul(46341) / 32768
}

/// Common data of `DensePathGraph`.
pub struct DenseParams {
    pub locomotor: Rc<Locomotor>,
    pub actor: Option<ActorRef>,
    pub check: BlockedByActor,
    pub custom_cost: Option<CustomCost>,
    pub ignore_actor: Option<ActorRef>,
    pub lane_bias: bool,
    pub in_reverse: bool,
}

pub type EdgesFn = Rc<dyn Fn(CPos) -> Vec<GraphConnection>>;

pub enum Graph {
    Map { p: DenseParams, infos: Vec<CellInfo>, width: i32 },
    Grid { p: DenseParams, infos: Vec<CellInfo>, grid: Grid },
    Sparse { edges: EdgesFn, info: HashMap<CPos, CellInfo> },
}

impl Graph {
    #[inline]
    pub fn get(&self, c: CPos) -> CellInfo {
        match self {
            Graph::Map { infos, width, .. } => infos[(c.y() * width + c.x()) as usize],
            Graph::Grid { infos, grid, .. } => infos[((c.y() - grid.top_left.y()) * grid.width() + (c.x() - grid.top_left.x())) as usize],
            Graph::Sparse { info, .. } => info.get(&c).copied().unwrap_or_default(),
        }
    }

    #[inline]
    pub fn set(&mut self, c: CPos, v: CellInfo) {
        match self {
            Graph::Map { infos, width, .. } => infos[(c.y() * *width + c.x()) as usize] = v,
            Graph::Grid { infos, grid, .. } => {
                infos[((c.y() - grid.top_left.y()) * grid.width() + (c.x() - grid.top_left.x())) as usize] = v
            }
            Graph::Sparse { info, .. } => {
                info.insert(c, v);
            }
        }
    }

    fn get_connections(&self, position: CPos, target_predicate: &dyn Fn(CPos) -> bool) -> Vec<GraphConnection> {
        let (p, grid) = match self {
            Graph::Sparse { edges, .. } => return edges(position),
            Graph::Map { p, .. } => (p, None),
            Graph::Grid { p, grid, .. } => (p, Some(*grid)),
        };
        let info = self.get(position);
        let dx = position.x() - info.previous_node.x();
        let dy = position.y() - info.previous_node.y();
        let directions = DIRECTED_NEIGHBORS[(dy * 3 + dx + 4) as usize];
        let mut valid = Vec::with_capacity(directions.len());
        for &dir in directions {
            let neighbor = position + dir;
            if grid.is_some_and(|g| !g.contains(neighbor)) {
                continue;
            }
            let path_cost = get_path_cost_to_node(p, neighbor, dir, target_predicate);
            if path_cost != PATH_COST_FOR_INVALID_PATH && self.get(neighbor).status != CellStatus::Closed {
                valid.push(GraphConnection {
                    destination: neighbor,
                    cost: path_cost,
                });
            }
        }
        valid
    }
}

fn get_path_cost_to_node(p: &DenseParams, dest: CPos, direction: CVec, target_predicate: &dyn Fn(CPos) -> bool) -> i32 {
    let mut cost = p
        .locomotor
        .movement_cost_to_enter_cell(p.actor.as_ref(), dest, p.check, p.ignore_actor.as_ref(), false, FULL_CELL);
    if cost == MOVEMENT_COST_FOR_UNREACHABLE_CELL && p.in_reverse && target_predicate(dest) {
        cost = 0;
    }
    if cost != MOVEMENT_COST_FOR_UNREACHABLE_CELL {
        return calculate_cell_path_cost(p, dest, direction, cost);
    }
    PATH_COST_FOR_INVALID_PATH
}

fn calculate_cell_path_cost(p: &DenseParams, neighbor: CPos, direction: CVec, movement_cost: i16) -> i32 {
    let mut cell_cost = if direction.x * direction.y != 0 {
        multiply_by_sqrt_two(movement_cost)
    } else {
        movement_cost as i32
    };
    if let Some(cc) = &p.custom_cost {
        let custom = cc(neighbor);
        if custom == PATH_COST_FOR_INVALID_PATH {
            return PATH_COST_FOR_INVALID_PATH;
        }
        cell_cost = cell_cost.wrapping_add(custom);
    }
    if p.lane_bias {
        let r = i32::from(p.in_reverse);
        let ux = (neighbor.x() + r) & 1;
        let uy = (neighbor.y() + r) & 1;
        if (ux == 0 && direction.y < 0) || (ux == 1 && direction.y > 0) {
            cell_cost += LANE_BIAS_COST;
        }
        if (uy == 0 && direction.x < 0) || (uy == 1 && direction.x > 0) {
            cell_cost += LANE_BIAS_COST;
        }
    }
    cell_cost
}

pub fn cell_allows_movement(w: &World, cell: CPos, custom_cost: Option<&CustomCost>) -> bool {
    w.map().contains(cell) && custom_cost.is_none_or(|c| c(cell) != PATH_COST_FOR_INVALID_PATH)
}

/// `DefaultCostEstimator(locomotor)`.
#[derive(Clone, Copy)]
pub struct CostEstimator {
    cell_cost: i32,
    diagonal_cell_cost: i32,
}

impl CostEstimator {
    pub fn new(locomotor: &Locomotor) -> Self {
        let c = locomotor.min_terrain_cost();
        CostEstimator {
            cell_cost: c as i32,
            diagonal_cell_cost: multiply_by_sqrt_two(c),
        }
    }

    #[inline]
    pub fn estimate(&self, here: CPos, dest: CPos) -> i32 {
        let dx = (here.x() - dest.x()).abs();
        let dy = (here.y() - dest.y()).abs();
        self.cell_cost * (dx + dy) + (self.diagonal_cell_cost - 2 * self.cell_cost) * dx.min(dy)
    }
}

pub struct PathSearch {
    pub graph: Graph,
    pub target_predicate: Box<dyn Fn(CPos) -> bool>,
    heuristic: Heuristic,
    heuristic_weight_percentage: i32,
    open_queue: PriorityQueue,
}

impl Drop for PathSearch {
    fn drop(&mut self) {
        if let Graph::Map { infos, .. } = &mut self.graph {
            let l = std::mem::take(infos);
            LAYER_POOL.with(|p| {
                let mut p = p.borrow_mut();
                if p.len() < MAX_POOL_SIZE {
                    p.push(l);
                }
            });
        }
    }
}

fn map_graph(map: &Map, p: DenseParams) -> Graph {
    let (w, h) = map.map_size;
    Graph::Map {
        p,
        infos: layer_from_pool((w * h) as usize),
        width: w,
    }
}

impl PathSearch {
    fn new(graph: Graph, heuristic: Heuristic, weight: i32, target_predicate: Box<dyn Fn(CPos) -> bool>) -> Self {
        PathSearch {
            graph,
            target_predicate,
            heuristic,
            heuristic_weight_percentage: weight,
            open_queue: PriorityQueue::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn to_target_cell_by_predicate(
        world: &World,
        locomotor: &Rc<Locomotor>,
        self_: Option<&ActorRef>,
        froms: &[CPos],
        target_predicate: Box<dyn Fn(CPos) -> bool>,
        check: BlockedByActor,
        custom_cost: Option<CustomCost>,
        ignore_actor: Option<&ActorRef>,
        lane_bias: bool,
    ) -> PathSearch {
        let graph = map_graph(
            world.map(),
            DenseParams {
                locomotor: locomotor.clone(),
                actor: self_.cloned(),
                check,
                custom_cost: custom_cost.clone(),
                ignore_actor: ignore_actor.cloned(),
                lane_bias,
                in_reverse: false,
            },
        );
        let mut search = PathSearch::new(graph, Box::new(|_, _| 0), 0, target_predicate);
        search.add_initial_cells(world, locomotor, self_, froms, check, custom_cost.as_ref(), ignore_actor, false);
        search
    }

    #[allow(clippy::too_many_arguments)]
    pub fn to_target_cell(
        world: &World,
        locomotor: &Rc<Locomotor>,
        self_: Option<&ActorRef>,
        froms: &[CPos],
        target: CPos,
        check: BlockedByActor,
        weight: i32,
        custom_cost: Option<CustomCost>,
        ignore_actor: Option<&ActorRef>,
        lane_bias: bool,
        in_reverse: bool,
        heuristic: Option<Heuristic>,
        grid: Option<Grid>,
    ) -> PathSearch {
        let p = DenseParams {
            locomotor: locomotor.clone(),
            actor: self_.cloned(),
            check,
            custom_cost: custom_cost.clone(),
            ignore_actor: ignore_actor.cloned(),
            lane_bias,
            in_reverse,
        };
        let graph = match grid {
            Some(g) => Graph::Grid {
                p,
                infos: vec![CellInfo::default(); (g.width() * g.height()) as usize],
                grid: g,
            },
            None => map_graph(world.map(), p),
        };
        let heuristic = heuristic.unwrap_or_else(|| {
            let e = CostEstimator::new(locomotor);
            Box::new(move |here, _| e.estimate(here, target))
        });
        let mut search = PathSearch::new(graph, heuristic, weight, Box::new(move |l| l == target));
        search.add_initial_cells(
            world,
            locomotor,
            self_,
            froms,
            check,
            custom_cost.as_ref(),
            ignore_actor,
            in_reverse,
        );
        search
    }

    #[allow(clippy::too_many_arguments)]
    fn add_initial_cells(
        &mut self,
        world: &World,
        locomotor: &Locomotor,
        self_: Option<&ActorRef>,
        froms: &[CPos],
        check: BlockedByActor,
        custom_cost: Option<&CustomCost>,
        ignore_actor: Option<&ActorRef>,
        in_reverse: bool,
    ) {
        for &sl in froms {
            if cell_allows_movement(world, sl, custom_cost)
                && (!in_reverse
                    || locomotor.movement_cost_to_enter_cell(self_, sl, check, ignore_actor, true, FULL_CELL)
                        != MOVEMENT_COST_FOR_UNREACHABLE_CELL)
            {
                self.add_initial_cell(sl, custom_cost);
            }
        }
    }

    pub fn to_target_cell_over_graph(edges: EdgesFn, locomotor: &Locomotor, from: CPos, target: CPos) -> PathSearch {
        let graph = Graph::Sparse {
            edges,
            info: HashMap::new(),
        };
        let e = CostEstimator::new(locomotor);
        let mut search = PathSearch::new(
            graph,
            Box::new(move |here, _| e.estimate(here, target)),
            100,
            Box::new(move |l| l == target),
        );
        search.add_initial_cell(from, None);
        search
    }

    fn add_initial_cell(&mut self, location: CPos, custom_cost: Option<&CustomCost>) {
        let mut initial_cost = 0;
        if let Some(c) = custom_cost {
            initial_cost = c(location);
            if initial_cost == PATH_COST_FOR_INVALID_PATH {
                return;
            }
        }
        let heuristic_cost = (self.heuristic)(location, false);
        if heuristic_cost == PATH_COST_FOR_INVALID_PATH {
            return;
        }
        let estimated = heuristic_cost.wrapping_mul(self.heuristic_weight_percentage) / 100;
        self.graph.set(
            location,
            CellInfo {
                status: CellStatus::Open,
                cost_so_far: initial_cost,
                estimated_total_cost: initial_cost.wrapping_add(estimated),
                previous_node: location,
            },
        );
        self.open_queue.add(GraphConnection {
            destination: location,
            cost: estimated,
        });
    }

    fn can_expand(&mut self) -> bool {
        loop {
            if self.open_queue.is_empty() {
                return false;
            }
            let status = self.graph.get(self.open_queue.peek().destination).status;
            if status == CellStatus::Closed {
                self.open_queue.pop();
            } else {
                return true;
            }
        }
    }

    fn expand(&mut self) -> CPos {
        let current = self.open_queue.pop().destination;
        let info = self.graph.get(current);
        self.graph.set(
            current,
            CellInfo {
                status: CellStatus::Closed,
                ..info
            },
        );
        let connections = self.graph.get_connections(current, &*self.target_predicate);
        for connection in connections {
            let cost_so_far = info.cost_so_far.wrapping_add(connection.cost);
            let neighbor = connection.destination;
            let ni = self.graph.get(neighbor);
            if ni.status == CellStatus::Closed || (ni.status == CellStatus::Open && cost_so_far >= ni.cost_so_far) {
                continue;
            }
            let remaining = if ni.status == CellStatus::Open {
                ni.estimated_total_cost.wrapping_sub(ni.cost_so_far)
            } else {
                let h = (self.heuristic)(neighbor, true);
                if h == PATH_COST_FOR_INVALID_PATH {
                    continue;
                }
                h.wrapping_mul(self.heuristic_weight_percentage) / 100
            };
            let total = cost_so_far.wrapping_add(remaining);
            self.graph.set(
                neighbor,
                CellInfo {
                    status: CellStatus::Open,
                    cost_so_far,
                    estimated_total_cost: total,
                    previous_node: current,
                },
            );
            self.open_queue.add(GraphConnection {
                destination: neighbor,
                cost: total,
            });
        }
        current
    }

    pub fn expand_to_target(&mut self) -> bool {
        while self.can_expand() {
            let c = self.expand();
            if (self.target_predicate)(c) {
                return true;
            }
        }
        false
    }

    pub fn expand_all(&mut self) -> Vec<CPos> {
        let mut out = Vec::new();
        while self.can_expand() {
            out.push(self.expand());
        }
        out
    }

    pub fn find_path(&mut self) -> Vec<CPos> {
        while self.can_expand() {
            let p = self.expand();
            if (self.target_predicate)(p) {
                return self.make_path(p);
            }
        }
        Vec::new()
    }

    fn make_path(&self, destination: CPos) -> Vec<CPos> {
        let mut ret = Vec::new();
        let mut current = destination;
        while self.graph.get(current).previous_node != current {
            ret.push(current);
            current = self.graph.get(current).previous_node;
        }
        ret.push(current);
        ret
    }

    pub fn find_bidi_path(first: &mut PathSearch, second: &mut PathSearch) -> Vec<CPos> {
        while first.can_expand() && second.can_expand() {
            let p = first.expand();
            let pi = second.graph.get(p);
            if pi.status == CellStatus::Closed && pi.cost_so_far != PATH_COST_FOR_INVALID_PATH {
                return Self::make_bidi_path(first, second, p);
            }
            let q = second.expand();
            let qi = first.graph.get(q);
            if qi.status == CellStatus::Closed && qi.cost_so_far != PATH_COST_FOR_INVALID_PATH {
                return Self::make_bidi_path(first, second, q);
            }
        }
        Vec::new()
    }

    fn make_bidi_path(first: &PathSearch, second: &PathSearch, confluence: CPos) -> Vec<CPos> {
        let mut ret = Vec::new();
        let mut q = confluence;
        let mut previous = first.graph.get(q).previous_node;
        while previous != q {
            ret.push(q);
            q = previous;
            previous = first.graph.get(q).previous_node;
        }
        ret.push(q);
        ret.reverse();
        q = confluence;
        previous = second.graph.get(q).previous_node;
        while previous != q {
            q = previous;
            previous = second.graph.get(q).previous_node;
            ret.push(q);
        }
        ret
    }
}
