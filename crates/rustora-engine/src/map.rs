// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Runtime map: grid geometry, tiles, resources and cell helpers (`Map/Map.cs`).
//! Only flat rectangular grids are supported so far (all RA and TD maps).

use std::cell::RefCell;
use std::rc::Rc;

use rustora_core::fieldloader::Rectangle;
use rustora_core::world_types::{CPos, CVec, MPos, MapGridType, MersenneTwister, PPos, WAngle, WDist, WPos, WRot, WVec, isqrt_i32};
use rustora_game::map::MapPackage;

use crate::terrain::{TerrainInfo, TerrainTypeInfo};
use crate::traits::subcell;

const INVALID_TERRAIN_INDEX: i16 = -1;
pub const MAXIMUM_TILE_SEARCH_RANGE: i32 = 50;

/// `MapGrid` mod data.
#[derive(Clone, Debug)]
pub struct MapGrid {
    pub grid_type: MapGridType,
    pub tile_size: (i32, i32),
    pub maximum_terrain_height: u8,
    pub default_subcell: u8,
    pub subcell_offsets: Vec<WVec>,
}

impl Default for MapGrid {
    fn default() -> Self {
        MapGrid {
            grid_type: MapGridType::Rectangular,
            tile_size: (24, 24),
            maximum_terrain_height: 0,
            default_subcell: 3,
            subcell_offsets: vec![
                WVec::new(0, 0, 0),
                WVec::new(-299, -256, 0),
                WVec::new(256, -256, 0),
                WVec::new(0, 0, 0),
                WVec::new(-299, 256, 0),
                WVec::new(256, 256, 0),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainTile {
    pub ty: u16,
    pub index: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceTile {
    pub ty: u8,
    pub index: u8,
}

/// A rectangular layer of per-cell values indexed by map coordinates.
#[derive(Clone, Debug)]
pub struct CellLayer<T> {
    pub width: i32,
    pub height: i32,
    data: Vec<T>,
}

impl<T: Clone> CellLayer<T> {
    pub fn new(width: i32, height: i32, value: T) -> Self {
        CellLayer {
            width,
            height,
            data: vec![value; (width * height) as usize],
        }
    }

    pub fn contains(&self, uv: MPos) -> bool {
        uv.u >= 0 && uv.u < self.width && uv.v >= 0 && uv.v < self.height
    }

    pub fn contains_cell(&self, c: CPos) -> bool {
        self.contains(MPos::new(c.x(), c.y()))
    }

    pub fn at(&self, c: CPos) -> &T {
        self.get(MPos::new(c.x(), c.y()))
    }

    pub fn at_mut(&mut self, c: CPos) -> &mut T {
        let i = (c.y() * self.width + c.x()) as usize;
        &mut self.data[i]
    }

    pub fn set_at(&mut self, c: CPos, v: T) {
        self.set(MPos::new(c.x(), c.y()), v);
    }

    pub fn clear_to(&mut self, v: T) {
        self.data.iter_mut().for_each(|d| *d = v.clone());
    }

    pub fn get(&self, uv: MPos) -> &T {
        &self.data[(uv.v * self.width + uv.u) as usize]
    }

    pub fn set(&mut self, uv: MPos, v: T) {
        let i = (uv.v * self.width + uv.u) as usize;
        self.data[i] = v;
    }
}

pub struct Map {
    pub title: String,
    pub uid: String,
    pub tileset: String,
    pub grid: MapGrid,
    pub map_size: (i32, i32),
    pub bounds: Rectangle,
    pub tiles: CellLayer<TerrainTile>,
    pub resources: CellLayer<ResourceTile>,
    pub height: CellLayer<u8>,
    pub package: MapPackage,
    pub terrain: Rc<TerrainInfo>,
    custom_terrain: RefCell<CellLayer<u8>>,
    cached_terrain_index: RefCell<CellLayer<i16>>,
    custom_terrain_listeners: RefCell<Vec<Rc<dyn Fn(CPos)>>>,
    tiles_by_distance: Vec<Vec<CVec>>,
    all_edge_cells: Vec<CPos>,
    sequences: std::cell::OnceCell<Rc<crate::sequence::SequenceSet>>,
    read_file: std::cell::OnceCell<crate::sequence::ReadFile>,
}

fn isqrt_ceil(n: i32) -> i32 {
    let r = isqrt_i32(n);
    if r * r < n { r + 1 } else { r }
}

fn create_tiles_by_distance() -> Vec<Vec<CVec>> {
    let m = MAXIMUM_TILE_SEARCH_RANGE;
    let mut ts = vec![Vec::new(); m as usize + 1];
    for j in -m..=m {
        for i in -m..=m {
            if m * m >= i * i + j * j {
                ts[isqrt_ceil(i * i + j * j) as usize].push(CVec::new(i, j));
            }
        }
    }
    for list in &mut ts {
        list.sort_by_key(|a: &CVec| (a.length_squared(), a.hash(), a.x, a.y));
    }
    ts
}

fn read_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn read_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

impl Map {
    /// Loads geometry and `map.bin` of a map package.
    pub fn load(package: MapPackage, grid: MapGrid, terrain: Rc<TerrainInfo>) -> Result<Map, String> {
        let field = |k: &str| {
            package
                .yaml
                .nodes
                .iter()
                .find(|n| n.key == k)
                .map(|n| n.value.value_str().to_string())
        };
        let size: Vec<i32> = field("MapSize")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let b: Vec<i32> = field("Bounds")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if size.len() != 2 || b.len() != 4 {
            return Err("invalid MapSize or Bounds".into());
        }
        let (w, h) = (size[0], size[1]);
        let mut tiles = CellLayer::new(w, h, TerrainTile::default());
        let mut resources = CellLayer::new(w, h, ResourceTile::default());
        let mut height = CellLayer::new(w, h, 0u8);
        let bin = package.package.get("map.bin").ok_or("map.bin missing")?;
        let (tiles_offset, heights_offset, resources_offset) = match bin[0] {
            1 => (5usize, 0usize, (3 * w * h + 5) as usize),
            2 => (read_u32(&bin, 5) as usize, read_u32(&bin, 9) as usize, read_u32(&bin, 13) as usize),
            f => return Err(format!("Unknown binary map format '{f}'")),
        };
        if tiles_offset > 0 {
            let mut o = tiles_offset;
            for i in 0..w {
                for j in 0..h {
                    let ty = read_u16(&bin, o);
                    let mut index = bin[o + 2];
                    o += 3;
                    if index == u8::MAX {
                        index = ((i % 4) + (j % 4) * 4) as u8;
                    }
                    tiles.set(MPos::new(i, j), TerrainTile { ty, index });
                }
            }
        }
        if resources_offset > 0 {
            let mut o = resources_offset;
            for i in 0..w {
                for j in 0..h {
                    resources.set(
                        MPos::new(i, j),
                        ResourceTile {
                            ty: bin[o],
                            index: bin[o + 1],
                        },
                    );
                    o += 2;
                }
            }
        }
        if heights_offset > 0 {
            let mut o = heights_offset;
            for i in 0..w {
                for j in 0..h {
                    height.set(MPos::new(i, j), bin[o].min(grid.maximum_terrain_height));
                    o += 1;
                }
            }
        }
        for i in 0..w {
            for j in 0..h {
                let uv = MPos::new(i, j);
                if terrain.tile_info(*tiles.get(uv)).is_none() {
                    tiles.set(uv, terrain.default_tile());
                }
            }
        }
        let bounds = Rectangle::new(b[0], b[1], b[2], b[3]);
        Ok(Map {
            custom_terrain: RefCell::new(CellLayer::new(w, h, u8::MAX)),
            cached_terrain_index: RefCell::new(CellLayer::new(w, h, INVALID_TERRAIN_INDEX)),
            custom_terrain_listeners: RefCell::new(Vec::new()),
            tiles_by_distance: create_tiles_by_distance(),
            all_edge_cells: edge_cells(&bounds),
            sequences: std::cell::OnceCell::new(),
            read_file: std::cell::OnceCell::new(),
            terrain,
            title: field("Title").unwrap_or_default(),
            uid: package.uid.clone(),
            tileset: field("Tileset").unwrap_or_default(),
            map_size: (w, h),
            bounds,
            tiles,
            resources,
            height,
            grid,
            package,
        })
    }

    pub fn set_sequences(&self, s: Rc<crate::sequence::SequenceSet>) {
        let _ = self.sequences.set(s);
    }

    pub fn set_read_file(&self, f: crate::sequence::ReadFile) {
        let _ = self.read_file.set(f);
    }

    /// `Map.Open`: a file of the map package, else of the mod file system.
    pub fn open(&self, name: &str) -> Option<Vec<u8>> {
        self.read_file.get().and_then(|f| f(name))
    }

    /// `Map.Sequences`.
    pub fn sequences(&self) -> &Rc<crate::sequence::SequenceSet> {
        self.sequences.get().expect("sequences loaded")
    }

    pub fn contains_ppos(&self, p: PPos) -> bool {
        self.bounds.contains(p.u, p.v)
    }

    pub fn contains(&self, cell: CPos) -> bool {
        self.bounds.contains(cell.x(), cell.y())
    }

    pub fn contains_mpos(&self, uv: MPos) -> bool {
        self.tiles.contains(uv) && self.bounds.contains(uv.u, uv.v)
    }

    pub fn center_of_cell(&self, cell: CPos) -> WPos {
        WPos::new(1024 * cell.x() + 512, 1024 * cell.y() + 512, 0)
    }

    pub fn center_of_subcell(&self, cell: CPos, sub: u8) -> WPos {
        let i = sub as usize;
        if i < self.grid.subcell_offsets.len() {
            return self.center_of_cell(cell) + self.grid.subcell_offsets[i];
        }
        self.center_of_cell(cell)
    }

    pub fn distance_above_terrain(&self, pos: WPos) -> WDist {
        WDist::new(pos.z)
    }

    pub fn cell_containing(&self, pos: WPos) -> CPos {
        CPos::new(pos.x / 1024, pos.y / 1024)
    }

    pub fn offset(&self, delta: rustora_core::world_types::CVec, _dz: i32) -> WVec {
        WVec::new(1024 * delta.x, 1024 * delta.y, 0)
    }

    pub fn default_subcell(&self) -> u8 {
        if self.grid.default_subcell == subcell::INVALID {
            (self.grid.subcell_offsets.len() / 2) as u8
        } else {
            self.grid.default_subcell
        }
    }

    pub fn clamp(&self, cell: CPos) -> CPos {
        let x = cell.x().max(self.bounds.left()).min(self.bounds.right() - 1);
        let y = cell.y().max(self.bounds.top()).min(self.bounds.bottom() - 1);
        CPos::new(x, y)
    }

    pub fn offset_of_subcell(&self, sub: u8) -> WVec {
        if sub == subcell::INVALID || sub == subcell::ANY {
            return WVec::ZERO;
        }
        self.grid.subcell_offsets[sub as usize]
    }

    pub fn terrain_orientation(&self, _cell: CPos) -> WRot {
        WRot::NONE
    }

    pub fn projected_cell_covering(&self, pos: WPos) -> PPos {
        let c = self.cell_containing(pos - WVec::new(0, pos.z, pos.z));
        PPos::new(c.x(), c.y())
    }

    pub fn projected_height(&self, _p: PPos) -> i32 {
        0
    }

    pub fn projected_cells(&self) -> impl Iterator<Item = PPos> + '_ {
        let b = self.bounds;
        (b.top()..b.bottom()).flat_map(move |v| (b.left()..b.right()).map(move |u| PPos::new(u, v)))
    }

    /// `AllCells`: every cell of the map, rows first.
    pub fn all_cells(&self) -> impl Iterator<Item = CPos> + '_ {
        let (w, h) = self.map_size;
        (0..h).flat_map(move |v| (0..w).map(move |u| CPos::new(u, v)))
    }

    pub fn facing_between(&self, cell: CPos, towards: CPos, fallback: WAngle) -> WAngle {
        let delta = self.center_of_cell(towards) - self.center_of_cell(cell);
        if delta.horizontal_length_squared() == 0 {
            return fallback;
        }
        delta.yaw()
    }

    /// Cells at distance `min_range..=max_range`, nearest first.
    pub fn find_tiles_in_annulus(
        &self,
        center: CPos,
        min_range: i32,
        max_range: i32,
        allow_outside_bounds: bool,
    ) -> impl Iterator<Item = CPos> + '_ {
        assert!(max_range >= min_range, "Maximum range is less than the minimum range.");
        assert!(
            (max_range as usize) < self.tiles_by_distance.len(),
            "The requested range ({max_range}) cannot exceed the value of MaximumTileSearchRange"
        );
        (min_range..=max_range).flat_map(move |i| {
            self.tiles_by_distance[i as usize].iter().filter_map(move |offset| {
                let t = center + *offset;
                let ok = if allow_outside_bounds {
                    self.tiles.contains_cell(t)
                } else {
                    self.contains(t)
                };
                ok.then_some(t)
            })
        })
    }

    pub fn find_tiles_in_circle(&self, center: CPos, max_range: i32, allow_outside_bounds: bool) -> impl Iterator<Item = CPos> + '_ {
        self.find_tiles_in_annulus(center, 0, max_range, allow_outside_bounds)
    }

    /// `GetTerrainIndex`.
    pub fn terrain_index(&self, cell: CPos) -> u8 {
        let mut cache = self.cached_terrain_index.borrow_mut();
        let i = *cache.at(cell);
        if i != INVALID_TERRAIN_INDEX {
            return i as u8;
        }
        let custom = *self.custom_terrain.borrow().at(cell);
        let idx = if custom != u8::MAX {
            custom
        } else {
            self.terrain.terrain_index_of_tile(*self.tiles.at(cell))
        };
        cache.set_at(cell, idx as i16);
        idx
    }

    pub fn terrain_type_info(&self, cell: CPos) -> &TerrainTypeInfo {
        &self.terrain.terrain_types[self.terrain_index(cell) as usize]
    }

    pub fn custom_terrain_at(&self, cell: CPos) -> u8 {
        *self.custom_terrain.borrow().at(cell)
    }

    pub fn set_custom_terrain(&self, cell: CPos, v: u8) {
        self.custom_terrain.borrow_mut().set_at(cell, v);
        self.cached_terrain_index.borrow_mut().set_at(cell, INVALID_TERRAIN_INDEX);
        let listeners = self.custom_terrain_listeners.borrow().clone();
        for f in listeners {
            f(cell);
        }
    }

    pub fn on_custom_terrain_changed(&self, f: Rc<dyn Fn(CPos)>) {
        self.custom_terrain_listeners.borrow_mut().push(f);
    }

    pub fn choose_random_cell(&self, rand: &mut MersenneTwister) -> CPos {
        let u = rand.next_range(self.bounds.left(), self.bounds.right());
        let v = rand.next_range(self.bounds.top(), self.bounds.bottom());
        // C# additionally draws from the one-element unprojection list.
        let _ = rand.next_max(1);
        CPos::new(u, v)
    }

    pub fn choose_closest_edge_cell(&self, cell: CPos) -> CPos {
        let b = &self.bounds;
        let (u, v) = (cell.x(), cell.y());
        let horizontal = if u - b.left() < b.width / 2 { b.left() } else { b.right() };
        let vertical = if v - b.top() < b.height / 2 { b.top() } else { b.bottom() };
        let du = (horizontal - u).abs();
        let dv = (vertical - v).abs();
        if du < dv {
            CPos::new(horizontal, v)
        } else {
            CPos::new(u, vertical)
        }
    }

    pub fn all_edge_cells(&self) -> &[CPos] {
        &self.all_edge_cells
    }

    pub fn choose_random_edge_cell(&self, rand: &mut MersenneTwister) -> CPos {
        self.all_edge_cells[rand.next_max(self.all_edge_cells.len() as i32) as usize]
    }

    /// `Map.ChooseClosestMatchingEdgeCell`: like C# `FirstOrDefault` on a struct, it returns cell (0,0)
    /// when no edge cell matches.
    pub fn choose_closest_matching_edge_cell(&self, cell: CPos, m: impl Fn(CPos) -> bool) -> CPos {
        let mut cells: Vec<CPos> = self.all_edge_cells.clone();
        cells.sort_by_key(|c| (cell - *c).length());
        cells.into_iter().find(|c| m(*c)).unwrap_or_default()
    }

    pub fn projected_top_left(&self) -> WPos {
        WPos::new(self.bounds.left() * 1024, self.bounds.top() * 1024, 0)
    }

    pub fn projected_bottom_right(&self) -> WPos {
        WPos::new((self.bounds.right() - 1) * 1024 - 1, self.bounds.bottom() * 1024 - 1, 0)
    }

    pub fn distance_to_edge(&self, pos: WPos, dir: WVec) -> WDist {
        let (tl, br) = (self.projected_top_left(), self.projected_bottom_right());
        let (tl_x, tl_y, br_x, br_y) = (tl.x, tl.y, br.x, br.y);
        let p = pos - WVec::new(0, pos.z, pos.z);
        let x = if dir.x == 0 {
            i32::MAX
        } else {
            ((if dir.x < 0 { tl_x } else { br_x }) - p.x) / dir.x
        };
        let y = if dir.y == 0 {
            i32::MAX
        } else {
            ((if dir.y < 0 { tl_y } else { br_y }) - p.y) / dir.y
        };
        WDist::new(x.min(y).wrapping_mul(dir.length()))
    }
}

fn edge_cells(b: &Rectangle) -> Vec<CPos> {
    let mut out = Vec::new();
    let bottom = b.bottom() - 1;
    for u in b.left()..b.right() {
        out.push(CPos::new(u, b.top()));
        out.push(CPos::new(u, bottom));
    }
    for v in b.top()..b.bottom() {
        out.push(CPos::new(b.left(), v));
        out.push(CPos::new(b.right() - 1, v));
    }
    out
}
