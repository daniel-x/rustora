// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `Traits/World/SmudgeLayer.cs`. Smudges are cosmetic: they draw from the local random only.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::world_types::{CPos, WDist, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::effect::EffectRef;
use rustora_engine::player::PlayerRef;
use rustora_engine::render::PaletteRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::effects::SpriteEffect;

pub fn register(r: &mut TraitRegistry) {
    r.register("SmudgeLayer", |n| load_info!(SmudgeLayerInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct SmudgeLayerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(name = "Type")]
    pub smudge_type: String,
    pub sequence: String,
    pub smoke_chance: i32,
    pub max_smoke_offset_distance: WDist,
    pub smoke_image: Option<String>,
    pub smoke_sequences: Vec<String>,
    pub smoke_palette: String,
    pub palette: String,
}

impl Default for SmudgeLayerInfo {
    fn default() -> Self {
        SmudgeLayerInfo {
            instance_name: None,
            smudge_type: "Scorch".into(),
            sequence: "scorch".into(),
            smoke_chance: 0,
            max_smoke_offset_distance: WDist::ZERO,
            smoke_image: None,
            smoke_sequences: Vec::new(),
            smoke_palette: "effect".into(),
            palette: "terrain".into(),
        }
    }
}

/// One drawn smudge: frame `depth` of sequence `kind` of the image `SmudgeLayerInfo.sequence`.
#[derive(Clone, Debug, PartialEq, Eq, rustora_engine::persist::Persist)]
#[persist(value)]
pub struct Smudge {
    pub kind: String,
    pub depth: i32,
}

#[derive(rustora_engine::persist::Persist)]
pub struct SmudgeLayer {
    pub info: Rc<SmudgeLayerInfo>,
    actor: ActorRef,
    kinds: Vec<String>,
    tiles: RefCell<HashMap<CPos, Smudge>>,
    /// Pending changes, applied when the render player sees the cell. `None` removes the smudge.
    dirty: RefCell<HashMap<CPos, Option<Smudge>>>,
}

register_traits!(SmudgeLayer);
info_impl!(SmudgeLayerInfo => |info, init| {
    let kinds = init.world().map().sequences().sequence_names(&info.sequence);
    Some(Rc::new(SmudgeLayer {
        info,
        actor: init.self_.clone(),
        kinds,
        tiles: RefCell::default(),
        dirty: RefCell::default(),
    }))
});

impl SmudgeLayer {
    /// `AddSmudge`.
    pub fn add_smudge(&self, loc: CPos) {
        let w = self.actor.world();
        let map = w.map();
        if !map.contains(loc) || self.kinds.is_empty() {
            return;
        }
        let info = &self.info;
        let mut random = w.local_random.borrow_mut();
        if let Some(image) = info
            .smoke_image
            .as_deref()
            .filter(|i| !i.is_empty() && !info.smoke_sequences.is_empty())
            && random.next_range(0, 100) <= info.smoke_chance
        {
            let mut position = map.center_of_cell(loc);
            let max = info.max_smoke_offset_distance.length;
            if max != 0 {
                let x = random.next_range(-max, max);
                let y = random.next_range(-max, max);
                position = position + WVec::new(x, y, 0);
                position = position - WVec::new(0, 0, map.distance_above_terrain(position).length);
            }
            let sequence = &info.smoke_sequences[random.next_max(info.smoke_sequences.len() as i32) as usize];
            let e: EffectRef = SpriteEffect::at(
                &w,
                position,
                rustora_core::world_types::WAngle::ZERO,
                image,
                sequence,
                PaletteRef::named(&info.smoke_palette),
            );
            w.add_frame_end_task(Box::new(move |w| w.add_effect(e)));
        }
        let mut dirty = self.dirty.borrow_mut();
        let pending = dirty.get(&loc).cloned().flatten();
        let existing = pending.or_else(|| self.tiles.borrow().get(&loc).cloned());
        let next = match existing {
            None => Smudge {
                kind: self.kinds[random.next_max(self.kinds.len() as i32) as usize].clone(),
                depth: 0,
            },
            Some(mut tile) => {
                let max_depth = map.sequences().get_sequence(&info.sequence, &tile.kind).length();
                if tile.depth < max_depth - 1 {
                    tile.depth += 1;
                }
                tile
            }
        };
        dirty.insert(loc, Some(next));
    }

    /// `RemoveSmudge`.
    pub fn remove_smudge(&self, cell: CPos) {
        if self.actor.world().map().contains(cell) {
            self.dirty.borrow_mut().insert(cell, None);
        }
    }

    /// `ITickRender.TickRender`: applies the pending changes that `viewer` can see.
    pub fn tick_render(&self, viewer: Option<&PlayerRef>) {
        let mut tiles = self.tiles.borrow_mut();
        self.dirty.borrow_mut().retain(|cell, change| {
            if viewer.is_some_and(|v| !v.shroud().is_visible(*cell)) {
                return true;
            }
            match change.take() {
                Some(s) => {
                    tiles.insert(*cell, s);
                }
                None => {
                    tiles.remove(cell);
                }
            }
            false
        });
    }

    pub fn at(&self, cell: CPos) -> Option<Smudge> {
        self.tiles.borrow().get(&cell).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.borrow().is_empty()
    }
}
