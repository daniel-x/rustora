// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Render/Hovers.cs` (its offset moves weapon muzzles) and `Traits/Huntable.cs`.

use std::cell::Cell;
use std::rc::Rc;

use rustora_core::world_types::{WAngle, WDist, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::*;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("Hovers", |n| load_info!(HoversInfo, n));
    r.register("Huntable", |n| load_info!(HuntableInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct HoversInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub bob_distance: WDist,
    pub min_hovering_altitude: WDist,
    pub ticks: i32,
    pub fall_ticks: i32,
    pub rise_ticks: i32,
    pub initial_height: WDist,
}

impl Default for HoversInfo {
    fn default() -> Self {
        HoversInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            bob_distance: WDist::new(-43),
            min_hovering_altitude: WDist::ZERO,
            ticks: 6,
            fall_ticks: 10,
            rise_ticks: 20,
            initial_height: WDist::new(43),
        }
    }
}

pub struct Hovers {
    info: Rc<HoversInfo>,
    state: ConditionalState,
    step_percentage: i32,
    fall_tick_height: i32,
    ticks: Cell<i32>,
    world_visual_offset: Cell<WVec>,
}

register_traits!(Hovers: ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(HoversInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    let height = (info.initial_height.length + info.bob_distance.length).max(info.fall_ticks);
    Some(Rc::new(Hovers {
        state,
        step_percentage: 256 / info.ticks,
        fall_tick_height: height / info.fall_ticks,
        ticks: Cell::new(0),
        world_visual_offset: Cell::new(WVec::ZERO),
        info,
    }))
});

impl ConditionalHooks for Hovers {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}
rustora_engine::conditional_observers_impl!(Hovers);

impl INotifyCreated for Hovers {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl ISync for Hovers {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.world_visual_offset.get().hash()
    }
}

impl Hovers {
    pub fn world_visual_offset(&self) -> WVec {
        self.world_visual_offset.get()
    }
}

impl ITick for Hovers {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            if self.world_visual_offset.get().z < 0 {
                return;
            }
            let fall_ticks = self.world_visual_offset.get().z / self.fall_tick_height - 1;
            self.world_visual_offset.set(WVec::new(0, 0, self.fall_tick_height * fall_ticks));
        } else {
            let info = &self.info;
            let w = self_.world();
            let visual = if w.map().distance_above_terrain(self_.center_position()) >= info.min_hovering_altitude {
                WAngle::new(self.ticks.get() % (info.ticks * 4) * self.step_percentage).sin()
            } else {
                0
            };
            let mut height = info.bob_distance.length * visual / 1024 + info.initial_height.length;
            let z = self.world_visual_offset.get().z;
            if z < height {
                height = (z + info.initial_height.length / info.rise_ticks).min(height);
            }
            self.world_visual_offset.set(WVec::new(0, 0, height));
            self.ticks.set(self.ticks.get() + 1);
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct HuntableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct Huntable;

register_traits!(Huntable);
info_impl!(HuntableInfo => |_info, _init| Some(Rc::new(Huntable)));
