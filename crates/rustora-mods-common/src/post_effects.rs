// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The state of the post-processing world traits: `FlashPostProcessEffect`, `ChronoshiftPostProcessEffect`,
//! `TintPostProcessEffect` and `MenuPostProcessEffect`. The client turns them into render passes. None of them
//! is part of the sync hash.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use rustora_core::fieldloader::Color;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("FlashPostProcessEffect", |n| load_info!(FlashPostProcessEffectInfo, n));
    r.register("ChronoshiftPostProcessEffect", |n| load_info!(ChronoshiftPostProcessEffectInfo, n));
    r.register("TintPostProcessEffect", |n| load_info!(TintPostProcessEffectInfo, n));
    r.register("MenuPostProcessEffect", |n| load_info!(MenuPostProcessEffectInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct FlashPostProcessEffectInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub length: i32,
    pub color: Color,
    #[field(name = "Type")]
    pub ty: Option<String>,
}

impl Default for FlashPostProcessEffectInfo {
    fn default() -> Self {
        FlashPostProcessEffectInfo {
            instance_name: None,
            length: 20,
            color: Color::from_argb(255, 255, 255, 255),
            ty: None,
        }
    }
}

pub struct FlashPostProcessEffect {
    pub info: Rc<FlashPostProcessEffectInfo>,
    remaining_frames: Cell<i32>,
    blend: Cell<f32>,
}

register_traits!(FlashPostProcessEffect: ITick);
info_impl!(FlashPostProcessEffectInfo => |info, _init| Some(Rc::new(FlashPostProcessEffect {
    info,
    remaining_frames: Cell::new(0),
    blend: Cell::new(0.0),
})));

impl FlashPostProcessEffect {
    /// `Enable`: -1 stands for `Length`.
    pub fn enable(&self, ticks: i32) {
        self.remaining_frames.set(if ticks == -1 { self.info.length } else { ticks });
    }

    pub fn enabled(&self) -> bool {
        self.remaining_frames.get() > 0
    }

    pub fn blend(&self) -> f32 {
        self.blend.get()
    }
}

impl ITick for FlashPostProcessEffect {
    fn tick(&self, _self_: &ActorRef) {
        let remaining = self.remaining_frames.get();
        if remaining > 0 {
            self.remaining_frames.set(remaining - 1);
            self.blend.set(((remaining - 1) as f32 / self.info.length as f32).min(1.0));
        }
    }
}

/// Enables the flash effects of `ty`, as `Lighting.Flash` and `FlashEffectWarhead` do.
pub fn flash(world: &World, ty: Option<&str>, ticks: i32) {
    for effect in world.world_actor().traits_implementing::<FlashPostProcessEffect>() {
        if effect.info.ty.as_deref() == ty {
            effect.enable(ticks);
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ChronoshiftPostProcessEffectInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub chrono_effect_length: i32,
}

impl Default for ChronoshiftPostProcessEffectInfo {
    fn default() -> Self {
        ChronoshiftPostProcessEffectInfo {
            instance_name: None,
            chrono_effect_length: 60,
        }
    }
}

pub struct ChronoshiftPostProcessEffect {
    info: Rc<ChronoshiftPostProcessEffectInfo>,
    remaining_frames: Cell<i32>,
}

register_traits!(ChronoshiftPostProcessEffect: ITick);
info_impl!(ChronoshiftPostProcessEffectInfo => |info, _init| Some(Rc::new(ChronoshiftPostProcessEffect {
    info,
    remaining_frames: Cell::new(0),
})));

impl ChronoshiftPostProcessEffect {
    pub fn enable(&self) {
        self.remaining_frames.set(self.info.chrono_effect_length);
    }

    /// The blend of an enabled effect.
    pub fn blend(&self) -> Option<f32> {
        let remaining = self.remaining_frames.get();
        (remaining > 0).then(|| remaining as f32 / self.info.chrono_effect_length as f32)
    }
}

impl ITick for ChronoshiftPostProcessEffect {
    fn tick(&self, _self_: &ActorRef) {
        let remaining = self.remaining_frames.get();
        if remaining > 0 {
            self.remaining_frames.set(remaining - 1);
        }
    }
}

/// Enables the chronoshift effects of all actors, as `Teleport` and `ConyardChronoReturn` do.
pub fn chronoshift_flash(world: &World) {
    for (_, effect) in world.actors_with_trait::<ChronoshiftPostProcessEffect>() {
        effect.enable();
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TintPostProcessEffectInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub ambient: f32,
}

impl Default for TintPostProcessEffectInfo {
    fn default() -> Self {
        TintPostProcessEffectInfo {
            instance_name: None,
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            ambient: 1.0,
        }
    }
}

/// `TintPostProcessEffect`, always enabled.
pub struct TintPostProcessEffect {
    pub red: Cell<f32>,
    pub green: Cell<f32>,
    pub blue: Cell<f32>,
    pub ambient: Cell<f32>,
}

register_traits!(TintPostProcessEffect);
info_impl!(TintPostProcessEffectInfo => |info, _init| Some(Rc::new(TintPostProcessEffect {
    red: Cell::new(info.red),
    green: Cell::new(info.green),
    blue: Cell::new(info.blue),
    ambient: Cell::new(info.ambient),
})));

impl TintPostProcessEffect {
    /// The `Tint` uniform.
    pub fn tint(&self) -> [f32; 3] {
        let a = self.ambient.get();
        [a * self.red.get(), a * self.green.get(), a * self.blue.get()]
    }
}

/// `MenuPostProcessEffect.EffectType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuEffectType {
    #[default]
    None,
    Black,
    Desaturated,
}

rustora_core::field_enum!(MenuEffectType, [None, Black, Desaturated]);

#[derive(Clone, Debug, FieldLoad)]
pub struct MenuPostProcessEffectInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub fade_length: i32,
    pub effect: MenuEffectType,
    pub menu_effect: MenuEffectType,
}

impl Default for MenuPostProcessEffectInfo {
    fn default() -> Self {
        MenuPostProcessEffectInfo {
            instance_name: None,
            fade_length: 10,
            effect: MenuEffectType::None,
            menu_effect: MenuEffectType::None,
        }
    }
}

/// `Ui.Timestep`.
const UI_TIMESTEP: i64 = 40;

/// `MenuPostProcessEffect`. `Game.RunTime` is the time since the process started.
pub struct MenuPostProcessEffect {
    pub info: Rc<MenuPostProcessEffectInfo>,
    from: Cell<MenuEffectType>,
    to: Cell<MenuEffectType>,
    start_time: Cell<i64>,
    end_time: Cell<i64>,
}

register_traits!(MenuPostProcessEffect: IWorldLoaded);
info_impl!(MenuPostProcessEffectInfo => |info, _init| Some(Rc::new(MenuPostProcessEffect {
    info,
    from: Cell::new(MenuEffectType::Black),
    to: Cell::new(MenuEffectType::Black),
    start_time: Cell::new(0),
    end_time: Cell::new(0),
})));

fn run_time() -> i64 {
    thread_local! {
        static START: Instant = Instant::now();
    }
    START.with(|s| s.elapsed().as_millis() as i64)
}

impl MenuPostProcessEffect {
    pub fn fade(&self, ty: MenuEffectType) {
        let start = run_time();
        self.start_time.set(start);
        self.end_time.set(start + UI_TIMESTEP * self.info.fade_length as i64);
        self.from.set(self.to.get());
        self.to.set(ty);
    }

    /// `Enabled` and `PrepareRender`: the from and to effects and the blend between them.
    pub fn state(&self) -> Option<(MenuEffectType, MenuEffectType, f32)> {
        if self.to.get() == MenuEffectType::None && self.end_time.get() == 0 {
            return None;
        }
        let (start, end) = (self.start_time.get(), self.end_time.get());
        let mut blend = (end - run_time()) as f32 / (end - start) as f32;
        if blend < 0.0 || end == start {
            blend = 0.0;
            self.start_time.set(0);
            self.end_time.set(0);
        }
        Some((self.from.get(), self.to.get(), blend))
    }
}

impl IWorldLoaded for MenuPostProcessEffect {
    fn world_loaded(&self, world: &Rc<World>) {
        if !world.is_loading_game_save() {
            self.fade(self.info.effect);
        }
    }
}

impl MenuPostProcessEffect {
    /// `INotifyGameLoaded.GameLoaded`, which the client calls when a game save has loaded.
    pub fn game_loaded(&self, world: &World) {
        if world.is_replay() {
            self.fade(self.info.effect);
        }
    }
}
