// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Runtime map: grid geometry, tiles, resources and cell helpers (`Map/Map.cs`).
//! Only flat rectangular grids are supported so far (all RA and TD maps).

use rustora_core::fieldloader::Rectangle;
use rustora_core::world_types::{CPos, MPos, MapGridType, WDist, WPos, WVec};
use rustora_game::map::MapPackage;

use crate::traits::subcell;

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
}

fn read_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn read_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

impl Map {
    /// Loads geometry and `map.bin` of a map package.
    pub fn load(package: MapPackage, grid: MapGrid) -> Result<Map, String> {
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
        Ok(Map {
            title: field("Title").unwrap_or_default(),
            uid: package.uid.clone(),
            tileset: field("Tileset").unwrap_or_default(),
            map_size: (w, h),
            bounds: Rectangle::new(b[0], b[1], b[2], b[3]),
            tiles,
            resources,
            height,
            grid,
            package,
        })
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
        let x = cell.x().clamp(self.bounds.left(), self.bounds.right() - 1);
        let y = cell.y().clamp(self.bounds.top(), self.bounds.bottom() - 1);
        CPos::new(x, y)
    }
}
