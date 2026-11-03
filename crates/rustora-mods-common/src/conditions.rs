// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Render/Hovers.cs` (its offset moves weapon muzzles), `Traits/Huntable.cs` and
//! `Traits/Render/WithInfantryBody.cs` (its idle timers draw from the shared random).

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use rustora_core::world_types::{MersenneTwister, WAngle, WDist, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::*;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sequence::{Animation, AnimationWithOffset};
use rustora_engine::target::Target;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::attack::{Armament, Barrel};
use crate::interfaces::*;
use crate::locomotor::movement_type;
use crate::render::{RenderSprites, make_facing_func};

pub fn register(r: &mut TraitRegistry) {
    r.register("Hovers", |n| load_info!(HoversInfo, n));
    r.register("Huntable", |n| load_info!(HuntableInfo, n));
    r.register("ScaredyCat", |n| load_info!(ScaredyCatInfo, n));
    r.register("Wanders", |n| load_info!(WandersInfo, n));
    r.register("RejectsOrders", |n| load_info!(RejectsOrdersInfo, n));
    r.register("ValidateOrder", |n| load_info!(ValidateOrderInfo, n));
    r.register("WithInfantryBody", |n| load_info!(WithInfantryBodyInfo, n));
    r.register("WithSpriteTurret", |n| load_info!(WithSpriteTurretInfo, n));
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

#[derive(Clone, Debug, FieldLoad)]
pub struct WithInfantryBodyInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub min_idle_delay: i32,
    pub max_idle_delay: i32,
    pub move_sequence: String,
    pub default_attack_sequence: Option<String>,
    pub attack_sequences: IndexMap<String, Vec<String>>,
    pub idle_sequences: Vec<String>,
    pub stand_sequences: Vec<String>,
    pub palette: Option<String>,
    pub is_player_palette: bool,
}

impl Default for WithInfantryBodyInfo {
    fn default() -> Self {
        WithInfantryBodyInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            min_idle_delay: 30,
            max_idle_delay: 110,
            move_sequence: "run".into(),
            default_attack_sequence: None,
            attack_sequences: IndexMap::new(),
            idle_sequences: Vec::new(),
            stand_sequences: vec!["stand".into()],
            palette: None,
            is_player_palette: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimationState {
    Idle,
    Attacking,
    Moving,
    Waiting,
    IdleAnimating,
}

pub struct WithInfantryBody {
    info: Rc<WithInfantryBodyInfo>,
    state: ConditionalState,
    me: Weak<WithInfantryBody>,
    mv: OnceCell<Rc<dyn IMove>>,
    default_animation: Rc<Animation>,
    dirty: Cell<bool>,
    rsm: OnceCell<Option<Rc<dyn IRenderInfantrySequenceModifier>>>,
    was_modifying: Cell<bool>,
    idle_sequence: RefCell<String>,
    idle_delay: Cell<i32>,
    anim_state: Cell<AnimationState>,
    cosmetic_random: RefCell<MersenneTwister>,
}

register_traits!(WithInfantryBody: ITick, INotifyAttack, INotifyIdle, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithInfantryBodyInfo => |info, init| {
    let self_ = &init.self_;
    let rs = self_.trait_::<RenderSprites>();
    let world = init.world();
    let anim = Animation::new(world.map().sequences().clone(), &rs.get_image(self_), Some(make_facing_func(self_)), None);
    let state = ConditionalState::new(&info.base);
    let body = Rc::new_cyclic(|me: &Weak<WithInfantryBody>| WithInfantryBody {
        state,
        me: me.clone(),
        mv: OnceCell::new(),
        default_animation: anim,
        dirty: Cell::new(false),
        rsm: OnceCell::new(),
        was_modifying: Cell::new(false),
        idle_sequence: RefCell::new(String::new()),
        idle_delay: Cell::new(0),
        anim_state: Cell::new(AnimationState::Waiting),
        cosmetic_random: RefCell::new(MersenneTwister::new(0)),
        info,
    });
    let weak = Rc::downgrade(&body);
    rs.add(
        AnimationWithOffset {
            animation: body.default_animation.clone(),
            offset_func: None,
            disable_func: Some(Rc::new(move || weak.upgrade().is_some_and(|b| b.state.is_trait_disabled()))),
            zoffset_func: None,
        },
        body.info.palette.clone(),
        body.info.is_player_palette,
    );
    body.play_stand_animation();
    Some(body)
});

impl ConditionalHooks for WithInfantryBody {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}
rustora_engine::conditional_plumbing!(WithInfantryBody);

impl INotifyCreated for WithInfantryBody {
    fn created(&self, self_: &ActorRef) {
        let _ = self.mv.set(self_.trait_::<dyn IMove>());
        let _ = self.rsm.set(self_.trait_or_default::<dyn IRenderInfantrySequenceModifier>());
        self.idle_delay.set(
            self_
                .world()
                .shared_random
                .borrow_mut()
                .next_range(self.info.min_idle_delay, self.info.max_idle_delay),
        );
        conditional_created(self, self_);
    }
}

impl WithInfantryBody {
    fn play_stand_animation(&self) {
        self.anim_state.set(AnimationState::Waiting);
        let sequence = self
            .default_animation
            .get_random_existing_sequence(&self.info.stand_sequences, &mut self.cosmetic_random.borrow_mut());
        if let Some(s) = sequence {
            self.default_animation.play_repeating(&self.normalize_infantry_sequence(&s));
        }
    }

    fn rsm(&self) -> Option<&Rc<dyn IRenderInfantrySequenceModifier>> {
        self.rsm.get().and_then(|r| r.as_ref())
    }

    fn is_modifying_sequence(&self) -> bool {
        self.rsm().is_some_and(|r| r.is_modifying_sequence())
    }

    fn normalize_infantry_sequence(&self, base: &str) -> String {
        let prefix = if self.is_modifying_sequence() {
            self.rsm().unwrap().sequence_prefix()
        } else {
            String::new()
        };
        let prefixed = prefix + base;
        if self.default_animation.has_sequence(&prefixed) {
            prefixed
        } else {
            base.to_string()
        }
    }

    fn stand_callback(&self) -> Option<Rc<dyn Fn()>> {
        let me = self.me.clone();
        Some(Rc::new(move || {
            if let Some(b) = me.upgrade() {
                b.play_stand_animation();
            }
        }))
    }

    fn attacking(&self, a: &Armament, barrel: &Barrel) {
        let mut sequence = self.info.default_attack_sequence.clone();
        if let Some(list) = self.info.attack_sequences.get(&a.info.name).filter(|l| !l.is_empty()) {
            sequence = Some(list[0].clone());
            if list.len() > 1 {
                for (i, s) in list.iter().enumerate() {
                    if a.barrels[i].offset == barrel.offset && a.barrels[i].yaw == barrel.yaw {
                        sequence = Some(s.clone());
                    }
                }
            }
        }
        if let Some(s) = sequence.filter(|s| !s.is_empty() && self.default_animation.has_sequence(&self.normalize_infantry_sequence(s))) {
            self.anim_state.set(AnimationState::Attacking);
            self.default_animation
                .play_then(&self.normalize_infantry_sequence(&s), self.stand_callback());
        }
    }
}

impl INotifyAttack for WithInfantryBody {
    fn attacking(&self, _self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {}

    fn preparing_attack(&self, self_: &ActorRef, _target: &Target, a: &Armament, barrel: &Barrel) {
        let (me, arm, b) = (self.me.clone(), a.me_rc(), barrel.clone());
        self_.world().add_frame_end_task(Box::new(move |_w| {
            if let Some(m) = me.upgrade() {
                m.attacking(&arm, &b);
            }
        }));
    }
}

impl ITick for WithInfantryBody {
    fn tick(&self, self_: &ActorRef) {
        if let Some(r) = self.rsm() {
            let m = r.is_modifying_sequence();
            if self.was_modifying.get() != m {
                self.dirty.set(true);
            }
            self.was_modifying.set(m);
        }
        let moving = self.mv.get().expect("move").current_movement_types() & movement_type::HORIZONTAL != 0;
        let s = self.anim_state.get();
        if (s != AnimationState::Moving || self.dirty.get()) && moving {
            self.anim_state.set(AnimationState::Moving);
            self.default_animation
                .play_repeating(&self.normalize_infantry_sequence(&self.info.move_sequence));
        } else if ((s == AnimationState::Moving || self.dirty.get()) && !moving)
            || ((s == AnimationState::Idle || s == AnimationState::IdleAnimating) && !self_.is_idle())
        {
            self.play_stand_animation();
        }
        self.dirty.set(false);
    }
}

impl INotifyIdle for WithInfantryBody {
    fn tick_idle(&self, self_: &ActorRef) {
        if self.info.idle_sequences.is_empty() || self.is_modifying_sequence() {
            return;
        }
        let s = self.anim_state.get();
        if s == AnimationState::Waiting {
            self.anim_state.set(AnimationState::Idle);
            let w = self_.world();
            let i = w.shared_random.borrow_mut().next_max(self.info.idle_sequences.len() as i32) as usize;
            *self.idle_sequence.borrow_mut() = self.info.idle_sequences[i].clone();
            self.idle_delay.set(
                w.shared_random
                    .borrow_mut()
                    .next_range(self.info.min_idle_delay, self.info.max_idle_delay),
            );
        } else if s == AnimationState::Idle && self.idle_delay.get() > 0 {
            self.idle_delay.set(self.idle_delay.get() - 1);
            if self.idle_delay.get() == 0 {
                self.anim_state.set(AnimationState::IdleAnimating);
                let seq = self.idle_sequence.borrow().clone();
                self.default_animation.play_then(&seq, self.stand_callback());
            }
        }
    }
}

/// `Traits/Render/WithSpriteTurret.cs`. The quantized turret facings come from the sprite and are part of the sync
/// state; the animation only matters to the renderer.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithSpriteTurretInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub sequence: String,
    pub palette: Option<String>,
    pub is_player_palette: bool,
    pub turret: String,
    pub recoils: bool,
}

impl Default for WithSpriteTurretInfo {
    fn default() -> Self {
        WithSpriteTurretInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            sequence: "turret".into(),
            palette: None,
            is_player_palette: false,
            turret: "primary".into(),
            recoils: true,
        }
    }
}

pub struct WithSpriteTurret {
    state: ConditionalState,
    pub default_animation: Rc<Animation>,
}

register_traits!(WithSpriteTurret: INotifyCreated, IObservesVariables, IDisabledTrait, ISync, crate::interfaces::INotifyDamageStateChanged);
info_impl!(WithSpriteTurretInfo => |info, init| {
    let self_ = init.self_.clone();
    let rs = self_.trait_::<RenderSprites>();
    let image = rs.get_image(&self_);
    let turret = self_.traits_implementing::<crate::turreted::Turreted>().into_iter().find(|t| t.name() == info.turret).expect("turret");
    let t2 = turret.clone();
    let anim = Animation::new(init.world().map().sequences().clone(), &image, Some(Rc::new(move || t2.world_orientation().yaw)), None);
    anim.play_repeating(&crate::render::normalize_sequence(&anim, crate::render::get_damage_state(&self_), &info.sequence));
    turret.quantized_facings.set(anim.current_sequence().expect("turret sequence").facings);
    let t = Rc::new_cyclic(|me: &Weak<WithSpriteTurret>| {
        let (a, tur, recoils, name) = (self_.clone(), turret.clone(), info.recoils, info.turret.clone());
        let disabled = me.clone();
        let center = self_.clone();
        rs.add(
            AnimationWithOffset {
                animation: anim.clone(),
                offset_func: Some(Rc::new(move || {
                    let position = tur.position(&a);
                    if !recoils {
                        return position;
                    }
                    let recoil: i32 = a.traits_implementing::<Armament>().iter().filter(|arm| arm.info.turret == name).map(|arm| arm.recoil.get().length).sum();
                    let body = a.trait_::<crate::render::BodyOrientation>();
                    position + body.local_to_world(WVec::new(-recoil, 0, 0).rotate(&tur.world_orientation()))
                })),
                disable_func: Some(Rc::new(move || disabled.upgrade().is_some_and(|t| t.state.is_trait_disabled()))),
                zoffset_func: Some(Rc::new(move |p| {
                    let d = center.center_position() - p;
                    d.y + d.z + 1
                })),
            },
            info.palette.clone(),
            info.is_player_palette,
        );
        WithSpriteTurret { state: ConditionalState::new(&info.base), default_animation: anim }
    });
    Some(t)
});
rustora_engine::conditional_simple!(WithSpriteTurret);

impl crate::interfaces::INotifyDamageStateChanged for WithSpriteTurret {
    fn damage_state_changed(&self, self_: &ActorRef, _e: &rustora_engine::damage::AttackInfo) {
        if let Some(s) = self.default_animation.current_sequence() {
            let ds = crate::render::get_damage_state(self_);
            self.default_animation
                .replace_anim(&crate::render::normalize_sequence(&self.default_animation, ds, &s.name));
        }
    }
}

/// `Traits/Infantry/ScaredyCat.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct ScaredyCatInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub panic_chance: i32,
    pub panic_duration: i32,
    pub panic_speed_modifier: i32,
    pub attack_panic_chance: i32,
    pub avoid_terrain_types: Vec<String>,
    pub panic_sequence_prefix: String,
}

impl Default for ScaredyCatInfo {
    fn default() -> Self {
        ScaredyCatInfo {
            instance_name: None,
            panic_chance: 100,
            panic_duration: 250,
            panic_speed_modifier: 200,
            attack_panic_chance: 20,
            avoid_terrain_types: Vec::new(),
            panic_sequence_prefix: "panic-".into(),
        }
    }
}

pub struct ScaredyCat {
    info: Rc<ScaredyCatInfo>,
    self_: ActorRef,
    mobile: Rc<crate::mobile::Mobile>,
    panic_started_tick: Cell<i32>,
}

register_traits!(ScaredyCat: ITick, INotifyIdle, INotifyDamage, INotifyAttack, ISpeedModifier, ISync, IRenderInfantrySequenceModifier);
info_impl!(ScaredyCatInfo => |info, init| {
    Some(Rc::new(ScaredyCat { mobile: init.self_.trait_::<crate::mobile::Mobile>(), self_: init.self_.clone(), info, panic_started_tick: Cell::new(0) }))
});

impl ScaredyCat {
    fn panicking(&self) -> bool {
        self.panic_started_tick.get() > 0
    }

    pub fn panic(&self) {
        if !self.panicking() {
            self.self_.cancel_activity();
        }
        self.panic_started_tick.set(self.self_.world().world_tick());
    }
}

impl ISync for ScaredyCat {
    fn sync_hash(&self) -> i32 {
        self.panic_started_tick.get()
    }
}

impl ITick for ScaredyCat {
    fn tick(&self, self_: &ActorRef) {
        if !self.panicking() {
            return;
        }
        if self_.world().world_tick() >= self.panic_started_tick.get() + self.info.panic_duration {
            self_.cancel_activity();
            self.panic_started_tick.set(0);
        }
    }
}

impl INotifyIdle for ScaredyCat {
    fn tick_idle(&self, self_: &ActorRef) {
        if !self.panicking() {
            return;
        }
        let w = self_.world();
        let avoid = |c: rustora_core::world_types::CPos| self.info.avoid_terrain_types.contains(&w.map().terrain_type_info(c).ty);
        let filter: Option<&dyn Fn(rustora_core::world_types::CPos) -> bool> = if self.info.avoid_terrain_types.is_empty() {
            None
        } else {
            Some(&avoid)
        };
        if let Some(cell) = self.mobile.get_adjacent_cell(self_.location(), filter) {
            self_.queue_activity_queued(false, self.mobile.move_to(cell, 0, None, false));
        }
    }
}

impl INotifyDamage for ScaredyCat {
    fn damaged(&self, self_: &ActorRef, e: &rustora_engine::damage::AttackInfo) {
        if e.damage.value > 0 && self_.world().shared_random.borrow_mut().next_max(100) < self.info.panic_chance {
            self.panic();
        }
    }
}

impl INotifyAttack for ScaredyCat {
    fn attacking(&self, self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {
        if self_.world().shared_random.borrow_mut().next_max(100) < self.info.attack_panic_chance {
            self.panic();
        }
    }

    fn preparing_attack(&self, _self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {}
}

impl ISpeedModifier for ScaredyCat {
    fn get_speed_modifier(&self) -> i32 {
        if self.panicking() { self.info.panic_speed_modifier } else { 100 }
    }
}

impl IRenderInfantrySequenceModifier for ScaredyCat {
    fn is_modifying_sequence(&self) -> bool {
        self.panicking()
    }

    fn sequence_prefix(&self) -> String {
        self.info.panic_sequence_prefix.clone()
    }
}

/// `Traits/Wanders.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WandersInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub wander_move_radius: i32,
    pub reduce_move_radius_delay: i32,
    pub min_move_delay: i32,
    pub max_move_delay: i32,
    pub avoid_terrain_types: Vec<String>,
}

impl Default for WandersInfo {
    fn default() -> Self {
        WandersInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            wander_move_radius: 1,
            reduce_move_radius_delay: 5,
            min_move_delay: 0,
            max_move_delay: 0,
            avoid_terrain_types: Vec::new(),
        }
    }
}

pub struct Wanders {
    info: Rc<WandersInfo>,
    state: ConditionalState,
    mv: Rc<dyn IMove>,
    countdown: Cell<i32>,
    ticks_idle: Cell<i32>,
    effective_move_radius: Cell<i32>,
}

register_traits!(Wanders: INotifyIdle, INotifyBecomingIdle, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WandersInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    let countdown = init.world().shared_random.borrow_mut().next_range(info.min_move_delay, info.max_move_delay);
    Some(Rc::new(Wanders {
        mv: init.self_.trait_::<dyn IMove>(),
        effective_move_radius: Cell::new(info.wander_move_radius),
        countdown: Cell::new(countdown),
        ticks_idle: Cell::new(0),
        info,
        state,
    }))
});

