// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Cosmetic render traits that add overlays, effects or animations to an actor (`Traits/Render/*`).

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use rustora_core::fieldloader::Color;
use rustora_core::world_types::{CPos, CVec, WAngle, WDist, WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::{AttackInfo, DamageState, DamageTypeTag};
use rustora_engine::effect::EffectRef;
use rustora_engine::render::{IRender, IRenderModifier, PaletteRef, Renderable, render_animation_with_offset};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sequence::{Animation, AnimationWithOffset};
use rustora_engine::target::Target;
use rustora_engine::traits::*;
use rustora_engine::{conditional_plumbing, conditional_simple, info_impl, load_info, register_traits};

use crate::attack::{Armament, Barrel};
use crate::building::BuildingInfo;
use crate::effects::{Contrail, SharedContrail, SpriteEffect};
use crate::interfaces::*;
use crate::render::{BodyOrientation, RenderSprites, WithSpriteBody};
use crate::resupply::{INotifyResupply, RESUPPLY_REARM, RESUPPLY_REPAIR};
use crate::support_powers::INotifySupportPower;

pub fn register(r: &mut TraitRegistry) {
    r.register("WithBuildingBib", |n| load_info!(WithBuildingBibInfo, n));
    r.register("WithDeathAnimation", |n| load_info!(WithDeathAnimationInfo, n));
    r.register("WithMuzzleOverlay", |n| load_info!(WithMuzzleOverlayInfo, n));
    r.register("LeavesTrails", |n| load_info!(LeavesTrailsInfo, n));
    r.register("Contrail", |n| load_info!(ContrailInfo, n));
    r.register("WithDamageOverlay", |n| load_info!(WithDamageOverlayInfo, n));
    r.register("WithColoredOverlay", |n| load_info!(WithColoredOverlayInfo, n));
    r.register("WithShadow", |n| load_info!(WithShadowInfo, n));
    r.register("WithResupplyAnimation", |n| load_info!(WithResupplyAnimationInfo, n));
    r.register("WithParachute", |n| load_info!(WithParachuteInfo, n));
    r.register("WithTurretAttackAnimation", |n| load_info!(WithTurretAttackAnimationInfo, n));
    r.register("WithTeslaChargeAnimation", |n| load_info!(WithTeslaChargeAnimationInfo, n));
    r.register("WithSupportPowerActivationAnimation", |n| {
        load_info!(WithSupportPowerActivationAnimationInfo, n)
    });
    r.register("WithLandingCraftAnimation", |n| load_info!(WithLandingCraftAnimationInfo, n));
    r.register("WithCrateBody", |n| load_info!(WithCrateBodyInfo, n));
}

fn body_named(self_: &ActorRef, name: &str) -> Rc<WithSpriteBody> {
    self_
        .traits_implementing::<WithSpriteBody>()
        .into_iter()
        .find(|w| w.info.name == name)
        .unwrap_or_else(|| panic!("{} has no sprite body {name}", self_.info.name))
}

fn normalized_color(c: Color) -> ([f32; 3], f32) {
    (
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0],
        c.a() as f32 / 255.0,
    )
}

/// `RenderUtils.ZOffsetFromCenter`.
fn z_offset_from_center(self_: &ActorRef, pos: WPos, offset: i32) -> i32 {
    let d = self_.center_position() - pos;
    d.y + d.z + offset
}

/// `Traits/Render/WithBuildingBib.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithBuildingBibInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub sequence: String,
    pub palette: String,
    pub has_minibib: bool,
}

impl Default for WithBuildingBibInfo {
    fn default() -> Self {
        WithBuildingBibInfo {
            instance_name: None,
            sequence: "bib".into(),
            palette: "terrain".into(),
            has_minibib: false,
        }
    }
}

pub struct WithBuildingBib {
    info: Rc<WithBuildingBibInfo>,
    anims: RefCell<Vec<Rc<Animation>>>,
}

register_traits!(WithBuildingBib: INotifyAddedToWorld, INotifyRemovedFromWorld);
info_impl!(WithBuildingBibInfo => |info, _init| Some(Rc::new(WithBuildingBib { info, anims: RefCell::new(Vec::new()) })));

impl INotifyAddedToWorld for WithBuildingBib {
    fn added_to_world(&self, self_: &ActorRef) {
        let rs = self_.trait_::<RenderSprites>();
        let bi = self_.info.trait_info::<BuildingInfo>().expect("WithBuildingBib requires Building");
        let w = self_.world();
        let map = w.map();
        let rows = if self.info.has_minibib { 1 } else { 2 };
        let width = bi.dimensions.x;
        let bib_offset = bi.dimensions.y - rows;
        let center_offset = bi.center_offset(&w);
        let location = self_.location();
        for i in 0..rows * width {
            let anim = Animation::new(map.sequences().clone(), &rs.get_image(self_), None, None);
            let cell = location + CVec::new(i % width, i / width + bib_offset);
            let test = format!("{}-{}", self.info.sequence, map.terrain_type_info(cell).ty);
            let sequence = if anim.has_sequence(&test) {
                test.as_str()
            } else {
                self.info.sequence.as_str()
            };
            anim.play_fetch_index(sequence, Rc::new(move || i));
            anim.is_decoration.set(true);
            let offset = map.center_of_cell(cell) - map.center_of_cell(location) - center_offset;
            let z = -(offset.y + center_offset.y + 512);
            rs.add(
                AnimationWithOffset {
                    animation: anim.clone(),
                    offset_func: Some(Rc::new(move || offset)),
                    disable_func: None,
                    zoffset_func: Some(Rc::new(move |_| z)),
                },
                Some(self.info.palette.clone()),
                false,
            );
            self.anims.borrow_mut().push(anim);
        }
    }
}

