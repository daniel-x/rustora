// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Mods.Common/Pathfinder` (searches) and `Primitives/PriorityQueue.cs`.
//! Custom movement layers are not used by RA and left out.

use rustora_core::hash::HashMap;
use std::cell::RefCell;
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
    /// The search generation of a pooled map layer that wrote the entry; entries of older searches read as
    /// unvisited, so that a reused layer needs no clearing.
    stamp: u16,
    pub cost_so_far: i32,
    pub estimated_total_cost: i32,
    pub previous_node: CPos,
}

impl Default for CellInfo {
    fn default() -> Self {
        CellInfo {
            status: CellStatus::Unvisited,
            stamp: 0,
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

/// Binary min-heap with the exact sift behavior of the C# `PriorityQueue`. The C# queue addresses an item by
/// level and index within the level; the flat position `2^level - 1 + index` of that item is used here, so the
/// children of position `i` are at `2i + 1` and `2i + 2`.
pub struct PriorityQueue {
    items: Vec<GraphConnection>,
}

impl PriorityQueue {
    pub fn new() -> Self {
        PriorityQueue { items: Vec::new() }
    }

    pub fn add(&mut self, item: GraphConnection) {
        let mut pos = self.items.len();
        self.items.push(item);
        while pos >= 1 {
            let parent = (pos - 1) / 2;
            let above = self.items[parent];
            if above.cost > item.cost {
                self.items[pos] = above;
                pos = parent;
            } else {
                break;
            }
        }
        self.items[pos] = item;
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn peek(&self) -> GraphConnection {
        *self.items.first().expect("PriorityQueue empty.")
    }

    /// Removes the minimum. As in C#, the last item sifts down from the root while it still counts as part of the
    /// heap and is dropped afterwards.
    pub fn pop(&mut self) -> GraphConnection {
        let ret = self.peek();
        let len = self.items.len();
        let val = self.items[len - 1];
        let mut pos = 0;
        loop {
            let mut down = 2 * pos + 1;
            if down >= len {
                break;
            }
            let mut down_item = self.items[down];
            if down + 1 < len {
                let down2 = self.items[down + 1];
                if down_item.cost >= down2.cost {
                    down += 1;
                    down_item = down2;
                }
            }
            if val.cost <= down_item.cost {
                break;
            }
            self.items[pos] = down_item;
            pos = down;
        }
        self.items[pos] = val;
        self.items.pop();
        ret
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
    static LAYER_POOL: RefCell<Vec<(Vec<CellInfo>, u16)>> = const { RefCell::new(Vec::new()) };
}

const MAX_POOL_SIZE: usize = 4;

/// A map layer and its search generation, which is never 0, so that fresh entries read as unvisited.
fn layer_from_pool(len: usize) -> (Vec<CellInfo>, u16) {
    let reused = LAYER_POOL.with(|p| p.borrow_mut().pop());
    match reused {
        Some((l, stamp)) if l.len() == len && stamp != u16::MAX => (l, stamp + 1),
        Some((mut l, _)) if l.len() == len => {
            l.iter_mut().for_each(|c| *c = CellInfo::default());
            (l, 1)
        }
        _ => (vec![CellInfo::default(); len], 1),
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

/// Common data of `DensePathGraph`. `map` and `owner_mask`, the player mask of the owner of `actor`, are
/// resolved once per search instead of once per neighbor cell.
pub struct DenseParams {
    pub map: Rc<Map>,
    pub locomotor: Rc<Locomotor>,
    pub actor: Option<ActorRef>,
    pub owner_mask: u64,
    pub check: BlockedByActor,
    pub custom_cost: Option<CustomCost>,
    pub ignore_actor: Option<ActorRef>,
    pub lane_bias: bool,
    pub in_reverse: bool,
}

impl DenseParams {
    #[allow(clippy::too_many_arguments)]
    fn new(
        world: &World,
        locomotor: &Rc<Locomotor>,
        actor: Option<&ActorRef>,
        check: BlockedByActor,
        custom_cost: Option<CustomCost>,
        ignore_actor: Option<&ActorRef>,
        lane_bias: bool,
        in_reverse: bool,
    ) -> Self {
        DenseParams {
            map: world.map().clone(),
            locomotor: locomotor.clone(),
            actor: actor.cloned(),
            owner_mask: actor.map_or(0, |a| a.with_owner(|o| o.player_mask)),
            check,
            custom_cost,
            ignore_actor: ignore_actor.cloned(),
            lane_bias,
            in_reverse,
        }
    }
}

/// The cell infos of a graph, as seen by one expansion step.
trait CellStore {
    fn get(&self, c: CPos) -> CellInfo;
    fn set(&mut self, c: CPos, v: CellInfo);
}

struct MapCells<'a> {
    infos: &'a mut [CellInfo],
    width: i32,
    stamp: u16,
}

impl CellStore for MapCells<'_> {
    #[inline(always)]
    fn get(&self, c: CPos) -> CellInfo {
        let info = self.infos[(c.y() * self.width + c.x()) as usize];
        if info.stamp == self.stamp { info } else { CellInfo::default() }
    }

    #[inline(always)]
    fn set(&mut self, c: CPos, v: CellInfo) {
        self.infos[(c.y() * self.width + c.x()) as usize] = CellInfo { stamp: self.stamp, ..v };
    }
}

struct GridCells<'a> {
    infos: &'a mut [CellInfo],
    grid: Grid,
}

impl CellStore for GridCells<'_> {
    #[inline(always)]
    fn get(&self, c: CPos) -> CellInfo {
        let g = &self.grid;
        self.infos[((c.y() - g.top_left.y()) * g.width() + (c.x() - g.top_left.x())) as usize]
    }

    #[inline(always)]
    fn set(&mut self, c: CPos, v: CellInfo) {
        let g = &self.grid;
        self.infos[((c.y() - g.top_left.y()) * g.width() + (c.x() - g.top_left.x())) as usize] = v;
    }
}

impl CellStore for HashMap<CPos, CellInfo> {
    fn get(&self, c: CPos) -> CellInfo {
        HashMap::get(self, &c).copied().unwrap_or_default()
    }

    fn set(&mut self, c: CPos, v: CellInfo) {
        self.insert(c, v);
    }
}

pub type EdgesFn = Rc<dyn Fn(CPos) -> Vec<GraphConnection>>;

pub enum Graph {
    Map {
        p: DenseParams,
        infos: Vec<CellInfo>,
        width: i32,
        stamp: u16,
    },
    Grid {
        p: DenseParams,
        infos: Vec<CellInfo>,
        grid: Grid,
    },
    Sparse {
        edges: EdgesFn,
        info: HashMap<CPos, CellInfo>,
    },
}

impl Graph {
    #[inline]
    pub fn get(&self, c: CPos) -> CellInfo {
        match self {
            Graph::Map { infos, width, stamp, .. } => {
                let info = infos[(c.y() * width + c.x()) as usize];
                if info.stamp == *stamp { info } else { CellInfo::default() }
            }
            Graph::Grid { infos, grid, .. } => infos[((c.y() - grid.top_left.y()) * grid.width() + (c.x() - grid.top_left.x())) as usize],
            Graph::Sparse { info, .. } => info.get(&c).copied().unwrap_or_default(),
        }
    }

    #[inline]
    pub fn set(&mut self, c: CPos, v: CellInfo) {
        match self {
            Graph::Map { infos, width, stamp, .. } => infos[(c.y() * *width + c.x()) as usize] = CellInfo { stamp: *stamp, ..v },
            Graph::Grid { infos, grid, .. } => {
                infos[((c.y() - grid.top_left.y()) * grid.width() + (c.x() - grid.top_left.x())) as usize] = v
            }
            Graph::Sparse { info, .. } => {
                info.insert(c, v);
            }
        }
    }
}

#[inline]
fn get_path_cost_to_node(p: &DenseParams, dest: CPos, direction: CVec, target_predicate: &dyn Fn(CPos) -> bool) -> i32 {
    let actor = p.actor.as_ref().map(|a| (a, p.owner_mask));
    let mut cost = p
        .locomotor
        .movement_cost_to_enter_cell_in(&p.map, actor, dest, p.check, p.ignore_actor.as_ref(), false, FULL_CELL);
    if cost == MOVEMENT_COST_FOR_UNREACHABLE_CELL && p.in_reverse && target_predicate(dest) {
        cost = 0;
    }
    if cost != MOVEMENT_COST_FOR_UNREACHABLE_CELL {
        return calculate_cell_path_cost(p, dest, direction, cost);
    }
    PATH_COST_FOR_INVALID_PATH
}

#[inline]
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
        if let Graph::Map { infos, stamp, .. } = &mut self.graph {
            let l = std::mem::take(infos);
            let stamp = *stamp;
            LAYER_POOL.with(|p| {
                let mut p = p.borrow_mut();
                if p.len() < MAX_POOL_SIZE {
                    p.push((l, stamp));
                }
            });
        }
    }
}

fn map_graph(map: &Map, p: DenseParams) -> Graph {
    let (w, h) = map.map_size;
    let (infos, stamp) = layer_from_pool((w * h) as usize);
    Graph::Map { p, infos, width: w, stamp }
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
            DenseParams::new(world, locomotor, self_, check, custom_cost.clone(), ignore_actor, lane_bias, false),
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
        let p = DenseParams::new(
            world,
            locomotor,
            self_,
            check,
            custom_cost.clone(),
            ignore_actor,
            lane_bias,
            in_reverse,
        );
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
            info: HashMap::default(),
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
                stamp: 0,
            },
        );
        self.open_queue.add(GraphConnection {
            destination: location,
            cost: estimated,
        });
    }

    pub fn can_expand(&mut self) -> bool {
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
        let PathSearch {
            graph,
            target_predicate,
            heuristic,
            heuristic_weight_percentage,
            open_queue,
            ..
        } = self;
        let mut step = Expansion {
            heuristic,
            weight: *heuristic_weight_percentage,
            open_queue,
        };
        match graph {
            Graph::Map { p, infos, width, stamp } => {
                let mut cells = MapCells {
                    infos,
                    width: *width,
                    stamp: *stamp,
                };
                step.expand_dense(&mut cells, p, None, &**target_predicate, current);
            }
            Graph::Grid { p, infos, grid } => {
                let grid = *grid;
                let mut cells = GridCells { infos, grid };
                step.expand_dense(&mut cells, p, Some(grid), &**target_predicate, current);
            }
            Graph::Sparse { edges, info } => {
                let current_info = step.close(info, current);
                for c in edges(current) {
                    step.relax(info, current, current_info, c.destination, c.cost);
                }
            }
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

/// The parts of a `PathSearch` that one expansion step uses besides the cell infos.
struct Expansion<'a> {
    heuristic: &'a mut Heuristic,
    weight: i32,
    open_queue: &'a mut PriorityQueue,
}

impl Expansion<'_> {
    /// Marks `current` as closed and returns its info.
    #[inline(always)]
    fn close<S: CellStore>(&mut self, cells: &mut S, current: CPos) -> CellInfo {
        let info = cells.get(current);
        cells.set(
            current,
            CellInfo {
                status: CellStatus::Closed,
                ..info
            },
        );
        info
    }

    /// `DensePathGraph.GetConnections` and the expansion over them in one pass. The C# collects the connections
    /// first; the costs do not depend on the cell infos and no step changes a closed status, so the order of the
    /// calls does not change the result.
    #[inline(always)]
    fn expand_dense<S: CellStore>(
        &mut self,
        cells: &mut S,
        p: &DenseParams,
        grid: Option<Grid>,
        target_predicate: &dyn Fn(CPos) -> bool,
        current: CPos,
    ) {
        let info = self.close(cells, current);
        let dx = current.x() - info.previous_node.x();
        let dy = current.y() - info.previous_node.y();
        for &dir in DIRECTED_NEIGHBORS[(dy * 3 + dx + 4) as usize] {
            let neighbor = current + dir;
            if grid.is_some_and(|g| !g.contains(neighbor)) {
                continue;
            }
            let path_cost = get_path_cost_to_node(p, neighbor, dir, target_predicate);
            if path_cost != PATH_COST_FOR_INVALID_PATH {
                self.relax(cells, current, info, neighbor, path_cost);
            }
        }
    }

    #[inline(always)]
    fn relax<S: CellStore>(&mut self, cells: &mut S, current: CPos, info: CellInfo, neighbor: CPos, cost: i32) {
        let cost_so_far = info.cost_so_far.wrapping_add(cost);
        let ni = cells.get(neighbor);
        if ni.status == CellStatus::Closed || (ni.status == CellStatus::Open && cost_so_far >= ni.cost_so_far) {
            return;
        }
        let remaining = if ni.status == CellStatus::Open {
            ni.estimated_total_cost.wrapping_sub(ni.cost_so_far)
        } else {
            let h = (self.heuristic)(neighbor, true);
            if h == PATH_COST_FOR_INVALID_PATH {
                return;
            }
            h.wrapping_mul(self.weight) / 100
        };
        let total = cost_so_far.wrapping_add(remaining);
        cells.set(
            neighbor,
            CellInfo {
                status: CellStatus::Open,
                cost_so_far,
                estimated_total_cost: total,
                previous_node: current,
                stamp: 0,
            },
        );
        self.open_queue.add(GraphConnection {
            destination: neighbor,
            cost: total,
        });
    }
}
