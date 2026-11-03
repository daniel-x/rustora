// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Player/Shroud.cs`. Flat maps only: a cell projects to itself.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::world_types::{CPos, MPos, PPos, WDist, WPos, WVec};
use rustora_derive::FieldLoad;

use crate::actor::ActorRef;
use crate::conditional::sync_bool;
use crate::map::Map;
use crate::player::{WinState, hash_player};
use crate::traits::*;
use crate::{info_impl, register_traits};

#[derive(Clone, Debug, FieldLoad)]
pub struct ShroudInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub fog_checkbox_enabled: bool,
    pub explored_map_checkbox_enabled: bool,
}

impl Default for ShroudInfo {
    fn default() -> Self {
        ShroudInfo {
            instance_name: None,
            fog_checkbox_enabled: true,
            explored_map_checkbox_enabled: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceType {
    PassiveVisibility,
    Shroud,
    Visibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CellType {
    Shroud,
    Fog,
    Visible,
}

pub const VISIBILITY_EXPLORED: u8 = 1;
pub const VISIBILITY_VISIBLE: u8 = 2;

#[derive(Default)]
struct Layers {
    passive_visible: Vec<i16>,
    visible: Vec<i16>,
    generated_shroud: Vec<i16>,
    explored: Vec<bool>,
    touched: Vec<bool>,
    resolved: Vec<u8>,
    sources: HashMap<usize, (SourceType, Rc<[PPos]>)>,
}

pub type ShroudChanged = Rc<dyn Fn(PPos)>;

pub struct Shroud {
    info: Rc<ShroudInfo>,
    map: Rc<Map>,
    width: i32,
    height: i32,
    l: RefCell<Layers>,
    listeners: RefCell<Vec<ShroudChanged>>,
    any_cell_touched: Cell<bool>,
    disabled_changed: Cell<bool>,
    disabled: Cell<bool>,
    fog_enabled: Cell<bool>,
    explore_map_enabled: Cell<bool>,
    shroud_generation_enabled: Cell<bool>,
    passive_visibility_enabled: Cell<bool>,
    pub revealed_cells: Cell<i32>,
    pub hash: Cell<i32>,
}

register_traits!(Shroud: ISync, INotifyCreated, ITick);
info_impl!(ShroudInfo => |info, init| {
    let map = init.world().map().clone();
    let (width, height) = map.map_size;
    let n = (width * height) as usize;
    Some(Rc::new(Shroud {
        info,
        map,
        width,
        height,
        l: RefCell::new(Layers {
            passive_visible: vec![0; n],
            visible: vec![0; n],
            generated_shroud: vec![0; n],
            explored: vec![false; n],
            touched: vec![false; n],
            resolved: vec![0; n],
            sources: HashMap::new(),
        }),
        listeners: RefCell::new(Vec::new()),
        any_cell_touched: Cell::new(true),
        disabled_changed: Cell::new(false),
        disabled: Cell::new(false),
        fog_enabled: Cell::new(false),
        explore_map_enabled: Cell::new(false),
        shroud_generation_enabled: Cell::new(false),
        passive_visibility_enabled: Cell::new(false),
        revealed_cells: Cell::new(0),
        hash: Cell::new(0),
    }))
});

impl ISync for Shroud {
    fn sync_hash(&self) -> i32 {
        sync_bool(self.disabled.get())
    }
}

impl INotifyCreated for Shroud {
    fn created(&self, self_: &ActorRef) {
        let w = self_.world();
        let gs = &w.lobby_info.global_settings;
        self.fog_enabled.set(gs.option_bool("fog", self.info.fog_checkbox_enabled));
        self.explore_map_enabled
            .set(gs.option_bool("explored", self.info.explored_map_checkbox_enabled));
        if self.explore_map_enabled.get() {
            let me = self_.clone();
            w.add_frame_end_task(Box::new(move |_| me.trait_::<Shroud>().explore_all()));
        }
        if !self.fog_enabled.get() && self.explore_map_enabled.get() {
            self.revealed_cells.set(self.map.bounds.width * self.map.bounds.height);
        }
    }
}

impl ITick for Shroud {
    fn tick(&self, self_: &ActorRef) {
        if !self.any_cell_touched.get() && !self.disabled_changed.get() {
            return;
        }
        self.any_cell_touched.set(false);
        if self.listeners.borrow().is_empty() {
            self.disabled_changed.set(false);
            return;
        }
        let n = (self.width * self.height) as usize;
        if self.disabled_changed.get() {
            self.l.borrow_mut().touched.iter_mut().for_each(|t| *t = false);
            for i in 0..n {
                self.update_cell(i, self_);
            }
        } else {
            for i in 0..n {
                let was = {
                    let mut l = self.l.borrow_mut();
                    std::mem::replace(&mut l.touched[i], false)
                };
                if was {
                    self.update_cell(i, self_);
                }
            }
        }
        self.hash
            .set(hash_player(Some(&self_.owner())).wrapping_add(self_.world().world_tick()));
        self.disabled_changed.set(false);
    }
}

impl Shroud {
    fn index(&self, p: PPos) -> usize {
        (p.v * self.width + p.u) as usize
    }

    fn layer_contains(&self, p: PPos) -> bool {
        p.u >= 0 && p.u < self.width && p.v >= 0 && p.v < self.height
    }

    fn resolved(&self, p: PPos) -> u8 {
        self.l.borrow().resolved[self.index(p)]
    }

    fn update_cell(&self, index: usize, self_: &ActorRef) {
        let (ty, old) = {
            let l = self.l.borrow();
            let mut ty = CellType::Shroud as u8;
            if l.explored[index] {
                let mut count = l.visible[index];
                if !self.shroud_generation_enabled.get() || count > 0 || l.generated_shroud[index] == 0 {
                    if self.passive_visibility_enabled.get() {
                        count += l.passive_visible[index];
                    }
                    ty = if count > 0 { CellType::Visible as u8 } else { CellType::Fog as u8 };
                }
            }
            (ty, l.resolved[index])
        };
        let disabled_changed = self.disabled_changed.get();
        if ty != old || disabled_changed {
            self.l.borrow_mut().resolved[index] = ty;
            let puv = PPos::new(index as i32 % self.width, index as i32 / self.width);
            if self.map.contains_ppos(puv) {
                let listeners = self.listeners.borrow().clone();
                for f in listeners {
                    f(puv);
                }
            }
            if !disabled_changed && (self.fog_enabled.get() || !self.explore_map_enabled.get()) {
                if ty == CellType::Visible as u8 {
                    self.revealed_cells.set(self.revealed_cells.get() + 1);
                } else if self.fog_enabled.get() && old == CellType::Visible as u8 {
                    self.revealed_cells.set(self.revealed_cells.get() - 1);
                }
            }
            if self_.owner().win_state.get() == WinState::Lost {
                self.revealed_cells.set(0);
            }
        }
    }

    pub fn on_shroud_changed(&self, f: ShroudChanged) {
        self.listeners.borrow_mut().push(f);
    }

    pub fn disabled(&self) -> bool {
        self.disabled.get()
    }

    pub fn set_disabled(&self, v: bool) {
        if self.disabled.get() == v {
            return;
        }
        self.disabled.set(v);
        self.disabled_changed.set(true);
    }

    pub fn fog_enabled(&self) -> bool {
        !self.disabled.get() && self.fog_enabled.get()
    }

    pub fn explore_map_enabled(&self) -> bool {
        self.explore_map_enabled.get()
    }

    /// `ProjectedCellsInRange` with a world position.
    pub fn projected_cells_in_range(map: &Map, pos: WPos, min_range: WDist, max_range: WDist, max_height_delta: i32) -> Vec<PPos> {
        let r = (max_range.length + 1023 + 512) / 1024;
        let min_limit = min_range.length_squared();
        let max_limit = max_range.length_squared();
        let projected_pos = pos - WVec::new(0, pos.z, pos.z);
        let projected_cell = map.cell_containing(projected_pos);
        let projected_height = pos.z / 512;
        let mut out = Vec::new();
        for c in map.find_tiles_in_annulus(projected_cell, min_range.length / 1024, r, true) {
            let dist = (map.center_of_cell(c) - projected_pos).horizontal_length_squared();
            if dist <= max_limit && (dist == 0 || dist > min_limit) {
                let uv = c.to_mpos(map.grid.grid_type);
                let puv = PPos::new(uv.u, uv.v);
                if max_height_delta < 0 || map.projected_height(puv) < projected_height + max_height_delta {
                    out.push(puv);
                }
            }
        }
        out
    }

    pub fn projected_cells_in_cell_range(map: &Map, cell: CPos, range: WDist) -> Vec<PPos> {
        Self::projected_cells_in_range(map, map.center_of_cell(cell), WDist::ZERO, range, -1)
    }

    /// `key` identifies the source (C# uses the object reference).
    pub fn add_source(&self, key: usize, ty: SourceType, projected_cells: Rc<[PPos]>) {
        let mut l = self.l.borrow_mut();
        assert!(
            l.sources.insert(key, (ty, projected_cells.clone())).is_none(),
            "Attempting to add duplicate shroud source"
        );
        for &puv in projected_cells.iter() {
            if !self.map.contains_ppos(puv) {
                continue;
            }
            let i = self.index(puv);
            l.touched[i] = true;
            self.any_cell_touched.set(true);
            match ty {
                SourceType::PassiveVisibility => {
                    self.passive_visibility_enabled.set(true);
                    l.passive_visible[i] += 1;
                    l.explored[i] = true;
                }
                SourceType::Visibility => {
                    l.visible[i] += 1;
                    l.explored[i] = true;
                }
                SourceType::Shroud => {
                    self.shroud_generation_enabled.set(true);
                    l.generated_shroud[i] += 1;
                }
            }
        }
    }

    pub fn remove_source(&self, key: usize) {
        let mut l = self.l.borrow_mut();
        let Some((ty, cells)) = l.sources.remove(&key) else { return };
        for &puv in cells.iter() {
            if self.map.contains_ppos(puv) {
                let i = self.index(puv);
                l.touched[i] = true;
                self.any_cell_touched.set(true);
                match ty {
                    SourceType::PassiveVisibility => l.passive_visible[i] -= 1,
                    SourceType::Visibility => l.visible[i] -= 1,
                    SourceType::Shroud => l.generated_shroud[i] -= 1,
                }
            }
        }
    }

    pub fn explore_projected_cells(&self, cells: &[PPos]) {
        let mut l = self.l.borrow_mut();
        for &puv in cells {
            if self.map.contains_ppos(puv) {
                let i = self.index(puv);
                if !l.explored[i] {
                    l.touched[i] = true;
                    self.any_cell_touched.set(true);
                    l.explored[i] = true;
                }
            }
        }
    }

    pub fn explore(&self, s: &Shroud) {
        let other = s.l.borrow().explored.clone();
        let mut l = self.l.borrow_mut();
        for puv in self.map.projected_cells() {
            let i = self.index(puv);
            if !l.explored[i] && other[i] {
                l.touched[i] = true;
                self.any_cell_touched.set(true);
                l.explored[i] = true;
            }
        }
    }

    pub fn explore_all(&self) {
        let mut l = self.l.borrow_mut();
        for puv in self.map.projected_cells() {
            let i = self.index(puv);
            if !l.explored[i] {
                l.touched[i] = true;
                self.any_cell_touched.set(true);
                l.explored[i] = true;
            }
        }
    }

    pub fn reset_exploration(&self) {
        let mut l = self.l.borrow_mut();
        for puv in self.map.projected_cells() {
            let i = self.index(puv);
            l.touched[i] = true;
            l.explored[i] = l.visible[i] + l.passive_visible[i] > 0;
        }
        self.any_cell_touched.set(true);
    }

    pub fn is_explored_wpos(&self, pos: WPos) -> bool {
        self.is_explored_ppos(self.map.projected_cell_covering(pos))
    }

    pub fn is_explored(&self, cell: CPos) -> bool {
        self.is_explored_mpos(cell.to_mpos(self.map.grid.grid_type))
    }

    pub fn is_explored_mpos(&self, uv: MPos) -> bool {
        if !self.map.contains_mpos(uv) {
            return false;
        }
        self.is_explored_ppos(PPos::new(uv.u, uv.v))
    }

    pub fn is_explored_ppos(&self, puv: PPos) -> bool {
        if self.disabled.get() {
            return self.map.contains_ppos(puv);
        }
        self.layer_contains(puv) && self.resolved(puv) > CellType::Shroud as u8
    }

    pub fn is_visible_wpos(&self, pos: WPos) -> bool {
        self.is_visible_ppos(self.map.projected_cell_covering(pos))
    }

    pub fn is_visible(&self, cell: CPos) -> bool {
        let uv = cell.to_mpos(self.map.grid.grid_type);
        self.map.tiles.contains(uv) && self.is_visible_ppos(PPos::new(uv.u, uv.v))
    }

    pub fn is_visible_ppos(&self, puv: PPos) -> bool {
        if !self.fog_enabled() {
            return self.map.contains_ppos(puv);
        }
        self.layer_contains(puv) && self.resolved(puv) == CellType::Visible as u8
    }

    pub fn contains(&self, uv: PPos) -> bool {
        self.layer_contains(uv)
    }

    pub fn any_explored(&self, cells: &[PPos]) -> bool {
        cells.iter().any(|&c| self.is_explored_ppos(c))
    }

    /// Bit flags [`VISIBILITY_EXPLORED`] and [`VISIBILITY_VISIBLE`].
    pub fn get_visibility(&self, puv: PPos) -> u8 {
        let mut state = 0;
        let contains = self.layer_contains(puv);
        if self.disabled.get() {
            if self.fog_enabled.get() {
                if contains {
                    state |= VISIBILITY_EXPLORED;
                    if self.resolved(puv) == CellType::Visible as u8 {
                        state |= VISIBILITY_VISIBLE;
                    }
                }
            } else if self.map.contains_ppos(puv) {
                state |= VISIBILITY_EXPLORED | VISIBILITY_VISIBLE;
            }
        } else if self.fog_enabled.get() {
            if contains {
                let rt = self.resolved(puv);
                if rt == CellType::Visible as u8 {
                    state |= VISIBILITY_EXPLORED | VISIBILITY_VISIBLE;
                } else if rt > CellType::Shroud as u8 {
                    state |= VISIBILITY_EXPLORED;
                }
            }
        } else if contains {
            state |= VISIBILITY_VISIBLE;
            if self.resolved(puv) > CellType::Shroud as u8 {
                state |= VISIBILITY_EXPLORED;
            }
        }
        state
    }
}