impl INotifyRemovedFromWorld for WithBuildingBib {
    fn removed_from_world(&self, self_: &ActorRef) {
        let rs = self_.trait_::<RenderSprites>();
        for a in self.anims.borrow_mut().drain(..) {
            rs.remove(&a);
        }
    }
}

/// `Traits/Render/WithDeathAnimation.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithDeathAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub death_sequence: String,
    pub death_sequence_palette: String,
    pub death_palette_is_player_palette: bool,
    pub use_death_type_suffix: bool,
    pub crushed_sequence: Option<String>,
    pub crushed_sequence_palette: String,
    pub crushed_palette_is_player_palette: bool,
    pub death_types: IndexMap<String, Vec<String>>,
    pub fallback_sequence: Option<String>,
    pub delay: i32,
}

impl Default for WithDeathAnimationInfo {
    fn default() -> Self {
        WithDeathAnimationInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            death_sequence: "die".into(),
            death_sequence_palette: "player".into(),
            death_palette_is_player_palette: true,
            use_death_type_suffix: true,
            crushed_sequence: None,
            crushed_sequence_palette: "effect".into(),
            crushed_palette_is_player_palette: false,
            death_types: IndexMap::new(),
            fallback_sequence: None,
            delay: 0,
        }
    }
}

pub struct WithDeathAnimation {
    info: Rc<WithDeathAnimationInfo>,
    state: ConditionalState,
    crushed: Cell<bool>,
}

register_traits!(WithDeathAnimation: INotifyKilled, INotifyCrushed, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithDeathAnimationInfo => |info, _init| Some(Rc::new(WithDeathAnimation {
    state: ConditionalState::new(&info.base),
    crushed: Cell::new(false),
    info,
})));
conditional_simple!(WithDeathAnimation);

impl WithDeathAnimation {
    fn spawn(&self, self_: &ActorRef, sequence: &str, palette: PaletteRef) {
        let w = self_.world();
        let pos = self_.center_position();
        let image = self_.trait_::<RenderSprites>().get_image(self_);
        let e: EffectRef = SpriteEffect::new(
            &w,
            Box::new(move || pos),
            Rc::new(|| WAngle::ZERO),
            &image,
            sequence,
            palette,
            false,
            self.info.delay,
        );
        w.add_frame_end_task(Box::new(move |w| w.add_effect(e)));
    }
}

impl INotifyKilled for WithDeathAnimation {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        if self.crushed.get() || self.state.is_trait_disabled() {
            return;
        }
        let info = &self.info;
        let palette = PaletteRef::resolve(
            Some(&info.death_sequence_palette),
            info.death_palette_is_player_palette,
            &self_.owner(),
        );
        if e.damage.damage_types.is_empty() {
            if let Some(f) = &info.fallback_sequence {
                self.spawn(self_, f, palette);
            }
            return;
        }
        let mut sequence = info.death_sequence.clone();
        if info.use_death_type_suffix {
            let Some(suffixes) = info
                .death_types
                .iter()
                .find(|(k, _)| e.damage.damage_types.contains(k))
                .map(|(_, v)| v)
            else {
                return;
            };
            let i = self_.world().shared_random.borrow_mut().next_max(suffixes.len() as i32);
            sequence.push_str(&suffixes[i as usize]);
        }
        self.spawn(self_, &sequence, palette);
    }
}

impl INotifyCrushed for WithDeathAnimation {
    fn on_crush(&self, self_: &ActorRef, _crusher: &ActorRef, _classes: BitSet<CrushClass>) {
        self.crushed.set(true);
        let info = &self.info;
        if let Some(s) = &info.crushed_sequence {
            let palette = PaletteRef::resolve(
                Some(&info.crushed_sequence_palette),
                info.crushed_palette_is_player_palette,
                &self_.owner(),
            );
            self.spawn(self_, s, palette);
        }
    }

    fn warn_crush(&self, _self_: &ActorRef, _crusher: &ActorRef, _classes: BitSet<CrushClass>) {}
}

/// `Traits/Render/WithMuzzleOverlay.cs`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct WithMuzzleOverlayInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub ignore_offset: bool,
}

struct MuzzleFlash {
    armament: Rc<Armament>,
    barrel: usize,
    visible: Rc<Cell<bool>>,
    animation: AnimationWithOffset,
}

pub struct WithMuzzleOverlay {
    info: Rc<WithMuzzleOverlayInfo>,
    state: ConditionalState,
    flashes: OnceCell<Vec<MuzzleFlash>>,
}

register_traits!(WithMuzzleOverlay: INotifyAttack, IRender, ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithMuzzleOverlayInfo => |info, _init| Some(Rc::new(WithMuzzleOverlay {
    state: ConditionalState::new(&info.base),
    flashes: OnceCell::new(),
    info,
})));
conditional_simple!(WithMuzzleOverlay);

impl WithMuzzleOverlay {
    fn flashes(&self, self_: &ActorRef) -> &Vec<MuzzleFlash> {
        self.flashes.get_or_init(|| {
            let image = self_.trait_::<RenderSprites>().get_image(self_);
            let seqs = self_.world().map().sequences().clone();
            let mut out = Vec::new();
            for arm in self_.traits_implementing::<Armament>() {
                if arm.info.muzzle_sequence.is_none() {
                    continue;
                }
                let turret = self_
                    .traits_implementing::<crate::turreted::Turreted>()
                    .into_iter()
                    .find(|t| t.name() == arm.info.turret);
                let facing_trait = self_.trait_or_default::<dyn IFacing>();
                let facing: Rc<dyn Fn() -> WAngle> = match (turret, facing_trait) {
                    (Some(t), _) => Rc::new(move || t.world_orientation().yaw),
                    (None, Some(f)) => Rc::new(move || f.facing()),
                    (None, None) => Rc::new(|| WAngle::ZERO),
                };
                for barrel in 0..arm.barrels.len() {
                    let visible = Rc::new(Cell::new(false));
                    let animation = Animation::new(seqs.clone(), &image, Some(facing.clone()), None);
                    animation.is_decoration.set(true);
                    let (a, m, ignore) = (self_.clone(), arm.clone(), self.info.ignore_offset);
                    let center = self_.clone();
                    out.push(MuzzleFlash {
                        armament: arm.clone(),
                        barrel,
                        visible: visible.clone(),
                        animation: AnimationWithOffset {
                            animation,
                            offset_func: Some(Rc::new(move || {
                                if ignore {
                                    WVec::ZERO
                                } else {
                                    m.muzzle_offset(&a, &m.barrels[barrel])
                                }
                            })),
                            disable_func: None,
                            zoffset_func: Some(Rc::new(move |p| z_offset_from_center(&center, p, 2))),
                        },
                    });
                }
            }
            out
        })
    }
}

