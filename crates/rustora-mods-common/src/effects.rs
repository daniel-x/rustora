// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Cosmetic world effects: `SpriteEffect`, `ContrailRenderable`, and `ContrailFader`.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use rustora_core::fieldloader::Color;
use rustora_core::world_types::{WAngle, WDist, WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::effect::{Effect, EffectRef};
use rustora_engine::player::PlayerRef;
use rustora_engine::render::{PaletteRef, Renderable, fog_obscures, render_actor, render_animation};
use rustora_engine::sequence::Animation;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::world::World;

/// `Effects/SpriteEffect.cs`.
pub struct SpriteEffect {
    me: Weak<SpriteEffect>,
    pos_func: Box<dyn Fn() -> WPos>,
    pos: Cell<WPos>,
    palette: PaletteRef,
    anim: Rc<Animation>,
    sequence: String,
    visible_through_fog: bool,
    delay: Cell<i32>,
    initialized: Cell<bool>,
    finished: Rc<Cell<bool>>,
}

impl SpriteEffect {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        world: &World,
        pos_func: Box<dyn Fn() -> WPos>,
        facing: Rc<dyn Fn() -> WAngle>,
        image: &str,
        sequence: &str,
        palette: PaletteRef,
        visible_through_fog: bool,
        delay: i32,
    ) -> Rc<SpriteEffect> {
        let pos = pos_func();
        Rc::new_cyclic(|me| SpriteEffect {
            me: me.clone(),
            pos_func,
            pos: Cell::new(pos),
            palette,
            anim: Animation::new(world.map().sequences().clone(), image, Some(facing), None),
            sequence: sequence.to_string(),
            visible_through_fog,
            delay: Cell::new(delay),
            initialized: Cell::new(false),
            finished: Rc::new(Cell::new(false)),
        })
    }

    pub fn at(world: &World, pos: WPos, facing: WAngle, image: &str, sequence: &str, palette: PaletteRef) -> Rc<SpriteEffect> {
        Self::new(
            world,
            Box::new(move || pos),
            Rc::new(move || facing),
            image,
            sequence,
            palette,
            false,
            0,
        )
    }

    pub fn on_actor(actor: &ActorRef, image: &str, sequence: &str, palette: PaletteRef, delay: i32) -> Rc<SpriteEffect> {
        let a = actor.clone();
        Self::new(
            &actor.world(),
            Box::new(move || a.center_position()),
            Rc::new(|| WAngle::ZERO),
            image,
            sequence,
            palette,
            false,
            delay,
        )
    }
}

impl Effect for SpriteEffect {
    fn tick(&self, world: &Rc<World>) {
        let delay = self.delay.get();
        self.delay.set(delay - 1);
        if delay > 0 {
            return;
        }
        if !self.initialized.get() {
            let finished = self.finished.clone();
            self.anim.play_then(&self.sequence, Some(Rc::new(move || finished.set(true))));
            self.initialized.set(true);
        } else {
            self.anim.tick();
            self.pos.set((self.pos_func)());
        }
        if self.finished.replace(false) {
            let me: EffectRef = self.me.upgrade().expect("sprite effect");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
        }
    }

