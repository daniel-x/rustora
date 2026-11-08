// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/World/WeatherOverlay.cs`: snow or rain particles drawn above the world. The particles and the wind
//! draw from `World.LocalRandom`, so the calls follow the C# order.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::MersenneTwister;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::{ConditionalHooks, ConditionalState, ConditionalTraitInfo, conditional_created};
use rustora_engine::render::{IRenderAboveWorld, WorldPixelShape, WorldRenderView};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::effects::premultiplied;

pub fn register(r: &mut TraitRegistry) {
    r.register("WeatherOverlay", |n| load_info!(WeatherOverlayInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct WeatherOverlayInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub particle_density_factor: i32,
    pub changing_wind_level: bool,
    pub wind_levels: Vec<i32>,
    pub wind_tick: Vec<i32>,
    pub instant_wind_changes: bool,
    pub use_squares: bool,
    pub particle_size: Vec<i32>,
    pub scatter_direction: Vec<i32>,
    pub gravity: Vec<f32>,
    pub swing_offset: Vec<f32>,
    pub swing_speed: Vec<f32>,
    pub swing_amplitude: Vec<f32>,
    pub particle_colors: Vec<Color>,
    pub line_tail_alpha_value: u8,
    pub fade_out_ticks: i32,
    pub fade_in_ticks: i32,
    pub initial_particle_percentage: i32,
}

impl Default for WeatherOverlayInfo {
    fn default() -> Self {
        WeatherOverlayInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            particle_density_factor: 8,
            changing_wind_level: true,
            wind_levels: vec![-12, -7, -5, 0, 5, 7, 12],
            wind_tick: vec![150, 550],
            instant_wind_changes: false,
            use_squares: true,
            particle_size: vec![1, 3],
            scatter_direction: vec![-1, 1],
            gravity: vec![2.5, 5.0],
            swing_offset: vec![2.5, 3.5],
            swing_speed: vec![0.0025, 0.06],
            swing_amplitude: vec![1.0, 1.5],
            particle_colors: vec![
                Color::from_argb(255, 236, 236, 236),
                Color::from_argb(255, 228, 228, 228),
                Color::from_argb(255, 208, 208, 208),
                Color::from_argb(255, 188, 188, 188),
            ],
            line_tail_alpha_value: 200,
            fade_out_ticks: 1000,
            fade_in_ticks: 1000,
            initial_particle_percentage: 100,
        }
    }
}

/// `float2.Lerp`.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[derive(Clone, Copy)]
struct Particle {
    pos: [f32; 2],
    size: i32,
    direction_scatter_x: f32,
    gravity: f32,
    swing_offset: f32,
    swing_speed: f32,
    swing_direction: i32,
    swing_amplitude: f32,
    color: Color,
    tail_color: Color,
}

impl Particle {
    /// A particle at a random place of the rectangle `(left, top, width, height)`.
    fn new(info: &WeatherOverlayInfo, r: &mut MersenneTwister, rect: (i32, i32, i32, i32)) -> Particle {
        let (left, top, width, height) = rect;
        let x = r.next_range(left, left + width);
        let y = r.next_range(top, top + height);
        let size = r.next_range(info.particle_size[0], info.particle_size[1] + 1);
        let direction_scatter_x = (info.scatter_direction[0] + r.next_max(info.scatter_direction[1] - info.scatter_direction[0])) as f32;
        let gravity = lerp(info.gravity[0], info.gravity[1], r.next_float());
        let swing_offset = lerp(info.swing_offset[0], info.swing_offset[1], r.next_float());
        let swing_speed = lerp(info.swing_speed[0], info.swing_speed[1], r.next_float());
        let swing_direction = if r.next_max(2) == 0 { 1 } else { -1 };
        let swing_amplitude = lerp(info.swing_amplitude[0], info.swing_amplitude[1], r.next_float());
        let color = info.particle_colors[r.next_max(info.particle_colors.len() as i32) as usize];
        Particle {
            pos: [x as f32, y as f32],
            size,
            direction_scatter_x,
            gravity,
            swing_offset,
            swing_speed,
            swing_direction,
            swing_amplitude,
            color,
            tail_color: Color::from_argb(info.line_tail_alpha_value, color.r(), color.g(), color.b()),
        }
    }
}

/// The particles and the size of the viewport they live in, created for the first viewport size.
struct ParticleField {
    size: (i32, i32),
    particles: Vec<Particle>,
    last_render: i64,
}

pub struct WeatherOverlay {
    info: Rc<WeatherOverlayInfo>,
    state: ConditionalState,
    fade_in_tick: i32,
    fade_out_tick: i32,
    wind_strength: Cell<f32>,
    target_wind_strength_index: Cell<usize>,
    wind_update_countdown: Cell<i64>,
    initial_tick: Cell<bool>,
    fading_in_tick: Cell<i32>,
    fading_out_tick: Cell<i32>,
    field: RefCell<Option<ParticleField>>,
}

register_traits!(WeatherOverlay: ITick, IRenderAboveWorld, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WeatherOverlayInfo => |info, init| {
    let world = init.world();
    let mut random = world.local_random.borrow_mut();
    let target = if info.changing_wind_level { random.next_max(info.wind_levels.len() as i32) as usize } else { 0 };
    let countdown = random.next_range(info.wind_tick[0], info.wind_tick[1]) as i64;
    drop(random);
    Some(Rc::new(WeatherOverlay {
        state: ConditionalState::new(&info.base),
        fade_in_tick: if info.fade_in_ticks > 0 { info.fade_in_ticks } else { 1 },
        fade_out_tick: if info.fade_out_ticks > 0 { info.fade_out_ticks } else { 1 },
        wind_strength: Cell::new(info.wind_levels[target] as f32),
        target_wind_strength_index: Cell::new(target),
        wind_update_countdown: Cell::new(countdown),
        initial_tick: Cell::new(true),
        fading_in_tick: Cell::new(0),
        fading_out_tick: Cell::new(0),
        field: RefCell::new(None),
        info,
    }))
});

impl ConditionalHooks for WeatherOverlay {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, _self_: &ActorRef) {
        if !self.initial_tick.get() {
            self.fading_in_tick
                .set((self.fade_out_tick - self.fading_out_tick.get()) * 1000 * self.fade_in_tick / self.fade_out_tick / 1000);
        }
    }

    fn trait_disabled(&self, _self_: &ActorRef) {
        if !self.initial_tick.get() {
            self.fading_out_tick
                .set((self.fade_in_tick - self.fading_in_tick.get()) * 1000 * self.fade_out_tick / self.fade_in_tick / 1000);
        }
    }
}
rustora_engine::conditional_plumbing!(WeatherOverlay);