impl INotifyAttack for WithMuzzleOverlay {
    fn attacking(&self, self_: &ActorRef, _target: &Target, a: &Armament, barrel: &Barrel) {
        let Some(sequence) = &a.info.muzzle_sequence else {
            return;
        };
        for f in self.flashes(self_) {
            if std::ptr::eq(Rc::as_ptr(&f.armament), a) && std::ptr::eq(&f.armament.barrels[f.barrel], barrel) {
                f.visible.set(true);
                let visible = f.visible.clone();
                f.animation.animation.play_then(sequence, Some(Rc::new(move || visible.set(false))));
            }
        }
    }

    fn preparing_attack(&self, _self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {}
}

impl ITick for WithMuzzleOverlay {
    fn tick(&self, self_: &ActorRef) {
        for f in self.flashes(self_) {
            if f.animation.animation.current_sequence().is_some() {
                f.animation.animation.tick();
            }
        }
    }
}

impl IRender for WithMuzzleOverlay {
    fn render(&self, self_: &ActorRef, out: &mut Vec<Renderable>) {
        if self.state.is_trait_disabled() {
            return;
        }
        for f in self.flashes(self_) {
            if f.visible.get() {
                let palette = PaletteRef::named(&f.armament.info.muzzle_palette);
                render_animation_with_offset(&f.animation, self_, &palette, out);
            }
        }
    }
}

/// `Traits/Render/LeavesTrails.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct LeavesTrailsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub image: Option<String>,
    pub sequences: Vec<String>,
    pub palette: String,
    pub terrain_types: IndexSet<String>,
    #[field(name = "Type")]
    pub trail_type: String,
    pub visible_through_fog: bool,
    pub trail_while_stationary: bool,
    pub stationary_interval: i32,
    pub trail_while_moving: bool,
    pub moving_interval: i32,
    pub start_delay: i32,
    pub offsets: Vec<WVec>,
    pub spawn_at_last_position: bool,
}

impl Default for LeavesTrailsInfo {
    fn default() -> Self {
        LeavesTrailsInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            image: None,
            sequences: vec!["idle".into()],
            palette: "effect".into(),
            terrain_types: IndexSet::new(),
            trail_type: "Cell".into(),
            visible_through_fog: false,
            trail_while_stationary: false,
            stationary_interval: 0,
            trail_while_moving: true,
            moving_interval: 0,
            start_delay: 0,
            offsets: vec![WVec::ZERO],
            spawn_at_last_position: true,
        }
    }
}

pub struct LeavesTrails {
    info: Rc<LeavesTrailsInfo>,
    state: ConditionalState,
    cached_facing: Cell<WAngle>,
    cached_interval: Cell<i32>,
    cached_position: Cell<WPos>,
    ticks: Cell<i32>,
    offset: Cell<usize>,
    is_moving: Cell<bool>,
    previous_spawn: Cell<Option<(CPos, WAngle)>>,
}

register_traits!(LeavesTrails: ITick, INotifyAddedToWorld, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(LeavesTrailsInfo => |info, _init| Some(Rc::new(LeavesTrails {
    state: ConditionalState::new(&info.base),
    cached_facing: Cell::new(WAngle::ZERO),
    cached_interval: Cell::new(info.start_delay),
    cached_position: Cell::new(WPos::ZERO),
    ticks: Cell::new(0),
    offset: Cell::new(0),
    is_moving: Cell::new(false),
    previous_spawn: Cell::new(None),
    info,
})));

fn current_facing(self_: &ActorRef) -> WAngle {
    self_.trait_or_default::<dyn IFacing>().map_or(WAngle::ZERO, |f| f.facing())
}

impl ConditionalHooks for LeavesTrails {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        self.cached_position.set(self_.center_position());
    }
}
conditional_plumbing!(LeavesTrails);

impl INotifyCreated for LeavesTrails {
    fn created(&self, self_: &ActorRef) {
        self.cached_facing.set(current_facing(self_));
        self.cached_position.set(self_.center_position());
        conditional_created(self, self_);
    }
}

impl INotifyAddedToWorld for LeavesTrails {
    fn added_to_world(&self, self_: &ActorRef) {
        self.cached_position.set(self_.center_position());
    }
}

