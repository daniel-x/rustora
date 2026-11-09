// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Traits/World/TerrainLighting.cs` and `Traits/TerrainLightSource.cs`: a global light tint and local light
//! sources that the renderer multiplies into terrain and sprite colors. Nothing simulated depends on them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_core::world_types::{WDist, WPos};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::render::ITerrainLighting;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("TerrainLighting", |n| load_info!(TerrainLightingInfo, n));
    r.register("TerrainLightSource", |n| load_info!(TerrainLightSourceInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TerrainLightingInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub intensity: f32,
    pub height_step: f32,
    pub red_tint: f32,
    pub green_tint: f32,
    pub blue_tint: f32,
    pub bin_size: i32,
}

impl Default for TerrainLightingInfo {
    fn default() -> Self {
        TerrainLightingInfo {
            instance_name: None,
            intensity: 1.0,
            height_step: 0.0,
            red_tint: 1.0,
            green_tint: 1.0,
            blue_tint: 1.0,
            bin_size: 10,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
struct LightSource {
    pos: WPos,
    range: WDist,
    intensity: f32,
    tint: [f32; 3],
}

/// `TerrainLighting`. The light sources are kept in insertion order; C# bins them only to find them faster.
#[derive(rustora_engine::persist::Persist)]
pub struct TerrainLighting {
    info: Rc<TerrainLightingInfo>,
    world: std::rc::Weak<World>,
    sources: RefCell<IndexMap<i32, LightSource>>,
    next_token: Cell<i32>,
}

register_traits!(TerrainLighting: ITerrainLighting);
info_impl!(TerrainLightingInfo => |info, init| Some(Rc::new(TerrainLighting {
    info,
    world: Rc::downgrade(&init.world()),
    sources: RefCell::new(IndexMap::new()),
    next_token: Cell::new(1),
})));

impl TerrainLighting {
    /// `AddLightSource`: returns the token for [`TerrainLighting::remove_light_source`].
    pub fn add_light_source(&self, pos: WPos, range: WDist, intensity: f32, tint: [f32; 3]) -> i32 {
        let token = self.next_token.get();
        self.next_token.set(token + 1);
        self.sources.borrow_mut().insert(
            token,
            LightSource {
                pos,
                range,
                intensity,
                tint,
            },
        );
        token
    }

    /// `RemoveLightSource`.
    pub fn remove_light_source(&self, token: i32) {
        self.sources.borrow_mut().shift_remove(&token);
    }
}

impl ITerrainLighting for TerrainLighting {
    fn tint_at(&self, pos: WPos) -> [f32; 3] {
        let mut tint = [self.info.red_tint, self.info.green_tint, self.info.blue_tint];
        let Some(world) = self.world.upgrade() else {
            return tint;
        };
        let map = world.map();
        let cell = map.cell_containing(pos);
        if !map.height.contains_cell(cell) {
            return tint;
        }
        let mut intensity = self.info.intensity + self.info.height_step * *map.height.at(cell) as f32;
        for source in self.sources.borrow().values() {
            let range = source.range.length;
            let distance = (source.pos - pos).length();
            if distance > range {
                continue;
            }
            let falloff = (range - distance) as f32 / range as f32;
            intensity += falloff * source.intensity;
            for (t, s) in tint.iter_mut().zip(source.tint) {
                *t += falloff * s;
            }
        }
        tint.map(|t| intensity * t)
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TerrainLightSourceInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub range: WDist,
    pub intensity: f32,
    pub red_tint: f32,
    pub green_tint: f32,
    pub blue_tint: f32,
}

impl Default for TerrainLightSourceInfo {
    fn default() -> Self {
        TerrainLightSourceInfo {
            instance_name: None,
            range: WDist::from_cells(10),
            intensity: 0.0,
            red_tint: 0.0,
            green_tint: 0.0,
            blue_tint: 0.0,
        }
    }
}

/// `TerrainLightSource`: a light while the actor is in the world.
#[derive(rustora_engine::persist::Persist)]
pub struct TerrainLightSource {
    info: Rc<TerrainLightSourceInfo>,
    token: Cell<i32>,
}

register_traits!(TerrainLightSource: INotifyAddedToWorld, INotifyRemovedFromWorld);
info_impl!(TerrainLightSourceInfo => |info, _init| Some(Rc::new(TerrainLightSource { info, token: Cell::new(-1) })));

impl TerrainLightSource {
    fn lighting(self_: &ActorRef) -> Rc<TerrainLighting> {
        self_
            .world()
            .world_actor()
            .trait_or_default::<TerrainLighting>()
            .expect("TerrainLightSource can only be used with the world TerrainLighting trait.")
    }
}

impl INotifyAddedToWorld for TerrainLightSource {
    fn added_to_world(&self, self_: &ActorRef) {
        let i = &self.info;
        let token = Self::lighting(self_).add_light_source(
            self_.center_position(),
            i.range,
            i.intensity,
            [i.red_tint, i.green_tint, i.blue_tint],
        );
        self.token.set(token);
    }
}

impl INotifyRemovedFromWorld for TerrainLightSource {
    fn removed_from_world(&self, self_: &ActorRef) {
        Self::lighting(self_).remove_light_source(self.token.get());
    }
}