rustora_engine::conditional_simple!(Wanders);

impl INotifyBecomingIdle for Wanders {
    fn on_becoming_idle(&self, self_: &ActorRef) {
        self.countdown.set(
            self_
                .world()
                .shared_random
                .borrow_mut()
                .next_range(self.info.min_move_delay, self.info.max_move_delay),
        );
    }
}

impl INotifyIdle for Wanders {
    fn tick_idle(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            return;
        }
        self.countdown.set(self.countdown.get() - 1);
        if self.countdown.get() > 0 {
            return;
        }
        if let Some(cell) = self.pick_target_location(self_) {
            self_.queue_activity(self.mv.move_to(cell, 0, None, false));
        }
    }
}

impl Wanders {
    fn pick_target_location(&self, self_: &ActorRef) -> Option<rustora_core::world_types::CPos> {
        let w = self_.world();
        let facing = w.shared_random.borrow_mut().next_max(255);
        let target = self_.center_position()
            + WVec::new(0, -1024 * self.effective_move_radius.get(), 0).rotate(&rustora_core::world_types::WRot::from_facing(facing));
        let cell = w.map().cell_containing(target);
        if !w.map().contains(cell) {
            self.ticks_idle.set(self.ticks_idle.get() + 1);
            if self.ticks_idle.get() % self.info.reduce_move_radius_delay == 0 {
                self.effective_move_radius.set(self.effective_move_radius.get() - 1);
            }
            return None;
        }
        if !self.info.avoid_terrain_types.is_empty() && self.info.avoid_terrain_types.contains(&w.map().terrain_type_info(cell).ty) {
            return None;
        }
        self.ticks_idle.set(0);
        self.effective_move_radius.set(self.info.wander_move_radius);
        Some(cell)
    }
}