impl ITick for LeavesTrails {
    fn tick(&self, self_: &ActorRef) {
        if !self_.is_in_world() || self.state.is_trait_disabled() {
            return;
        }
        let info = &self.info;
        let was_stationary = !self.is_moving.get();
        let is_moving = self_.center_position() != self.cached_position.get();
        self.is_moving.set(is_moving);
        if (is_moving && !info.trail_while_moving) || (!is_moving && !info.trail_while_stationary) {
            return;
        }
        if is_moving == was_stationary && info.start_delay > -1 {
            self.cached_interval.set(info.start_delay);
            self.ticks.set(0);
        }
        self.ticks.set(self.ticks.get() + 1);
        if self.ticks.get() < self.cached_interval.get() {
            return;
        }
        let w = self_.world();
        let map = w.map();
        let spawn_position = if info.spawn_at_last_position {
            self.cached_position.get()
        } else {
            self_.center_position()
        };
        let spawn_cell = map.cell_containing(spawn_position);
        if !map.contains(spawn_cell) {
            return;
        }
        let ty = map.terrain_type_info(spawn_cell).ty.clone();
        self.offset.set(if self.offset.get() + 1 >= info.offsets.len() {
            0
        } else {
            self.offset.get() + 1
        });
        if info.terrain_types.is_empty() || info.terrain_types.contains(ty.as_str()) {
            let mut spawn_facing = if info.spawn_at_last_position {
                self.cached_facing.get()
            } else {
                current_facing(self_)
            };
            if let Some((cell, facing)) = self.previous_spawn.get()
                && cell == spawn_cell
            {
                spawn_facing = facing;
            }
            let pos = if info.trail_type == "CenterPosition" {
                let body = self_.trait_::<BodyOrientation>();
                let rotated = info.offsets[self.offset.get()].rotate(&body.quantize_orientation(self_.orientation()));
                spawn_position + body.local_to_world(rotated)
            } else {
                map.center_of_cell(spawn_cell)
            };
            let i = w.local_random.borrow_mut().next_max(info.sequences.len() as i32);
            let e: EffectRef = SpriteEffect::new(
                &w,
                Box::new(move || pos),
                Rc::new(move || spawn_facing),
                info.image.as_deref().unwrap_or_default(),
                &info.sequences[i as usize],
                PaletteRef::named(&info.palette),
                info.visible_through_fog,
                0,
            );
            w.add_frame_end_task(Box::new(move |w| w.add_effect(e)));
            self.previous_spawn.set(Some((spawn_cell, spawn_facing)));
        }
        self.cached_position.set(self_.center_position());
        self.cached_facing.set(current_facing(self_));
        self.ticks.set(0);
        self.cached_interval.set(if is_moving {
            info.moving_interval
        } else {
            info.stationary_interval
        });
    }
}

/// `Traits/Render/Contrail.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct ContrailInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub offset: WVec,
    #[field(name = "ZOffset")]
    pub z_offset: i32,
    pub trail_length: i32,
    pub trail_delay: i32,
    pub start_width: WDist,
    pub end_width: Option<WDist>,
    pub start_color: Color,
    pub start_color_use_player_color: bool,
    pub start_color_alpha: i32,
    pub end_color: Option<Color>,
    pub end_color_use_player_color: bool,
    pub end_color_alpha: i32,
}

impl Default for ContrailInfo {
    fn default() -> Self {
        ContrailInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            offset: WVec::ZERO,
            z_offset: 0,
            trail_length: 25,
            trail_delay: 0,
            start_width: WDist::new(64),
            end_width: None,
            start_color: Color::from_argb(255, 255, 255, 255),
            start_color_use_player_color: true,
            start_color_alpha: 255,
            end_color: None,
            end_color_use_player_color: false,
            end_color_alpha: 0,
        }
    }
}

impl ContrailInfo {
    fn create_trail(&self, self_: &ActorRef) -> SharedContrail {
        let with_alpha = |a: i32, c: Color| Color::from_argb(a as u8, c.r(), c.g(), c.b());
        let start = with_alpha(self.start_color_alpha, self.start_color);
        let end = with_alpha(self.end_color_alpha, self.end_color.unwrap_or(start));
        let use_player_end = if self.end_color.is_none() {
            self.start_color_use_player_color
        } else {
            self.end_color_use_player_color
        };
        Rc::new(RefCell::new(Contrail::new(
            self_.clone(),
            (start, self.start_color_use_player_color),
            (end, use_player_end),
            self.start_width,
            self.end_width.unwrap_or(self.start_width),
            self.trail_length,
            self.trail_delay,
            self.z_offset,
        )))
    }
}

pub struct ContrailTrait {
    info: Rc<ContrailInfo>,
    state: ConditionalState,
    trail: RefCell<SharedContrail>,
}

register_traits!(ContrailTrait: ITick, IRender, INotifyAddedToWorld, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ContrailInfo => |info, init| Some(Rc::new(ContrailTrait {
    state: ConditionalState::new(&info.base),
    trail: RefCell::new(info.create_trail(&init.self_)),
    info,
})));
conditional_simple!(ContrailTrait);

impl ITick for ContrailTrait {
    fn tick(&self, self_: &ActorRef) {
        let body = self_.trait_::<BodyOrientation>();
        let local = self.info.offset.rotate(&body.quantize_orientation(self_.orientation()));
        self.trail
            .borrow()
            .borrow_mut()
            .update(self_.center_position() + body.local_to_world(local));
    }
}

impl IRender for ContrailTrait {
    fn render(&self, _self_: &ActorRef, out: &mut Vec<Renderable>) {
        if !self.state.is_trait_disabled() {
            self.trail.borrow().borrow().render(None, out);
        }
    }
}

impl INotifyAddedToWorld for ContrailTrait {
    fn added_to_world(&self, self_: &ActorRef) {
        *self.trail.borrow_mut() = self.info.create_trail(self_);
    }
}

/// `Traits/Render/WithDamageOverlay.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithDamageOverlayInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub image: String,
    pub idle_sequence: String,
    pub loop_sequence: String,
    pub end_sequence: String,
    pub offset: WVec,
    pub palette: Option<String>,
    pub is_player_palette: bool,
    pub damage_types: BitSet<DamageTypeTag>,
    pub minimum_damage_state: DamageState,
    pub maximum_damage_state: DamageState,
}

impl Default for WithDamageOverlayInfo {
    fn default() -> Self {
        WithDamageOverlayInfo {
            instance_name: None,
            image: "smoke_m".into(),
            idle_sequence: "idle".into(),
            loop_sequence: "loop".into(),
            end_sequence: "end".into(),
            offset: WVec::ZERO,
            palette: None,
            is_player_palette: false,
            damage_types: BitSet::default(),
            minimum_damage_state: DamageState::HEAVY,
            maximum_damage_state: DamageState::DEAD,
        }
    }
}

