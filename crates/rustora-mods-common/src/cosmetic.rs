// Copyright (c) The OpenRA Developers and Contributors. Rustora is a Rust port of OpenRA.
// SPDX-License-Identifier: GPL-3.0-or-later

//! Traits without simulation effect: `ScreenShaker`, `ShakeOnDeath`, `ThrowsParticle`, `GpsDot` and the selection bars.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::{WAngle, WDist, WPos, WRot, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::FacingInit;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::{AttackInfo, DamageTypeTag};
use rustora_engine::effect::Effect;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::render::{PaletteRef, Renderable, render_animation};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sequence::{Animation, AnimationWithOffset};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::actor_reference::TurretFacingInit;
use crate::capture::ICaptureProgressWatcher;
use crate::experience::IConditionTimerWatcher;
use crate::gps::{GpsWatcher, gps_watcher_of};
use crate::interfaces::INotifyKilled;
use crate::production::ProductionQueue;
use crate::render::{BodyOrientation, RenderSprites};
use crate::support_powers::SupportPowerManager;

pub fn register(r: &mut TraitRegistry) {
    r.register("ScreenShaker", |n| load_info!(ScreenShakerInfo, n));
    r.register("ShakeOnDeath", |n| load_info!(ShakeOnDeathInfo, n));
    r.register("ThrowsParticle", |n| load_info!(ThrowsParticleInfo, n));
    r.register("GpsDot", |n| load_info!(GpsDotInfo, n));
    r.register("TimedConditionBar", |n| load_info!(TimedConditionBarInfo, n));
    r.register("CapturableProgressBar", |n| load_info!(CapturableProgressBarInfo, n));
    r.register("ProductionBar", |n| load_info!(ProductionBarInfo, n));
    r.register("SupportPowerChargeBar", |n| load_info!(SupportPowerChargeBarInfo, n));
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ScreenShakerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

const MAX_MULTIPLIER: f32 = 3.0;

struct ShakeEffect {
    expiry_time: i32,
    position: WPos,
    intensity: i32,
    multiplier: (f32, f32),
}

/// `ScreenShaker`. The client publishes its view center and applies the offset, which only it can do.
pub struct ScreenShaker {
    effects: RefCell<Vec<ShakeEffect>>,
    ticks: Cell<i32>,
    view_center: Cell<WPos>,
    pending_scroll: Cell<(f32, f32)>,
}

register_traits!(ScreenShaker: ITick);
info_impl!(ScreenShakerInfo => |_info, _init| Some(Rc::new(ScreenShaker {
    effects: RefCell::new(Vec::new()),
    ticks: Cell::new(0),
    view_center: Cell::new(WPos::ZERO),
    pending_scroll: Cell::new((0.0, 0.0)),
})));

impl ScreenShaker {
    pub fn add_effect(&self, time: i32, position: WPos, intensity: i32, multiplier: (f32, f32)) {
        self.effects.borrow_mut().push(ShakeEffect {
            expiry_time: self.ticks.get() + time,
            position,
            intensity,
            multiplier,
        });
    }

    /// Sets the view center for the next ticks and returns the scroll of the ticks since the last call.
    pub fn take_scroll(&self, view_center: WPos) -> (f32, f32) {
        self.view_center.set(view_center);
        self.pending_scroll.replace((0.0, 0.0))
    }

    fn scroll_offset(&self) -> (f32, f32) {
        let effects = self.effects.borrow();
        let constrain = |v: f32| v.clamp(-MAX_MULTIPLIER, MAX_MULTIPLIER);
        let multiplier = (
            constrain(effects.iter().map(|e| e.multiplier.0).sum()),
            constrain(effects.iter().map(|e| e.multiplier.1).sum()),
        );
        let center = self.view_center.get();
        let sum: f32 = effects
            .iter()
            .map(|e| e.intensity as f32 / (e.position - center).length_squared() as f32)
            .sum();
        let intensity = (100.0 * 1024.0 * 1024.0 * sum).min(10.0);
        let t = self.ticks.get() as f32 * 2.0 * std::f32::consts::PI;
        (
            multiplier.0 * intensity * (t / 4.0).sin(),
            multiplier.1 * intensity * (t / 5.0).cos(),
        )
    }
}

impl ITick for ScreenShaker {
    fn tick(&self, _self_: &ActorRef) {
        if !self.effects.borrow().is_empty() {
            let (x, y) = self.scroll_offset();
            let (px, py) = self.pending_scroll.get();
            self.pending_scroll.set((px + x, py + y));
            let ticks = self.ticks.get();
            self.effects.borrow_mut().retain(|e| e.expiry_time != ticks);
        }
        self.ticks.set(self.ticks.get() + 1);
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ShakeOnDeathInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub death_types: BitSet<DamageTypeTag>,
    pub duration: i32,
    pub intensity: i32,
}

impl Default for ShakeOnDeathInfo {
    fn default() -> Self {
        ShakeOnDeathInfo {
            instance_name: None,
            death_types: BitSet::default(),
            duration: 10,
            intensity: 1,
        }
    }
}

pub struct ShakeOnDeath {
    info: Rc<ShakeOnDeathInfo>,
}

register_traits!(ShakeOnDeath: INotifyKilled);
info_impl!(ShakeOnDeathInfo => |info, _init| Some(Rc::new(ShakeOnDeath { info })));

impl INotifyKilled for ShakeOnDeath {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        if !self.info.death_types.is_empty() && !e.damage.damage_types.overlaps(&self.info.death_types) {
            return;
        }
        if let Some(shaker) = self_.world().world_actor().trait_or_default::<ScreenShaker>() {
            shaker.add_effect(self.info.duration, self_.center_position(), self.info.intensity, (1.0, 1.0));
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ThrowsParticleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub anim: String,
    pub offset: WVec,
    pub min_throw_range: WDist,
    pub max_throw_range: WDist,
    pub min_throw_angle: WAngle,
    pub max_throw_angle: WAngle,
    pub velocity: i32,
    pub turn_speed: WAngle,
}

impl Default for ThrowsParticleInfo {
    fn default() -> Self {
        ThrowsParticleInfo {
            instance_name: None,
            anim: String::new(),
            offset: WVec::ZERO,
            min_throw_range: WDist::new(256),
            max_throw_range: WDist::new(768),
            min_throw_angle: WAngle::from_degrees(30),
            max_throw_angle: WAngle::from_degrees(60),
            velocity: 75,
            turn_speed: WAngle::new(60),
        }
    }
}

pub struct ThrowsParticle {
    pos: Rc<Cell<WVec>>,
    initial_pos: WVec,
    final_pos: WVec,
    angle: WAngle,
    tick: Cell<i32>,
    length: i32,
    facing: Rc<Cell<WAngle>>,
    rotation: Cell<WAngle>,
    direction: i32,
}

register_traits!(ThrowsParticle: ITick);
info_impl!(ThrowsParticleInfo => |info, init| {
    let self_ = init.self_.clone();
    let world = self_.world();
    let rs = self_.trait_::<RenderSprites>();
    let body = self_.trait_::<BodyOrientation>();
    let body_facing = init.inits.get::<FacingInit>().map_or(WAngle::ZERO, |f| f.value);
    let facing = init
        .inits
        .get_for::<TurretFacingInit>(info.instance_name.as_deref())
        .map(|f| f.value)
        .or_else(|| init.inits.get::<FacingInit>().map(|f| f.value))
        .unwrap_or(WAngle::ZERO);
    let (throw_rotation, throw_distance, angle, rotation_speed) = {
        let mut r = world.local_random.borrow_mut();
        let rotation = WRot::from_yaw(WAngle::new(r.next_max(1024)));
        let distance = r.next_range(info.min_throw_range.length, info.max_throw_range.length);
        let angle = WAngle::new(r.next_range(info.min_throw_angle.angle, info.max_throw_angle.angle));
        (rotation, distance, angle, WDist::from_pdf(&mut r, 2).length * info.turn_speed.angle / 1024)
    };
    let initial_pos = info.offset.rotate(&body.quantize_orientation(WRot::from_yaw(body_facing)));
    let final_pos = initial_pos + WVec::new(throw_distance, 0, 0).rotate(&throw_rotation);
    let facing = Rc::new(Cell::new(facing));
    let pos = Rc::new(Cell::new(initial_pos));
    let facing_func = facing.clone();
    let anim = Animation::new(world.map().sequences().clone(), &rs.get_image(&self_), Some(Rc::new(move || facing_func.get())), None);
    anim.play_repeating(&info.anim);
    let pos_func = pos.clone();
    rs.add(
        AnimationWithOffset {
            animation: anim,
            offset_func: Some(Rc::new(move || pos_func.get())),
            disable_func: None,
            zoffset_func: None,
        },
        None,
        false,
    );
    Some(Rc::new(ThrowsParticle {
        pos,
        initial_pos,
        final_pos,
        angle,
        tick: Cell::new(0),
        length: (final_pos - initial_pos).length() / info.velocity,
        facing,
        rotation: Cell::new(WAngle::new(rotation_speed.abs())),
        direction: if rotation_speed < 0 { -1 } else { 1 },
    }))
});

impl ITick for ThrowsParticle {
    fn tick(&self, _self_: &ActorRef) {
        let tick = self.tick.get();
        if tick >= self.length {
            return;
        }
        self.pos.set(WVec::lerp_quadratic(
            self.initial_pos,
            self.final_pos,
            self.angle,
            tick,
            self.length,
        ));
        self.tick.set(tick + 1);
        let rotation = self.rotation.get().angle;
        self.facing.set(self.facing.get() + WAngle::new(self.direction * rotation));
        self.rotation.set(WAngle::new(rotation * 90 / 100));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GpsDotInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub image: String,
    pub string: String,
    pub indicator_palette_prefix: String,
}

impl Default for GpsDotInfo {
    fn default() -> Self {
        GpsDotInfo {
            instance_name: None,
            image: "gpsdot".into(),
            string: "Infantry".into(),
            indicator_palette_prefix: "player".into(),
        }
    }
}

pub struct GpsDot {
    info: Rc<GpsDotInfo>,
    effect: OnceCell<Rc<GpsDotEffect>>,
}

register_traits!(GpsDot: INotifyCreated, INotifyAddedToWorld, INotifyRemovedFromWorld);
info_impl!(GpsDotInfo => |info, _init| Some(Rc::new(GpsDot { info, effect: OnceCell::new() })));

impl INotifyCreated for GpsDot {
    fn created(&self, self_: &ActorRef) {
        let world = self_.world();
        let anim = Animation::new(world.map().sequences().clone(), &self.info.image, None, None);
        anim.play_repeating(&self.info.string);
        let states = world
            .players()
            .into_iter()
            .filter_map(|p| {
                let watcher = gps_watcher_of(&p)?;
                Some(DotState {
                    player: p,
                    watcher,
                    visible: Cell::new(false),
                })
            })
            .collect();
        let _ = self.effect.set(Rc::new(GpsDotEffect {
            actor: self_.clone(),
            palette_prefix: self.info.indicator_palette_prefix.clone(),
            anim,
            states,
            modifiers: self_.traits_implementing::<dyn IVisibilityModifier>(),
        }));
    }
}

impl INotifyAddedToWorld for GpsDot {
    fn added_to_world(&self, self_: &ActorRef) {
        if let Some(e) = self.effect.get().cloned() {
            self_.world().add_frame_end_task(Box::new(move |w: &Rc<World>| w.add_effect(e)));
        }
    }
}

impl INotifyRemovedFromWorld for GpsDot {
    fn removed_from_world(&self, self_: &ActorRef) {
        if let Some(e) = self.effect.get().cloned() {
            self_
                .world()
                .add_frame_end_task(Box::new(move |w: &Rc<World>| w.remove_effect(&(e as Rc<dyn Effect>))));
        }
    }
}

struct DotState {
    player: PlayerRef,
    watcher: Rc<GpsWatcher>,
    visible: Cell<bool>,
}

/// `GpsDotEffect`. The actors with `GpsDot` have no frozen state, so its frozen check does not apply.
pub struct GpsDotEffect {
    actor: ActorRef,
    palette_prefix: String,
    anim: Rc<Animation>,
    states: Vec<DotState>,
    modifiers: Vec<Rc<dyn IVisibilityModifier>>,
}

impl GpsDotEffect {
    fn effective_owner(&self) -> PlayerRef {
        self.actor
            .cache()
            .effective_owner
            .as_ref()
            .and_then(|e| e.owner())
            .unwrap_or_else(|| self.actor.owner())
    }

    fn should_render(&self, state: &DotState) -> bool {
        if !state.watcher.granted() && !state.watcher.granted_allies() {
            return false;
        }
        if state.player.is_allied_with(Some(&self.effective_owner())) {
            return false;
        }
        let shroud = state.player.shroud();
        let pos = self.actor.center_position();
        if !shroud.is_explored_wpos(pos) || shroud.is_visible_wpos(pos) {
            return false;
        }
        self.modifiers.iter().all(|m| m.is_visible(&self.actor, Some(&state.player)))
    }
}

impl Effect for GpsDotEffect {
    fn tick(&self, _world: &Rc<World>) {
        for state in &self.states {
            state.visible.set(self.should_render(state));
        }
    }

    fn render_above_shroud(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        let Some(viewer) = viewer else {
            return;
        };
        if !self.states.iter().any(|s| Rc::ptr_eq(&s.player, viewer) && s.visible.get()) {
            return;
        }
        let palette = PaletteRef::player(&self.palette_prefix, &self.effective_owner());
        render_animation(&self.anim, self.actor.center_position(), WVec::ZERO, 0, &palette, out);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

const SKY_BLUE: (u8, u8, u8) = (135, 206, 235);
const RED: (u8, u8, u8) = (255, 0, 0);
const MAGENTA: (u8, u8, u8) = (255, 0, 255);

fn color((r, g, b): (u8, u8, u8)) -> Color {
    Color::from_argb(255, r, g, b)
}

/// `Color.Magenta`, the bar of `PortableChrono`.
pub fn magenta() -> Color {
    color(MAGENTA)
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TimedConditionBarInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub condition: String,
    pub color: Color,
}

impl Default for TimedConditionBarInfo {
    fn default() -> Self {
        TimedConditionBarInfo {
            instance_name: None,
            condition: String::new(),
            color: color(RED),
        }
    }
}

pub struct TimedConditionBar {
    info: Rc<TimedConditionBarInfo>,
    value: Cell<f32>,
}

register_traits!(TimedConditionBar: ISelectionBar, IConditionTimerWatcher);
info_impl!(TimedConditionBarInfo => |info, _init| Some(Rc::new(TimedConditionBar { info, value: Cell::new(0.0) })));

impl IConditionTimerWatcher for TimedConditionBar {
    fn update(&self, duration: i32, remaining: i32) {
        self.value.set(if duration > 0 { remaining as f32 / duration as f32 } else { 0.0 });
    }

    fn condition(&self) -> &str {
        &self.info.condition
    }
}

impl ISelectionBar for TimedConditionBar {
    fn value(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> f32 {
        if self_.owner().is_allied_with(viewer.map(|v| &**v)) {
            self.value.get()
        } else {
            0.0
        }
    }

    fn color(&self) -> Color {
        self.info.color
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CapturableProgressBarInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub color: Color,
}

impl Default for CapturableProgressBarInfo {
    fn default() -> Self {
        CapturableProgressBarInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            color: color((255, 165, 0)),
        }
    }
}

pub struct CapturableProgressBar {
    info: Rc<CapturableProgressBarInfo>,
    state: ConditionalState,
    progress: RefCell<HashMap<u32, (i32, i32)>>,
}

register_traits!(CapturableProgressBar: ISelectionBar, ICaptureProgressWatcher, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(CapturableProgressBarInfo => |info, _init| Some(Rc::new(CapturableProgressBar {
    state: ConditionalState::new(&info.base),
    info,
    progress: RefCell::new(HashMap::new()),
})));
rustora_engine::conditional_simple!(CapturableProgressBar);

impl ICaptureProgressWatcher for CapturableProgressBar {
    fn update(&self, self_: &ActorRef, captor: &ActorRef, target: &ActorRef, progress: i32, total: i32) {
        if self_.id != target.id {
            return;
        }
        if total == 0 {
            self.progress.borrow_mut().remove(&captor.id);
        } else {
            self.progress.borrow_mut().insert(captor.id, (progress, total));
        }
    }
}

impl ISelectionBar for CapturableProgressBar {
    fn value(&self, _self_: &ActorRef, _viewer: Option<&PlayerRef>) -> f32 {
        if self.state.is_trait_disabled() {
            return 0.0;
        }
        self.progress
            .borrow()
            .values()
            .map(|&(current, total)| current as f32 / total as f32)
            .fold(0.0, f32::max)
    }

    fn color(&self) -> Color {
        self.info.color
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ProductionBarInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub production_type: String,
    pub color: Color,
}

impl Default for ProductionBarInfo {
    fn default() -> Self {
        ProductionBarInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            production_type: String::new(),
            color: color(SKY_BLUE),
        }
    }
}

pub struct ProductionBar {
    info: Rc<ProductionBarInfo>,
    state: ConditionalState,
    queue: RefCell<Option<Rc<ProductionQueue>>>,
    value: Cell<f32>,
}

register_traits!(ProductionBar: ISelectionBar, ITick, INotifyOwnerChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ProductionBarInfo => |info, _init| Some(Rc::new(ProductionBar {
    state: ConditionalState::new(&info.base),
    info,
    queue: RefCell::new(None),
    value: Cell::new(0.0),
})));
rustora_engine::conditional_observers_impl!(ProductionBar);

impl ConditionalHooks for ProductionBar {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl ISync for ProductionBar {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash()
    }
}

impl ProductionBar {
    fn find_queue(&self, self_: &ActorRef) {
        let matches = |q: &Rc<ProductionQueue>| q.info.ty == self.info.production_type;
        let queue = self_
            .traits_implementing::<ProductionQueue>()
            .into_iter()
            .find(matches)
            .or_else(|| {
                self_
                    .owner()
                    .player_actor()
                    .traits_implementing::<ProductionQueue>()
                    .into_iter()
                    .find(matches)
            });
        *self.queue.borrow_mut() = queue;
    }
}

impl INotifyCreated for ProductionBar {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
        self.find_queue(self_);
    }
}

impl INotifyOwnerChanged for ProductionBar {
    fn on_owner_changed(&self, self_: &ActorRef, _old_owner: &PlayerRef, _new_owner: &PlayerRef) {
        self.find_queue(self_);
    }
}

impl ITick for ProductionBar {
    fn tick(&self, _self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            return;
        }
        let Some(queue) = self.queue.borrow().clone() else {
            return;
        };
        let current = queue
            .all_queued()
            .into_iter()
            .filter(|i| i.started.get())
            .min_by_key(|i| i.remaining_time.get());
        self.value.set(match current {
            None => 0.0,
            Some(i) if i.total_cost <= 0 => 1.0,
            Some(i) => 1.0 - i.remaining_cost.get() as f32 / i.total_cost as f32,
        });
    }
}

impl ISelectionBar for ProductionBar {
    fn value(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> f32 {
        if self.state.is_trait_disabled() || !self_.owner().is_allied_with(viewer.map(|v| &**v)) {
            return 0.0;
        }
        self.value.get()
    }

    fn color(&self) -> Color {
        self.info.color
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct SupportPowerChargeBarInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub display_relationships: PlayerRelationship,
    pub color: Color,
}

impl Default for SupportPowerChargeBarInfo {
    fn default() -> Self {
        SupportPowerChargeBarInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            display_relationships: PlayerRelationship::ALLY,
            color: color((255, 0, 255)),
        }
    }
}

pub struct SupportPowerChargeBar {
    info: Rc<SupportPowerChargeBarInfo>,
    state: ConditionalState,
}

register_traits!(SupportPowerChargeBar: ISelectionBar, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(SupportPowerChargeBarInfo => |info, _init| Some(Rc::new(SupportPowerChargeBar {
    state: ConditionalState::new(&info.base),
    info,
})));
rustora_engine::conditional_simple!(SupportPowerChargeBar);

impl ISelectionBar for SupportPowerChargeBar {
    fn value(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> f32 {
        if self.state.is_trait_disabled() {
            return 0.0;
        }
        let owner = self_.owner();
        let Some(manager) = owner.player_actor().trait_or_default::<SupportPowerManager>() else {
            return 0.0;
        };
        let Some(power) = manager.get_powers_for_actor(self_).into_iter().find(|p| !p.disabled()) else {
            return 0.0;
        };
        if viewer.is_some_and(|v| !self.info.display_relationships.has_relationship(owner.relationship_with(Some(v)))) {
            return 0.0;
        }
        1.0 - power.remaining_ticks() as f32 / power.total_ticks as f32
    }

    fn color(&self) -> Color {
        self.info.color
    }
}