impl INotifyCreated for WeatherOverlay {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl WeatherOverlay {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    /// `INotifyViewportZoomExtentsChanged`: spreads new particles over a viewport fixed to the minimum zoom.
    fn create_field(&self, self_: &ActorRef, view: &WorldRenderView) -> ParticleField {
        let w = (1.0 / view.min_zoom * view.native_resolution.0 as f32) as i32;
        let h = (1.0 / view.min_zoom * view.native_resolution.1 as f32) as i32;
        let count = w * h * self.info.particle_density_factor / 10000;
        let world = self_.world();
        let mut random = world.local_random.borrow_mut();
        let particles = (0..count).map(|_| Particle::new(&self.info, &mut random, (0, 0, w, h))).collect();
        ParticleField {
            size: (w, h),
            particles,
            last_render: 0,
        }
    }
}

impl ITick for WeatherOverlay {
    fn tick(&self, self_: &ActorRef) {
        if self.initial_tick.get() {
            self.initial_tick.set(false);
            let initial = self.info.initial_particle_percentage.clamp(0, 100);
            if self.is_trait_disabled() {
                self.fading_out_tick.set(self.fade_out_tick * (100 - initial) / 100);
            } else {
                self.fading_in_tick.set(self.fade_in_tick * initial / 100);
            }
        }
        if self.is_trait_disabled() {
            if self.fading_out_tick.get() < self.fade_out_tick {
                self.fading_out_tick.set(self.fading_out_tick.get() + 1);
            }
        } else if self.fading_in_tick.get() < self.fade_in_tick {
            self.fading_in_tick.set(self.fading_in_tick.get() + 1);
        }

        let levels = &self.info.wind_levels;
        if !self.info.changing_wind_level || levels.len() == 1 {
            return;
        }
        self.wind_update_countdown.set(self.wind_update_countdown.get() - 1);
        if self.wind_update_countdown.get() <= 0 {
            let world = self_.world();
            let mut random = world.local_random.borrow_mut();
            self.wind_update_countdown
                .set(random.next_range(self.info.wind_tick[0], self.info.wind_tick[1]) as i64);
            let index = self.target_wind_strength_index.get();
            if index > 0 && random.next_max(2) == 1 {
                self.target_wind_strength_index.set(index - 1);
            } else if index < levels.len() - 1 {
                self.target_wind_strength_index.set(index + 1);
            }
        }
        let target = levels[self.target_wind_strength_index.get()] as f32;
        let wind = self.wind_strength.get();
        if self.info.instant_wind_changes {
            self.wind_strength.set(target);
        } else if (wind - target).abs() > 0.01 {
            if wind > target {
                self.wind_strength.set(wind - 0.01);
            } else if wind < target {
                self.wind_strength.set(wind + 0.01);
            }
        }
    }
}

impl IRenderAboveWorld for WeatherOverlay {
    fn render_above_world(&self, self_: &ActorRef, view: &WorldRenderView, out: &mut Vec<WorldPixelShape>) {
        let mut field = self.field.borrow_mut();
        let field = field.get_or_insert_with(|| self.create_field(self_, view));
        let world = self_.world();
        let (vw, vh) = field.size;
        let (left, top) = (view.center.0 - vw / 2, view.center.1 - vh / 2);
        let tick_fraction = ((view.run_time - field.last_render) as f32 / world.timestep as f32).min(1.0);
        field.last_render = view.run_time;

        let len = field.particles.len() as i32;
        let spawn = if self.is_trait_disabled() {
            len * ((self.fade_out_tick - self.fading_out_tick.get()) * 1000 / self.fade_out_tick) / 1000
        } else {
            len * (self.fading_in_tick.get() * 1000 / self.fade_in_tick) / 1000
        };
        let wind = self.wind_strength.get();
        let paused = world.paused();
        for p in field.particles.iter_mut().take(spawn.max(0) as usize) {
            if !paused {
                let mut swing_direction = p.swing_direction;
                if p.swing_offset < -p.swing_amplitude || p.swing_offset > p.swing_amplitude {
                    swing_direction = -swing_direction;
                }
                let swing_offset = p.swing_offset + p.swing_direction as f32 * p.swing_speed;
                p.pos = [
                    p.pos[0] + tick_fraction * (p.direction_scatter_x + p.swing_offset + wind),
                    p.pos[1] + tick_fraction * p.gravity,
                ];
                p.swing_direction = swing_direction;
                p.swing_offset = swing_offset;
            }
            let (ix, iy) = (p.pos[0] as i32, p.pos[1] as i32);
            if !(left <= ix && ix < left + vw && top <= iy && iy < top + vh) {
                let mut dx = (p.pos[0] - left as f32) % vw as f32;
                let mut dy = (p.pos[1] - top as f32) % vh as f32;
                if dx < 0.0 {
                    dx += vw as f32;
                }
                if dy < 0.0 {
                    dy += vh as f32;
                }
                p.pos = [left as f32 + dx, top as f32 + dy];
            }
            if self.info.use_squares {
                out.push(WorldPixelShape::Rect {
                    x: p.pos[0],
                    y: p.pos[1],
                    w: p.size as f32,
                    h: p.size as f32,
                    color: premultiplied(p.color),
                });
            } else {
                out.push(WorldPixelShape::Line {
                    from: p.pos,
                    to: [p.pos[0] - wind, p.pos[1] - p.gravity * 2.0 / 3.0],
                    width: p.size as f32,
                    color: premultiplied(p.tail_color),
                });
            }
        }
    }
}