pub struct WithDamageOverlay {
    info: Rc<WithDamageOverlayInfo>,
    anim: Rc<Animation>,
    is_smoking: Rc<Cell<bool>>,
}

register_traits!(WithDamageOverlay: INotifyDamage, INotifyCreated);
info_impl!(WithDamageOverlayInfo => |info, init| Some(Rc::new(WithDamageOverlay {
    anim: Animation::new(init.world().map().sequences().clone(), &info.image, None, None),
    is_smoking: Rc::new(Cell::new(false)),
    info,
})));

impl INotifyCreated for WithDamageOverlay {
    fn created(&self, self_: &ActorRef) {
        let rs = self_.trait_::<RenderSprites>();
        let body = self_.trait_or_default::<BodyOrientation>();
        let offset = self.info.offset;
        let offset_func: Option<Rc<dyn Fn() -> WVec>> = match body {
            Some(b) if offset != WVec::ZERO => {
                let a = self_.clone();
                Some(Rc::new(move || {
                    b.local_to_world(offset.rotate(&b.quantize_orientation(a.orientation())))
                }))
            }
            _ => None,
        };
        let smoking = self.is_smoking.clone();
        rs.add(
            AnimationWithOffset {
                animation: self.anim.clone(),
                offset_func,
                disable_func: Some(Rc::new(move || !smoking.get())),
                zoffset_func: None,
            },
            self.info.palette.clone(),
            self.info.is_player_palette,
        );
    }
}

impl INotifyDamage for WithDamageOverlay {
    fn damaged(&self, _self_: &ActorRef, e: &AttackInfo) {
        let info = &self.info;
        if !info.damage_types.is_empty() && !e.damage.damage_types.overlaps(&info.damage_types) {
            return;
        }
        if self.is_smoking.get()
            || e.damage.value < 0
            || e.damage_state < info.minimum_damage_state
            || e.damage_state > info.maximum_damage_state
        {
            return;
        }
        self.is_smoking.set(true);
        let (a1, a2) = (self.anim.clone(), self.anim.clone());
        let (l, en, smoking) = (info.loop_sequence.clone(), info.end_sequence.clone(), self.is_smoking.clone());
        self.anim.play_then(
            &info.idle_sequence,
            Some(Rc::new(move || {
                let (a2, en, smoking) = (a2.clone(), en.clone(), smoking.clone());
                a1.play_then(
                    &l,
                    Some(Rc::new(move || {
                        let smoking = smoking.clone();
                        a2.play_then(&en, Some(Rc::new(move || smoking.set(false))));
                    })),
                );
            })),
        );
    }
}

/// `Traits/Modifiers/WithColoredOverlay.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithColoredOverlayInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub color: Color,
}

impl Default for WithColoredOverlayInfo {
    fn default() -> Self {
        WithColoredOverlayInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            color: Color::from_argb(128, 128, 0, 0),
        }
    }
}

pub struct WithColoredOverlay {
    info: Rc<WithColoredOverlayInfo>,
    state: ConditionalState,
}

register_traits!(WithColoredOverlay: IRenderModifier, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithColoredOverlayInfo => |info, _init| Some(Rc::new(WithColoredOverlay {
    state: ConditionalState::new(&info.base),
    info,
})));
conditional_simple!(WithColoredOverlay);

impl IRenderModifier for WithColoredOverlay {
    fn modify_render(&self, _self_: &ActorRef, renderables: &mut Vec<Renderable>) {
        if self.state.is_trait_disabled() {
            return;
        }
        let (tint, alpha) = normalized_color(self.info.color);
        let mut out = Vec::with_capacity(renderables.len() * 2);
        for r in renderables.drain(..) {
            let overlay = match &r {
                Renderable::Sprite(s) if !s.is_decoration => {
                    let mut o = s.clone();
                    o.replace_color = Some(tint);
                    o.alpha = alpha;
                    Some(Renderable::Sprite(o))
                }
                _ => None,
            };
            out.push(r);
            out.extend(overlay);
        }
        *renderables = out;
    }
}

/// `Traits/Render/WithShadow.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithShadowInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub shadow_color: Color,
    pub offset: WVec,
    #[field(name = "ZOffset")]
    pub z_offset: i32,
}

impl Default for WithShadowInfo {
    fn default() -> Self {
        WithShadowInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            shadow_color: Color::from_argb(140, 0, 0, 0),
            offset: WVec::ZERO,
            z_offset: -5,
        }
    }
}

pub struct WithShadow {
    info: Rc<WithShadowInfo>,
    state: ConditionalState,
}

register_traits!(WithShadow: IRenderModifier, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithShadowInfo => |info, _init| Some(Rc::new(WithShadow {
    state: ConditionalState::new(&info.base),
    info,
})));
conditional_simple!(WithShadow);

impl IRenderModifier for WithShadow {
    fn modify_render(&self, self_: &ActorRef, renderables: &mut Vec<Renderable>) {
        if self.state.is_trait_disabled() {
            return;
        }
        let height = self_.world().map().distance_above_terrain(self_.center_position()).length;
        let (tint, alpha) = normalized_color(self.info.shadow_color);
        let mut shadows: Vec<Renderable> = renderables
            .iter()
            .filter_map(|r| match r {
                Renderable::Sprite(s) if !s.is_decoration => {
                    let mut o = s.clone();
                    o.replace_color = Some(tint);
                    o.alpha = alpha;
                    o.pos = o.pos + self.info.offset - WVec::new(0, 0, height);
                    o.z_offset += height + self.info.z_offset;
                    o.is_decoration = true;
                    Some(Renderable::Sprite(o))
                }
                _ => None,
            })
            .collect();
        shadows.append(renderables);
        *renderables = shadows;
    }
}