/// `Traits/RejectsOrders.cs`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct RejectsOrdersInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub reject: IndexSet<String>,
    pub except: IndexSet<String>,
    pub remove_orders: bool,
}

pub struct RejectsOrders {
    info: Rc<RejectsOrdersInfo>,
    state: ConditionalState,
}

register_traits!(RejectsOrders: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(RejectsOrdersInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(RejectsOrders { info, state }))
});

impl ConditionalHooks for RejectsOrders {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        if self.info.remove_orders {
            self_.cancel_activity();
        }
    }
}
rustora_engine::conditional_plumbing!(RejectsOrders);

impl INotifyCreated for RejectsOrders {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

/// `RejectsOrdersExts.AcceptsOrder`.
pub fn accepts_order(a: &ActorRef, order_string: &str) -> bool {
    let traits: Vec<Rc<RejectsOrders>> = a
        .traits_implementing::<RejectsOrders>()
        .into_iter()
        .filter(|t| !t.state.is_trait_disabled())
        .collect();
    if traits.is_empty() || traits.iter().any(|t| t.info.except.contains(order_string)) {
        return true;
    }
    let mut any_rejects = false;
    for t in &traits {
        any_rejects = any_rejects || !t.info.reject.is_empty();
        if t.info.reject.contains(order_string) {
            return false;
        }
    }
    any_rejects
}

/// `Traits/World/ValidateOrder.cs`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ValidateOrderInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct ValidateOrder;

register_traits!(ValidateOrder: IValidateOrder);
info_impl!(ValidateOrderInfo => |_, _| Some(Rc::new(ValidateOrder)));

impl IValidateOrder for ValidateOrder {
    fn order_validation(&self, world: &rustora_engine::world::World, client_id: i32, order: &rustora_engine::order::Order) -> bool {
        let Some(subject) = &order.subject else { return true };
        let Some(owner) = subject.owner_opt() else { return true };
        let subject_client_id = owner.client_index;
        let Some(client) = world.lobby_info.client_with_index(subject_client_id) else {
            return false;
        };
        let is_bot_order = client.bot.is_some() && client_id == client.bot_controller_client_index;
        if subject_client_id != client_id && !is_bot_order {
            return false;
        }
        accepts_order(subject, &order.order_string)
    }
}