    fn render(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        if !self.initialized.get() || (!self.visible_through_fog && fog_obscures(viewer, self.pos.get())) {
            return;
        }
        render_animation(&self.anim, self.pos.get(), Default::default(), 0, &self.palette, out);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The contrail fields shared by `BulletInfo` and `MissileInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct ContrailFields {
    pub contrail_length: i32,
    pub contrail_delay: i32,
    #[field(name = "ContrailZOffset")]
    pub contrail_z_offset: i32,
    pub contrail_start_width: WDist,
    pub contrail_end_width: Option<WDist>,
    pub contrail_start_color: Color,
    pub contrail_start_color_use_player_color: bool,
    pub contrail_start_color_alpha: i32,
    pub contrail_end_color: Option<Color>,
    pub contrail_end_color_use_player_color: bool,
    pub contrail_end_color_alpha: i32,
}

impl Default for ContrailFields {
    fn default() -> Self {
        ContrailFields {
            contrail_length: 0,
            contrail_delay: 1,
            contrail_z_offset: 2047,
            contrail_start_width: WDist::new(64),
            contrail_end_width: None,
            contrail_start_color: Color::from_argb(255, 255, 255, 255),
            contrail_start_color_use_player_color: false,
            contrail_start_color_alpha: 255,
            contrail_end_color: None,
            contrail_end_color_use_player_color: false,
            contrail_end_color_alpha: 0,
        }
    }
}

impl ContrailFields {
    /// The contrail of a projectile fired by `owner`, if it has one.
    pub fn create(&self, owner: &ActorRef) -> Option<SharedContrail> {
        if self.contrail_length <= 0 {
            return None;
        }
        let with_alpha = |a: i32, c: Color| Color::from_argb(a as u8, c.r(), c.g(), c.b());
        let start = with_alpha(self.contrail_start_color_alpha, self.contrail_start_color);
        let end = with_alpha(self.contrail_end_color_alpha, self.contrail_end_color.unwrap_or(start));
        let use_player_end = if self.contrail_end_color.is_none() {
            self.contrail_start_color_use_player_color
        } else {
            self.contrail_end_color_use_player_color
        };
        Some(Rc::new(RefCell::new(Contrail::new(
            owner.clone(),
            (start, self.contrail_start_color_use_player_color),
            (end, use_player_end),
            self.contrail_start_width,
            self.contrail_end_width.unwrap_or(self.contrail_start_width),
            self.contrail_length,
            self.contrail_delay,
            self.contrail_z_offset,
        ))))
    }
}

pub type SharedContrail = Rc<RefCell<Contrail>>;

/// `Graphics/ContrailRenderable.cs`.
pub struct Contrail {
    owner: ActorRef,
    start_color: Color,
    use_player_start_color: bool,
    end_color: Color,
    use_player_end_color: bool,
    trail: Vec<WPos>,
    start_width: WDist,
    end_width: WDist,
    next: i32,
    length: i32,
    skip: i32,
    z_offset: i32,
}

const MAX_SMOOTH_LENGTH: i32 = 4;

fn color_lerp(t: f32, a: Color, b: Color) -> Color {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color::from_argb(l(a.a(), b.a()), l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A straight alpha color as premultiplied floats.
pub fn premultiplied(c: Color) -> [f32; 4] {
    let a = c.a() as f32 / 255.0;
    [c.r() as f32 / 255.0 * a, c.g() as f32 / 255.0 * a, c.b() as f32 / 255.0 * a, a]
}

impl Contrail {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner: ActorRef,
        start: (Color, bool),
        end: (Color, bool),
        start_width: WDist,
        end_width: WDist,
        length: i32,
        skip: i32,
        z_offset: i32,
    ) -> Contrail {
        Contrail {
            owner,
            start_color: start.0,
            use_player_start_color: start.1,
            end_color: end.0,
            use_player_end_color: end.1,
            trail: vec![WPos::ZERO; length as usize],
            start_width,
            end_width,
            next: 0,
            length: 0,
            skip,
            z_offset,
        }
    }

    pub fn trail_length(&self) -> i32 {
        self.trail.len() as i32
    }

    fn index(&self, i: i32) -> usize {
        i.rem_euclid(self.trail.len() as i32) as usize
    }

    pub fn update(&mut self, pos: WPos) {
        let next = self.index(self.next);
        self.trail[next] = pos;
        self.next = self.index(self.next + 1) as i32;
        if self.length < self.trail.len() as i32 {
            self.length += 1;
        }
    }

    pub fn render(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        let render_length = self.length - self.skip;
        if render_length <= 1 {
            return;
        }
        let player = self.owner.owner().color;
        let resolve = |c: Color, use_player: bool| {
            if use_player {
                Color::from_argb(c.a(), player.r(), player.g(), player.b())
            } else {
                c
            }
        };
        let start_color = resolve(self.start_color, self.use_player_start_color);
        let end_color = resolve(self.end_color, self.use_player_end_color);
        let mut cur_pos = self.trail[self.index(self.next - self.skip - 1)];
        let mut cur_color = start_color;
        let mut items = Vec::with_capacity(render_length as usize);
        for i in 1..render_length {
            let j = self.next - self.skip - 1 - i;
            let next_color = color_lerp(i as f32 / (render_length - 1) as f32, start_color, end_color);
            let (mut x, mut y, mut z) = (0i64, 0i64, 0i64);
            let mut k = 0;
            while k < render_length - i && k < MAX_SMOOTH_LENGTH {
                let p = self.trail[self.index(j - k)];
                x += p.x as i64;
                y += p.y as i64;
                z += p.z as i64;
                k += 1;
            }
            let k64 = k as i64;
            let next_pos = WPos::new((x / k64) as i32, (y / k64) as i32, (z / k64) as i32);
            let width = if render_length == 2 {
                self.start_width.length as f32
            } else {
                let lerp = (i - 1) as f32 / (render_length - 2) as f32;
                self.start_width.length as f32 * (1.0 - lerp) + self.end_width.length as f32 * lerp
            };
            if width > 0.0 && !fog_obscures(viewer, cur_pos) && !fog_obscures(viewer, next_pos) {
                items.push(Renderable::Line {
                    from: cur_pos,
                    to: next_pos,
                    width,
                    from_color: premultiplied(cur_color),
                    to_color: premultiplied(next_color),
                });
            }
            cur_pos = next_pos;
            cur_color = next_color;
        }
        out.push(Renderable::Group {
            pos: self.trail[self.index(self.next - 1)],
            z_offset: self.z_offset,
            items,
        });
    }
}

/// `Effects/ContrailFader.cs`.
pub struct ContrailFader {
    me: Weak<ContrailFader>,
    pos: WPos,
    trail: SharedContrail,
    ticks: Cell<i32>,
}

impl ContrailFader {
    pub fn new(pos: WPos, trail: SharedContrail) -> Rc<ContrailFader> {
        Rc::new_cyclic(|me| ContrailFader {
            me: me.clone(),
            pos,
            trail,
            ticks: Cell::new(0),
        })
    }
}

impl Effect for ContrailFader {
    fn tick(&self, world: &Rc<World>) {
        let t = self.ticks.get();
        self.ticks.set(t + 1);
        if t == self.trail.borrow().trail_length() {
            let me: EffectRef = self.me.upgrade().expect("fader");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
        }
        self.trail.borrow_mut().update(self.pos);
    }

    fn render(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        self.trail.borrow().render(viewer, out);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `OrderEffectsInfo`: how the client confirms an order. The tint flash type is not ported, RA uses the overlay.
#[derive(Clone, Debug, FieldLoad)]
pub struct OrderEffectsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub terrain_flash_image: String,
    pub terrain_flash_sequence: String,
    pub terrain_flash_palette: Option<String>,
    pub actor_flash_overlay_color: Color,
    pub actor_flash_overlay_alpha: f32,
    pub actor_flash_count: i32,
    pub actor_flash_interval: i32,
}

impl Default for OrderEffectsInfo {
    fn default() -> Self {
        OrderEffectsInfo {
            instance_name: None,
            terrain_flash_image: String::new(),
            terrain_flash_sequence: String::new(),
            terrain_flash_palette: None,
            actor_flash_overlay_color: Color::from_argb(255, 255, 255, 255),
            actor_flash_overlay_alpha: 0.5,
            actor_flash_count: 2,
            actor_flash_interval: 2,
        }
    }
}

rustora_engine::info_impl!(OrderEffectsInfo => |_info, _init| None);

/// `OrderEffects.OrderIssued`: returns whether the target type has a flash.
pub fn order_issued(world: &Rc<World>, target: &Target) -> bool {
    let Some(info) = world.world_actor().info.trait_info::<OrderEffectsInfo>() else {
        return false;
    };
    match target.target_type() {
        TargetType::Actor => {
            let Some(actor) = target.actor().cloned() else {
                return false;
            };
            let info = info.clone();
            world.add_frame_end_task(Box::new(move |w: &Rc<World>| {
                w.add_effect(FlashTarget::new(
                    w,
                    actor,
                    info.actor_flash_overlay_color,
                    info.actor_flash_overlay_alpha,
                    info.actor_flash_count,
                    info.actor_flash_interval,
                    0,
                ));
            }));
            true
        }
        TargetType::FrozenActor => true,
        TargetType::Terrain => {
            let pos = target.center_position();
            world.add_frame_end_task(Box::new(move |w: &Rc<World>| {
                let effect = SpriteAnnotation::new(
                    w,
                    pos,
                    &info.terrain_flash_image,
                    &info.terrain_flash_sequence,
                    info.terrain_flash_palette.as_deref().unwrap_or("effect"),
                );
                w.add_effect(effect);
            }));
            true
        }
        _ => false,
    }
}

/// `FlashTarget` with a color overlay.
pub struct FlashTarget {
    me: Weak<FlashTarget>,
    target: ActorRef,
    color: Color,
    alpha: f32,
    count: i32,
    interval: i32,
    tick: Cell<i32>,
}

impl FlashTarget {
    pub fn new(world: &Rc<World>, target: ActorRef, color: Color, alpha: f32, count: i32, interval: i32, delay: i32) -> Rc<FlashTarget> {
        for e in world.effects() {
            if e.as_any().downcast_ref::<FlashTarget>().is_some_and(|f| f.target.id == target.id) {
                world.remove_effect(&e);
            }
        }
        Rc::new_cyclic(|me| FlashTarget {
            me: me.clone(),
            target,
            color,
            alpha,
            count,
            interval,
            tick: Cell::new(-delay),
        })
    }
}

impl Effect for FlashTarget {
    fn tick(&self, world: &Rc<World>) {
        self.tick.set(self.tick.get() + 1);
        if self.tick.get() >= self.count * self.interval || !self.target.is_in_world() {
            let me = self.me.upgrade().expect("live effect");
            world.add_frame_end_task(Box::new(move |w: &Rc<World>| w.remove_effect(&(me as EffectRef))));
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn render(&self, _viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        if !self.target.is_in_world() || self.tick.get() < 0 || self.tick.get() % self.interval != 0 {
            return;
        }
        let mut own = Vec::new();
        render_actor(&self.target, &mut own);
        let replace = [
            self.color.r() as f32 / 255.0,
            self.color.g() as f32 / 255.0,
            self.color.b() as f32 / 255.0,
        ];
        for r in own {
            if let Renderable::Sprite(mut s) = r
                && !s.is_decoration
            {
                s.replace_color = Some(replace);
                s.alpha = self.alpha;
                out.push(Renderable::Sprite(s));
            }
        }
    }
}

/// `SpriteAnnotation`, drawn above the shroud instead of in the annotation pass.
pub struct SpriteAnnotation {
    pos: WPos,
    palette: PaletteRef,
    animation: Rc<Animation>,
}

impl SpriteAnnotation {
    pub fn new(world: &Rc<World>, pos: WPos, image: &str, sequence: &str, palette: &str) -> Rc<SpriteAnnotation> {
        let animation = Animation::new(world.map().sequences().clone(), image, None, None);
        Rc::new_cyclic(|me: &Weak<SpriteAnnotation>| {
            let (me, w) = (me.clone(), Rc::downgrade(world));
            animation.play_then(
                sequence,
                Some(Rc::new(move || {
                    if let (Some(me), Some(w)) = (me.upgrade(), w.upgrade()) {
                        w.add_frame_end_task(Box::new(move |w: &Rc<World>| w.remove_effect(&(me as EffectRef))));
                    }
                })),
            );
            SpriteAnnotation {
                pos,
                palette: PaletteRef::named(palette),
                animation,
            }
        })
    }
}

impl Effect for SpriteAnnotation {
    fn tick(&self, _world: &Rc<World>) {
        self.animation.tick();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn render_above_shroud(&self, _viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        render_animation(&self.animation, self.pos, WVec::ZERO, 0, &self.palette, out);
    }
}

/// `Effects/SpawnActorEffect.cs`: adds an actor to the world after a delay and queues its path.
pub struct SpawnActorEffect {
    me: Weak<SpawnActorEffect>,
    actor: ActorRef,
    path_after_spawn: Vec<rustora_core::world_types::CPos>,
    activity_at_destination: RefCell<Option<rustora_engine::activity::ActivityRef>>,
    move_: Option<Rc<dyn crate::interfaces::IMove>>,
    remaining_delay: Cell<i32>,
}

impl SpawnActorEffect {
    pub fn new(
        actor: ActorRef,
        delay: i32,
        path_after_spawn: Vec<rustora_core::world_types::CPos>,
        activity_at_destination: Option<rustora_engine::activity::ActivityRef>,
    ) -> Rc<SpawnActorEffect> {
        let move_ = actor.trait_or_default::<dyn crate::interfaces::IMove>();
        Rc::new_cyclic(|me| SpawnActorEffect {
            me: me.clone(),
            actor,
            path_after_spawn,
            activity_at_destination: RefCell::new(activity_at_destination),
            move_,
            remaining_delay: Cell::new(delay),
        })
    }
}

impl Effect for SpawnActorEffect {
    fn tick(&self, world: &Rc<World>) {
        let delay = self.remaining_delay.get();
        self.remaining_delay.set(delay - 1);
        if delay > 0 {
            return;
        }
        world.add(&self.actor);
        if let Some(m) = &self.move_ {
            for &cell in &self.path_after_spawn {
                self.actor.queue_activity(m.move_to(cell, 2, None, false));
            }
        }
        if let Some(a) = self.activity_at_destination.borrow_mut().take() {
            self.actor.queue_activity(a);
        }
        if let Some(me) = self.me.upgrade() {
            world.add_frame_end_task(Box::new(move |w: &Rc<World>| w.remove_effect(&(me as EffectRef))));
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