/// `Traits/Render/WithResupplyAnimation.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithResupplyAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub sequence: String,
    pub body: String,
    pub play_animation_on: Vec<String>,
}

impl Default for WithResupplyAnimationInfo {
    fn default() -> Self {
        WithResupplyAnimationInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            sequence: "active".into(),
            body: "body".into(),
            play_animation_on: vec!["Rearm".into(), "Repair".into()],
        }
    }
}

pub struct WithResupplyAnimation {
    info: Rc<WithResupplyAnimationInfo>,
    state: ConditionalState,
    anim_playing: Cell<bool>,
    repairing: Cell<bool>,
    rearming: Cell<bool>,
}

register_traits!(WithResupplyAnimation: INotifyResupply, ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithResupplyAnimationInfo => |info, _init| Some(Rc::new(WithResupplyAnimation {
    state: ConditionalState::new(&info.base),
    anim_playing: Cell::new(false),
    repairing: Cell::new(false),
    rearming: Cell::new(false),
    info,
})));

impl ConditionalHooks for WithResupplyAnimation {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_disabled(&self, self_: &ActorRef) {
        self.repairing.set(false);
        self.rearming.set(false);
        self.anim_playing.set(false);
        body_named(self_, &self.info.body).cancel_custom_animation(self_);
    }
}
conditional_plumbing!(WithResupplyAnimation);

impl INotifyCreated for WithResupplyAnimation {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl ITick for WithResupplyAnimation {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            return;
        }
        let on = |s: &str| self.info.play_animation_on.iter().any(|t| t == s);
        let active = (self.repairing.get() && on("Repair")) || (self.rearming.get() && on("Rearm"));
        if !self.anim_playing.get() && active {
            body_named(self_, &self.info.body).play_custom_animation_repeating(self_, &self.info.sequence);
            self.anim_playing.set(true);
        } else if self.anim_playing.get() && !active {
            body_named(self_, &self.info.body).cancel_custom_animation(self_);
            self.anim_playing.set(false);
        }
        self.repairing.set(false);
        self.rearming.set(false);
    }
}

impl INotifyResupply for WithResupplyAnimation {
    fn before_resupply(&self, _self_: &ActorRef, _target: &ActorRef, types: u8) {
        self.resupply_tick(_self_, _target, types);
    }

    fn resupply_tick(&self, _self_: &ActorRef, _target: &ActorRef, types: u8) {
        self.repairing.set(types & RESUPPLY_REPAIR != 0);
        self.rearming.set(types & RESUPPLY_REARM != 0);
    }
}

/// `Traits/Render/WithParachute.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithParachuteInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub image: Option<String>,
    pub opening_sequence: Option<String>,
    pub sequence: Option<String>,
    pub closing_sequence: Option<String>,
    pub palette: String,
    pub is_player_palette: bool,
    pub offset: WVec,
    pub shadow_image: Option<String>,
    pub shadow_sequence: Option<String>,
    pub shadow_color: Color,
    pub shadow_offset: WVec,
    #[field(name = "ShadowZOffset")]
    pub shadow_z_offset: i32,
}

impl Default for WithParachuteInfo {
    fn default() -> Self {
        WithParachuteInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            image: None,
            opening_sequence: None,
            sequence: None,
            closing_sequence: None,
            palette: "player".into(),
            is_player_palette: true,
            offset: WVec::new(0, 0, 384),
            shadow_image: None,
            shadow_sequence: None,
            shadow_color: Color::from_argb(140, 0, 0, 0),
            shadow_offset: WVec::new(0, 128, 0),
            shadow_z_offset: 0,
        }
    }
}

pub struct WithParachute {
    info: Rc<WithParachuteInfo>,
    state: ConditionalState,
    shadow: Option<Rc<Animation>>,
    overlay: Option<Rc<Animation>>,
    render_prolonged: Rc<Cell<bool>>,
}

register_traits!(WithParachute: ITick, IRender, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithParachuteInfo => |info, init| {
    let self_ = init.self_.clone();
    let seqs = init.world().map().sequences().clone();
    let shadow = info.shadow_image.as_ref().map(|image| {
        let s = Animation::new(seqs.clone(), image, None, None);
        s.play_repeating(info.shadow_sequence.as_deref().unwrap_or_default());
        s
    });
    let overlay = info.image.as_ref().map(|image| Animation::new(seqs.clone(), image, None, None));
    let render_prolonged = Rc::new(Cell::new(false));
    let t = Rc::new_cyclic(|me: &std::rc::Weak<WithParachute>| {
        if let Some(anim) = &overlay {
            let body = self_.trait_::<BodyOrientation>();
            let (a, c, off) = (self_.clone(), self_.clone(), info.offset);
            let (m, prolonged) = (me.clone(), render_prolonged.clone());
            self_.trait_::<RenderSprites>().add(
                AnimationWithOffset {
                    animation: anim.clone(),
                    offset_func: Some(Rc::new(move || body.local_to_world(off.rotate(&body.quantize_orientation(a.orientation()))))),
                    disable_func: Some(Rc::new(move || m.upgrade().is_some_and(|t| t.state.is_trait_disabled()) && !prolonged.get())),
                    zoffset_func: Some(Rc::new(move |p| z_offset_from_center(&c, p, 1))),
                },
                Some(info.palette.clone()),
                info.is_player_palette,
            );
        }
        WithParachute {
            state: ConditionalState::new(&info.base),
            shadow,
            overlay,
            render_prolonged,
            info: info.clone(),
        }
    });
    Some(t)
});

