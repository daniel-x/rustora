// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Buildings/Building.cs` (geometry and sync state; map influence pending).

use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use rustora_core::fieldloader::{LoadError, get_value};
use rustora_core::miniyaml::MiniYaml;
use rustora_core::world_types::{CPos, CVec, WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor_init::LocationInit;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("Building", |n| load_info!(BuildingInfo, n));
    r.register("Immobile", |n| load_info!(ImmobileInfo, n));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FootprintCellType {
    Empty,
    OccupiedPassable,
    Occupied,
    OccupiedUntargetable,
    OccupiedPassableTransitOnly,
}

impl FootprintCellType {
    fn from_char(c: char) -> Option<Self> {
        Some(match c {
            '_' => FootprintCellType::Empty,
            '=' => FootprintCellType::OccupiedPassable,
            'x' => FootprintCellType::Occupied,
            'X' => FootprintCellType::OccupiedUntargetable,
            '+' => FootprintCellType::OccupiedPassableTransitOnly,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct BuildingInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub terrain_types: IndexSet<String>,
    #[field(loader = "load_footprint")]
    pub footprint: IndexMap<CVec, FootprintCellType>,
    pub dimensions: CVec,
    pub local_center_offset: WVec,
    pub requires_base_provider: bool,
    pub allow_invalid_placement: bool,
    pub allow_placement_on_resources: bool,
    pub remove_smudges_on_build: bool,
    pub remove_smudges_on_sell: bool,
    pub remove_smudges_on_transform: bool,
    pub build_sounds: Vec<String>,
    pub undeploy_sounds: Vec<String>,
}

impl Default for BuildingInfo {
    fn default() -> Self {
        BuildingInfo {
            instance_name: None,
            terrain_types: IndexSet::new(),
            footprint: IndexMap::new(),
            dimensions: CVec::new(1, 1),
            local_center_offset: WVec::ZERO,
            requires_base_provider: false,
            allow_invalid_placement: false,
            allow_placement_on_resources: false,
            remove_smudges_on_build: true,
            remove_smudges_on_sell: true,
            remove_smudges_on_transform: true,
            build_sounds: Vec::new(),
            undeploy_sounds: Vec::new(),
        }
    }
}

fn load_footprint(yaml: &MiniYaml) -> Result<IndexMap<CVec, FootprintCellType>, LoadError> {
    let fp = yaml.node_with_key_or_default("Footprint")?;
    let chars: Vec<char> = match fp {
        Some(n) => n.value.value_str().chars().filter(|c| !c.is_whitespace()).collect(),
        None => vec!['x'],
    };
    let dim = match yaml.node_with_key_or_default("Dimensions")? {
        Some(n) => get_value::<CVec>("Dimensions", n.value.value.as_deref())?,
        None => CVec::new(1, 1),
    };
    if chars.len() as i32 != dim.x * dim.y {
        return Err(LoadError(format!(
            "Invalid footprint: {} does not match dimensions {}x{}",
            chars.iter().collect::<String>(),
            dim.x,
            dim.y
        )));
    }
    let mut out = IndexMap::new();
    let mut i = 0;
    for y in 0..dim.y {
        for x in 0..dim.x {
            let c = chars[i];
            i += 1;
            let t = FootprintCellType::from_char(c).ok_or_else(|| LoadError(format!("Invalid footprint cell type '{c}'")))?;
            out.insert(CVec::new(x, y), t);
        }
    }
    Ok(out)
}

impl BuildingInfo {
    pub fn footprint_tiles(&self, location: CPos, ty: FootprintCellType) -> impl Iterator<Item = CPos> + '_ {
        self.footprint
            .iter()
            .filter(move |(_, t)| **t == ty)
            .map(move |(k, _)| location + *k)
    }

    fn tiles_of(&self, location: CPos, types: &[FootprintCellType]) -> Vec<CPos> {
        types
            .iter()
            .flat_map(|t| self.footprint_tiles(location, *t).collect::<Vec<_>>())
            .collect()
    }

    pub fn tiles(&self, location: CPos) -> Vec<CPos> {
        use FootprintCellType::*;
        self.tiles_of(
            location,
            &[OccupiedPassable, Occupied, OccupiedUntargetable, OccupiedPassableTransitOnly],
        )
    }

    pub fn frozen_under_fog_tiles(&self, location: CPos) -> Vec<CPos> {
        let mut v: Vec<CPos> = self.footprint_tiles(location, FootprintCellType::Empty).collect();
        v.extend(self.tiles(location));
        v
    }

    pub fn occupied_tiles(&self, location: CPos) -> Vec<CPos> {
        use FootprintCellType::*;
        self.tiles_of(location, &[Occupied, OccupiedUntargetable, OccupiedPassableTransitOnly])
    }

    pub fn pathable_tiles(&self, location: CPos) -> Vec<CPos> {
        use FootprintCellType::*;
        self.tiles_of(location, &[Empty, OccupiedPassable])
    }

    pub fn transit_only_tiles(&self, location: CPos) -> Vec<CPos> {
        self.footprint_tiles(location, FootprintCellType::OccupiedPassableTransitOnly)
            .collect()
    }

    pub fn center_offset(&self, w: &World) -> WVec {
        let m = w.map();
        let off = (m.center_of_cell(CPos::new(self.dimensions.x, self.dimensions.y)) - m.center_of_cell(CPos::new(1, 1))) / 2;
        off - WVec::new(0, 0, off.z) + self.local_center_offset
    }
}

pub struct Building {
    pub info: Rc<BuildingInfo>,
    top_left: CPos,
    center: WPos,
    occupied: Vec<(CPos, u8)>,
    pub targetable: Vec<(CPos, u8)>,
    pub transit_only: Vec<CPos>,
}

register_traits!(Building: IOccupySpace, ISync);
info_impl!(BuildingInfo => |info, init| {
    let w = init.world();
    let top_left = init.inits.get::<LocationInit>().expect("Building needs a LocationInit").value;
    let occupied = info.occupied_tiles(top_left).into_iter().map(|c| (c, subcell::FULL_CELL)).collect();
    let targetable = info.footprint_tiles(top_left, FootprintCellType::Occupied).map(|c| (c, subcell::FULL_CELL)).collect();
    let transit_only = info.transit_only_tiles(top_left);
    let center = w.map().center_of_cell(top_left) + info.center_offset(&w);
    Some(Rc::new(Building { info, top_left, center, occupied, targetable, transit_only }))
});

impl ISync for Building {
    fn sync_hash(&self) -> i32 {
        self.top_left.hash()
    }
}

impl IOccupySpace for Building {
    fn center_position(&self) -> WPos {
        self.center
    }

    fn top_left(&self) -> CPos {
        self.top_left
    }

    fn occupied_cells(&self) -> Vec<(CPos, u8)> {
        self.occupied.clone()
    }
}

/// Port of `Traits/Immobile.cs` (map influence pending).
#[derive(Clone, Debug, FieldLoad)]
pub struct ImmobileInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub occupies_space: bool,
}

impl Default for ImmobileInfo {
    fn default() -> Self {
        ImmobileInfo {
            instance_name: None,
            occupies_space: true,
        }
    }
}

pub struct Immobile {
    top_left: CPos,
    center: WPos,
    occupied: Vec<(CPos, u8)>,
}

register_traits!(Immobile: IOccupySpace, ISync);
info_impl!(ImmobileInfo => |info, init| {
    let w = init.world();
    let top_left = init.inits.get::<LocationInit>().expect("Immobile needs a LocationInit").value;
    let occupied = if info.occupies_space { vec![(top_left, subcell::FULL_CELL)] } else { Vec::new() };
    Some(Rc::new(Immobile { top_left, center: w.map().center_of_cell(top_left), occupied }))
});

impl ISync for Immobile {
    fn sync_hash(&self) -> i32 {
        self.top_left.hash() ^ self.center.hash()
    }
}

impl IOccupySpace for Immobile {
    fn center_position(&self) -> WPos {
        self.center
    }

    fn top_left(&self) -> CPos {
        self.top_left
    }

    fn occupied_cells(&self) -> Vec<(CPos, u8)> {
        self.occupied.clone()
    }
}