impl ConditionalHooks for WithParachute {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, _self_: &ActorRef) {
        let Some(anim) = &self.overlay else { return };
        let (a, seq) = (anim.clone(), self.info.sequence.clone().unwrap_or_default());
        anim.play_then(
            self.info.opening_sequence.as_deref().unwrap_or_default(),
            Some(Rc::new(move || a.play_repeating(&seq))),
        );
    }

    fn trait_disabled(&self, _self_: &ActorRef) {
        let Some(anim) = &self.overlay else { return };
        self.render_prolonged.set(true);
        let prolonged = self.render_prolonged.clone();
        let after: Rc<dyn Fn()> = Rc::new(move || prolonged.set(false));
        match self.info.closing_sequence.as_deref().filter(|c| !c.is_empty()) {
            Some(c) => anim.play_then(c, Some(after)),
            None => anim.play_backwards_then(self.info.opening_sequence.as_deref().unwrap_or_default(), Some(after)),
        }
    }
}
conditional_plumbing!(WithParachute);

impl INotifyCreated for WithParachute {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl ITick for WithParachute {
    fn tick(&self, _self_: &ActorRef) {
        if let Some(s) = &self.shadow {
            s.tick();
        }
    }
}

impl IRender for WithParachute {
    fn render(&self, self_: &ActorRef, out: &mut Vec<Renderable>) {
        let Some(shadow) = &self.shadow else { return };
        if self.state.is_trait_disabled() || self_.is_dead() || !self_.is_in_world() {
            return;
        }
        let center = self_.center_position();
        let dat = self_.world().map().distance_above_terrain(center).length;
        let (tint, alpha) = normalized_color(self.info.shadow_color);
        let first = out.len();
        rustora_engine::render::render_animation(
            shadow,
            center - WVec::new(0, 0, dat),
            self.info.shadow_offset,
            self.info.shadow_z_offset,
            &PaletteRef::named(&self.info.palette),
            out,
        );
        for r in &mut out[first..] {
            if let Renderable::Sprite(s) = r {
                s.replace_color = Some(tint);
                s.alpha = alpha;
                s.is_decoration = true;
            }
        }
    }
}

/// `Traits/Render/WithTurretAttackAnimation.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithTurretAttackAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub armament: String,
    pub turret: String,
    pub sequence: Option<String>,
    pub delay: i32,
    pub delay_relative_to: String,
}

impl Default for WithTurretAttackAnimationInfo {
    fn default() -> Self {
        WithTurretAttackAnimationInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            armament: "primary".into(),
            turret: "primary".into(),
            sequence: None,
            delay: 0,
            delay_relative_to: "Preparation".into(),
        }
    }
}

pub struct WithTurretAttackAnimation {
    info: Rc<WithTurretAttackAnimationInfo>,
    state: ConditionalState,
    tick: Cell<i32>,
}

register_traits!(WithTurretAttackAnimation: ITick, INotifyAttack, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithTurretAttackAnimationInfo => |info, _init| Some(Rc::new(WithTurretAttackAnimation {
    state: ConditionalState::new(&info.base),
    tick: Cell::new(0),
    info,
})));
conditional_simple!(WithTurretAttackAnimation);

impl WithTurretAttackAnimation {
    fn play(&self, self_: &ActorRef) {
        if let Some(seq) = self.info.sequence.as_deref().filter(|s| !s.is_empty())
            && let Some(t) = self_
                .traits_implementing::<crate::conditions::WithSpriteTurret>()
                .into_iter()
                .find(|t| t.info.turret == self.info.turret)
        {
            t.play_custom_animation(self_, seq);
        }
    }

    fn notify(&self, self_: &ActorRef, a: &Armament, relative_to: &str) {
        if self.state.is_trait_disabled() || a.info.name != self.info.armament || self.info.delay_relative_to != relative_to {
            return;
        }
        if self.info.delay > 0 {
            self.tick.set(self.info.delay);
        } else {
            self.play(self_);
        }
    }
}

impl INotifyAttack for WithTurretAttackAnimation {
    fn attacking(&self, self_: &ActorRef, _target: &Target, a: &Armament, _barrel: &Barrel) {
        self.notify(self_, a, "Attack");
    }

    fn preparing_attack(&self, self_: &ActorRef, _target: &Target, a: &Armament, _barrel: &Barrel) {
        self.notify(self_, a, "Preparation");
    }
}

impl ITick for WithTurretAttackAnimation {
    fn tick(&self, self_: &ActorRef) {
        if !self.state.is_trait_disabled() && self.info.delay > 0 {
            self.tick.set(self.tick.get() - 1);
            if self.tick.get() == 0 {
                self.play(self_);
            }
        }
    }
}

/// `Traits/Render/WithTeslaChargeAnimation.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithTeslaChargeAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub charge_sequence: String,
    pub body: String,
}

impl Default for WithTeslaChargeAnimationInfo {
    fn default() -> Self {
        WithTeslaChargeAnimationInfo {
            instance_name: None,
            charge_sequence: "active".into(),
            body: "body".into(),
        }
    }
}

pub struct WithTeslaChargeAnimation {
    info: Rc<WithTeslaChargeAnimationInfo>,
}

register_traits!(WithTeslaChargeAnimation: INotifyTeslaCharging);
info_impl!(WithTeslaChargeAnimationInfo => |info, _init| Some(Rc::new(WithTeslaChargeAnimation { info })));

impl INotifyTeslaCharging for WithTeslaChargeAnimation {
    fn charging(&self, self_: &ActorRef, _target: &Target) {
        body_named(self_, &self.info.body).play_custom_animation(self_, &self.info.charge_sequence, None);
    }
}

/// `Traits/Render/WithSupportPowerActivationAnimation.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithSupportPowerActivationAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub sequence: String,
    pub body: String,
}

impl Default for WithSupportPowerActivationAnimationInfo {
    fn default() -> Self {
        WithSupportPowerActivationAnimationInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            sequence: "active".into(),
            body: "body".into(),
        }
    }
}

pub struct WithSupportPowerActivationAnimation {
    info: Rc<WithSupportPowerActivationAnimationInfo>,
    state: ConditionalState,
}

register_traits!(WithSupportPowerActivationAnimation: INotifySupportPower, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithSupportPowerActivationAnimationInfo => |info, _init| Some(Rc::new(WithSupportPowerActivationAnimation {
    state: ConditionalState::new(&info.base),
    info,
})));

impl ConditionalHooks for WithSupportPowerActivationAnimation {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_disabled(&self, self_: &ActorRef) {
        body_named(self_, &self.info.body).cancel_custom_animation(self_);
    }
}
conditional_plumbing!(WithSupportPowerActivationAnimation);

impl INotifyCreated for WithSupportPowerActivationAnimation {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl INotifySupportPower for WithSupportPowerActivationAnimation {
    fn charged(&self, _self_: &ActorRef) {}

    fn activated(&self, self_: &ActorRef) {
        if !self.state.is_trait_disabled() {
            body_named(self_, &self.info.body).play_custom_animation(self_, &self.info.sequence, None);
        }
    }
}

/// `Traits/Render/WithLandingCraftAnimation.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithLandingCraftAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub open_terrain_types: IndexSet<String>,
    pub open_sequence: String,
    pub close_sequence: String,
    pub unload_sequence: String,
    pub body: String,
}

impl Default for WithLandingCraftAnimationInfo {
    fn default() -> Self {
        WithLandingCraftAnimationInfo {
            instance_name: None,
            open_terrain_types: IndexSet::from(["Clear".to_string()]),
            open_sequence: "open".into(),
            close_sequence: "close".into(),
            unload_sequence: "unload".into(),
            body: "body".into(),
        }
    }
}

pub struct WithLandingCraftAnimation {
    info: Rc<WithLandingCraftAnimationInfo>,
    open: Cell<bool>,
}

register_traits!(WithLandingCraftAnimation: ITick);
info_impl!(WithLandingCraftAnimationInfo => |info, _init| Some(Rc::new(WithLandingCraftAnimation { info, open: Cell::new(false) })));

impl WithLandingCraftAnimation {
    fn should_be_open(&self, self_: &ActorRef) -> bool {
        let w = self_.world();
        let map = w.map();
        if self_.trait_::<dyn IMove>().current_movement_types() != 0 || map.distance_above_terrain(self_.center_position()).length > 0 {
            return false;
        }
        self_
            .trait_::<crate::cargo::Cargo>()
            .current_adjacent_cells()
            .iter()
            .any(|&c| map.contains(c) && self.info.open_terrain_types.contains(map.terrain_type_info(c).ty.as_str()))
    }
}

impl ITick for WithLandingCraftAnimation {
    fn tick(&self, self_: &ActorRef) {
        let body = body_named(self_, &self.info.body);
        let anim = &body.default_animation;
        if self.should_be_open(self_) {
            if !self.open.get() && anim.has_sequence(&self.info.open_sequence) {
                self.open.set(true);
                let (b, a, unload) = (body.clone(), self_.clone(), self.info.unload_sequence.clone());
                body.play_custom_animation(
                    self_,
                    &self.info.open_sequence,
                    Some(Rc::new(move || {
                        if b.default_animation.has_sequence(&unload) {
                            b.play_custom_animation_repeating(&a, &unload);
                        }
                    })),
                );
            }
        } else if self.open.get() && anim.has_sequence(&self.info.close_sequence) {
            self.open.set(false);
            body.play_custom_animation(self_, &self.info.close_sequence, None);
        }
    }
}

/// `Traits/Render/WithCrateBody.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithCrateBodyInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub xmas_images: Vec<String>,
    pub water_terrain_types: IndexSet<String>,
    pub idle_sequence: String,
    pub water_sequence: Option<String>,
    pub land_sequence: Option<String>,
}

impl Default for WithCrateBodyInfo {
    fn default() -> Self {
        WithCrateBodyInfo {
            instance_name: None,
            xmas_images: Vec::new(),
            water_terrain_types: IndexSet::from(["Water".to_string()]),
            idle_sequence: "idle".into(),
            water_sequence: None,
            land_sequence: None,
        }
    }
}

pub struct WithCrateBody {
    info: Rc<WithCrateBodyInfo>,
    anim: Rc<Animation>,
}

register_traits!(WithCrateBody: INotifyParachute, INotifyAddedToWorld);
info_impl!(WithCrateBodyInfo => |info, init| {
    let self_ = &init.self_;
    let rs = self_.trait_::<RenderSprites>();
    let anim = Animation::new(init.world().map().sequences().clone(), &rs.get_image(self_), None, None);
    anim.play(&info.idle_sequence);
    rs.add(
        AnimationWithOffset { animation: anim.clone(), offset_func: None, disable_func: None, zoffset_func: None },
        None,
        false,
    );
    Some(Rc::new(WithCrateBody { info, anim }))
});

impl WithCrateBody {
    fn play_sequence(&self, self_: &ActorRef) {
        let w = self_.world();
        let on_water = self
            .info
            .water_terrain_types
            .contains(w.map().terrain_type_info(self_.location()).ty.as_str());
        let sequence = if on_water {
            &self.info.water_sequence
        } else {
            &self.info.land_sequence
        };
        if let Some(s) = sequence.as_deref().filter(|s| !s.is_empty()) {
            self.anim.play_repeating(s);
        }
    }
}

impl INotifyAddedToWorld for WithCrateBody {
    fn added_to_world(&self, self_: &ActorRef) {
        let a = self_.clone();
        let (info, anim) = (self.info.clone(), self.anim.clone());
        self_.world().add_frame_end_task(Box::new(move |_w| {
            if !crate::util::is_at_ground_level(&a) {
                return;
            }
            WithCrateBody { info, anim }.play_sequence(&a);
        }));
    }
}

impl INotifyParachute for WithCrateBody {
    fn on_parachute(&self, _self_: &ActorRef) {}

    fn on_landed(&self, self_: &ActorRef) {
        self.play_sequence(self_);
    }
}
