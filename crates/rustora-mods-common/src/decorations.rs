// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `IDecoration` traits (`WithDecoration*`) and the annotation circles (`RenderRangeCircle` and others). The traits
//! describe what to draw, and the client draws it in screen pixels above the shroud.

use std::cell::Cell;
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_core::fieldloader::Color;
use rustora_core::world_types::{Int2, WAngle, WDist, WPos};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_info::TraitNode;
use rustora_engine::conditional::*;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sequence::Animation;
use rustora_engine::traits::*;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits};

use crate::resupply::AmmoPool;

pub fn register(r: &mut TraitRegistry) {
    r.register("WithDecoration", |n| load_info!(WithDecorationInfo, n));
    r.register("WithTextDecoration", |n| load_info!(WithTextDecorationInfo, n));
    r.register("WithNameTagDecoration", |n| load_info!(WithNameTagDecorationInfo, n));
    r.register("WithBuildingRepairDecoration", |n| load_info!(WithBuildingRepairDecorationInfo, n));
    r.register("WithAmmoPipsDecoration", |n| load_pips(n, PipsKind::Ammo));
    r.register("WithResourceStoragePipsDecoration", |n| load_pips(n, PipsKind::Storage));
    r.register("WithStoresResourcesPipsDecoration", |n| load_pips(n, PipsKind::Stores));
    r.register("WithCargoPipsDecoration", |n| load_pips(n, PipsKind::Cargo));
    r.register("WithSpriteControlGroupDecoration", |n| {
        load_info!(WithSpriteControlGroupDecorationInfo, n)
    });
    r.register("RenderRangeCircle", |n| load_info!(RenderRangeCircleInfo, n));
    r.register("RenderDetectionCircle", |n| load_info!(RenderDetectionCircleInfo, n));
    r.register("RenderShroudCircle", |n| load_info!(RenderShroudCircleInfo, n));
    r.register("WithRangeCircle", |n| load_info!(WithRangeCircleInfo, n));
}

fn load_pips(n: &TraitNode, kind: PipsKind) -> Result<Rc<dyn TraitInfo>, String> {
    let mut info = WithPipsDecorationInfo {
        kind,
        ..Default::default()
    };
    rustora_core::fieldloader::FieldLoad::load(&mut info, &n.yaml).map_err(|e| format!("Trait name {}: {e}", n.display_name()))?;
    info.instance_name = n.instance_name.clone();
    Ok(Rc::new(info))
}

/// One sprite of a decoration, centered on the decoration origin and moved by `index` strides. A zero stride means
/// the sprite width.
#[derive(Clone, Debug)]
pub struct DecorationSprite {
    pub image: String,
    pub sequence: String,
    pub frame: i32,
    pub palette: String,
    pub index: i32,
    pub stride: Int2,
}

/// The words of a decoration text.
pub enum DecorationLabel {
    Text(String),
    /// `Player.ResolvedPlayerName` of the owner, cut to `max_length` characters. The client resolves bot names.
    OwnerName {
        max_length: usize,
    },
}

/// A text centered on the decoration origin, as `UITextRenderable`.
pub struct DecorationText {
    pub font: String,
    pub color: Color,
    pub label: DecorationLabel,
}

/// A decoration at a named position of the decoration bounds.
pub struct Decoration {
    pub position: String,
    pub margin: Int2,
    pub sprites: Vec<DecorationSprite>,
    pub text: Option<DecorationText>,
}

/// `IDecoration`.
pub trait IDecoration {
    fn requires_selection(&self) -> bool;
    fn render_decoration(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> Option<Decoration>;
}

/// `WithDecorationBaseInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithDecorationBaseInfo {
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub position: String,
    pub valid_relationships: PlayerRelationship,
    pub requires_selection: bool,
    pub margin: Int2,
    pub blink_interval: i32,
    pub blink_pattern: Vec<String>,
}

impl Default for WithDecorationBaseInfo {
    fn default() -> Self {
        WithDecorationBaseInfo {
            base: ConditionalTraitInfo::default(),
            position: "TopLeft".into(),
            valid_relationships: PlayerRelationship::ALLY,
            requires_selection: false,
            margin: Int2::default(),
            blink_interval: 5,
            blink_pattern: Vec::new(),
        }
    }
}

impl WithDecorationBaseInfo {
    /// `WithDecorationBase.ShouldRender`.
    fn should_render(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> bool {
        if viewer.is_some_and(|v| !self_.can_be_viewed_by_player(Some(v))) {
            return false;
        }
        if !self.blink_pattern.is_empty() {
            let i = self_.world().world_tick() / self.blink_interval % self.blink_pattern.len() as i32;
            if self.blink_pattern[i as usize] != "On" {
                return false;
            }
        }
        viewer.is_none_or(|v| self.valid_relationships.has_relationship(self_.owner().relationship_with(Some(v))))
    }

    fn decoration(&self, sprites: Vec<DecorationSprite>) -> Option<Decoration> {
        Some(Decoration {
            position: self.position.clone(),
            margin: self.margin,
            sprites,
            text: None,
        })
    }

    fn text_decoration(&self, text: DecorationText) -> Option<Decoration> {
        Some(Decoration {
            position: self.position.clone(),
            margin: self.margin,
            sprites: Vec::new(),
            text: Some(text),
        })
    }
}

fn is_active(state: &ConditionalState, self_: &ActorRef) -> bool {
    !state.is_trait_disabled() && !self_.is_dead() && self_.is_in_world()
}

/// `Traits/Render/WithDecoration.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithDecorationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub deco: WithDecorationBaseInfo,
    pub image: Option<String>,
    pub sequence: String,
    pub palette: String,
    pub is_player_palette: bool,
}

impl Default for WithDecorationInfo {
    fn default() -> Self {
        WithDecorationInfo {
            instance_name: None,
            deco: WithDecorationBaseInfo::default(),
            image: None,
            sequence: String::new(),
            palette: "chrome".into(),
            is_player_palette: false,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct WithDecoration {
    info: Rc<WithDecorationInfo>,
    state: ConditionalState,
    anim: Rc<Animation>,
    image: String,
}

register_traits!(WithDecoration: IDecoration, ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithDecorationInfo => |info, init| {
    let image = info.image.clone().unwrap_or_else(|| init.self_.info.name.clone()).to_lowercase();
    let w = Rc::downgrade(&init.world());
    let paused: Rc<dyn Fn() -> bool> = Rc::new(move || w.upgrade().is_some_and(|w| w.paused()));
    let anim = Animation::new(init.world().map().sequences().clone(), &image, None, Some(paused));
    anim.play_repeating(&info.sequence);
    Some(Rc::new(WithDecoration { state: ConditionalState::new(&info.deco.base), info, anim, image }))
});
conditional_simple!(WithDecoration);

impl ITick for WithDecoration {
    fn tick(&self, _self_: &ActorRef) {
        self.anim.tick();
    }
}

impl IDecoration for WithDecoration {
    fn requires_selection(&self) -> bool {
        self.info.deco.requires_selection
    }

    fn render_decoration(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> Option<Decoration> {
        if !is_active(&self.state, self_) || !self.info.deco.should_render(self_, viewer) {
            return None;
        }
        let palette = if self.info.is_player_palette {
            format!("{}{}", self.info.palette, self_.owner().internal_name)
        } else {
            self.info.palette.clone()
        };
        self.info.deco.decoration(vec![DecorationSprite {
            image: self.image.clone(),
            sequence: self.anim.current_sequence()?.name.clone(),
            frame: self.anim.current_frame(),
            palette,
            index: 0,
            stride: Int2::default(),
        }])
    }
}

/// `Traits/Render/WithBuildingRepairDecoration.cs`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct WithBuildingRepairDecorationInfo {
    #[field(flatten)]
    pub base: WithDecorationInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct WithBuildingRepairDecoration {
    info: Rc<WithBuildingRepairDecorationInfo>,
    state: ConditionalState,
    anim: Rc<Animation>,
    shown_player: Rc<Cell<usize>>,
}

register_traits!(WithBuildingRepairDecoration: IDecoration, ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithBuildingRepairDecorationInfo => |info, init| {
    let image = info.base.image.clone().unwrap_or_else(|| init.self_.info.name.clone()).to_lowercase();
    let a = init.self_.clone();
    let paused: Rc<dyn Fn() -> bool> = Rc::new(move || {
        a.trait_or_default::<crate::base::RepairableBuilding>().is_none_or(|r| !r.repair_active() || r.is_trait_disabled())
    });
    let anim = Animation::new(init.world().map().sequences().clone(), &image, None, Some(paused));
    let shown_player = Rc::new(Cell::new(0));
    anim.play_repeating(&info.base.sequence);
    Some(Rc::new(WithBuildingRepairDecoration { state: ConditionalState::new(&info.base.deco.base), info, anim, shown_player }))
});
conditional_simple!(WithBuildingRepairDecoration);

impl ITick for WithBuildingRepairDecoration {
    fn tick(&self, self_: &ActorRef) {
        let before = self.anim.current_frame();
        self.anim.tick();
        if self.anim.current_frame() < before
            && let Some(r) = self_.trait_or_default::<crate::base::RepairableBuilding>()
        {
            let count = r.repairers.borrow().len().max(1);
            self.shown_player.set((self.shown_player.get() + 1) % count);
        }
    }
}

impl IDecoration for WithBuildingRepairDecoration {
    fn requires_selection(&self) -> bool {
        self.info.base.deco.requires_selection
    }

    fn render_decoration(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> Option<Decoration> {
        let info = &self.info.base;
        let repairable = self_.trait_or_default::<crate::base::RepairableBuilding>()?;
        let repairers = repairable.repairers.borrow();
        if repairers.is_empty() || !is_active(&self.state, self_) || !info.deco.should_render(self_, viewer) {
            return None;
        }
        let palette = if info.is_player_palette {
            format!(
                "{}{}",
                info.palette,
                repairers[self.shown_player.get() % repairers.len()].internal_name
            )
        } else {
            info.palette.clone()
        };
        info.deco.decoration(vec![DecorationSprite {
            image: self.anim.name.borrow().clone(),
            sequence: self.anim.current_sequence()?.name.clone(),
            frame: self.anim.current_frame(),
            palette,
            index: 0,
            stride: Int2::default(),
        }])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PipsKind {
    #[default]
    Ammo,
    Storage,
    Stores,
    Cargo,
}

/// `WithAmmoPipsDecorationInfo`, `WithResourceStoragePipsDecorationInfo` and `WithStoresResourcesPipsDecorationInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithPipsDecorationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(ignore)]
    pub kind: PipsKind,
    #[field(flatten)]
    pub deco: WithDecorationBaseInfo,
    pub pip_count: i32,
    pub pip_stride: Int2,
    pub image: String,
    pub empty_sequence: String,
    pub full_sequence: String,
    pub palette: String,
    pub ammo_pools: Vec<String>,
    pub resource_sequences: IndexMap<String, String>,
    pub custom_pip_sequences: IndexMap<String, String>,
}

impl Default for WithPipsDecorationInfo {
    fn default() -> Self {
        WithPipsDecorationInfo {
            instance_name: None,
            kind: PipsKind::Ammo,
            deco: WithDecorationBaseInfo::default(),
            pip_count: 0,
            pip_stride: Int2::default(),
            image: "pips".into(),
            empty_sequence: "pip-empty".into(),
            full_sequence: "pip-green".into(),
            palette: "chrome".into(),
            ammo_pools: Vec::new(),
            resource_sequences: IndexMap::new(),
            custom_pip_sequences: IndexMap::new(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct WithPipsDecoration {
    info: Rc<WithPipsDecorationInfo>,
    state: ConditionalState,
}

register_traits!(WithPipsDecoration: IDecoration, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithPipsDecorationInfo => |info, _init| Some(Rc::new(WithPipsDecoration {
    state: ConditionalState::new(&info.deco.base),
    info,
})));
conditional_simple!(WithPipsDecoration);

impl WithPipsDecoration {
    fn sequences(&self, self_: &ActorRef) -> Vec<String> {
        let info = &self.info;
        let (full, empty) = (&info.full_sequence, &info.empty_sequence);
        match info.kind {
            PipsKind::Ammo => {
                let pools: Vec<Rc<AmmoPool>> = self_
                    .traits_implementing::<AmmoPool>()
                    .into_iter()
                    .filter(|p| info.ammo_pools.is_empty() || info.ammo_pools.contains(&p.info.name))
                    .collect();
                let current: i32 = pools.iter().map(|p| p.current_ammo_count()).sum();
                let total: i32 = pools.iter().map(|p| p.info.ammo).sum();
                let count = if info.pip_count > 0 { info.pip_count } else { total };
                (0..count)
                    .map(|i| if current * count > i * total { full } else { empty }.clone())
                    .collect()
            }
            PipsKind::Storage => {
                let pr = self_.owner().player_actor().trait_::<crate::player::PlayerResources>();
                let (r, c) = (pr.resources.get(), pr.resource_capacity.get());
                (0..info.pip_count)
                    .map(|i| if r * info.pip_count > i * c { full } else { empty }.clone())
                    .collect()
            }
            PipsKind::Stores => {
                let Some(store) = self_.trait_or_default::<crate::harvester::StoresResources>() else {
                    return Vec::new();
                };
                let contents = store.contents();
                (0..info.pip_count)
                    .map(|i| {
                        let mut n = i * store.capacity() / info.pip_count;
                        for (ty, amount) in &contents {
                            if n < *amount {
                                return info.resource_sequences.get(&**ty).unwrap_or(full).clone();
                            }
                            n -= amount;
                        }
                        empty.clone()
                    })
                    .collect()
            }
            PipsKind::Cargo => {
                let Some(cargo) = self_.trait_or_default::<crate::cargo::Cargo>() else {
                    return Vec::new();
                };
                let max_weight = cargo.info.max_weight;
                let count = if info.pip_count > 0 { info.pip_count } else { max_weight };
                let passengers = cargo.passengers();
                (0..count)
                    .map(|i| {
                        let mut n = i * max_weight / count;
                        for p in &passengers {
                            let Some(pi) = p.info.trait_info::<crate::cargo::PassengerInfo>() else {
                                continue;
                            };
                            if n < pi.weight {
                                return pi
                                    .custom_pip_type
                                    .as_ref()
                                    .and_then(|t| info.custom_pip_sequences.get(t))
                                    .unwrap_or(full)
                                    .clone();
                            }
                            n -= pi.weight;
                        }
                        empty.clone()
                    })
                    .collect()
            }
        }
    }
}

impl IDecoration for WithPipsDecoration {
    fn requires_selection(&self) -> bool {
        self.info.deco.requires_selection
    }

    fn render_decoration(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> Option<Decoration> {
        if !is_active(&self.state, self_) || !self.info.deco.should_render(self_, viewer) {
            return None;
        }
        let sprites = self
            .sequences(self_)
            .into_iter()
            .enumerate()
            .map(|(i, sequence)| DecorationSprite {
                image: self.info.image.clone(),
                sequence,
                frame: 0,
                palette: self.info.palette.clone(),
                index: i as i32,
                stride: self.info.pip_stride,
            })
            .collect();
        self.info.deco.decoration(sprites)
    }
}

/// `Traits/Render/WithSpriteControlGroupDecoration.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithSpriteControlGroupDecorationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub palette: String,
    pub image: String,
    pub group_sequence: String,
    pub position: String,
    pub margin: Int2,
}

impl Default for WithSpriteControlGroupDecorationInfo {
    fn default() -> Self {
        WithSpriteControlGroupDecorationInfo {
            instance_name: None,
            palette: "chrome".into(),
            image: "pips".into(),
            group_sequence: "groups".into(),
            position: "TopLeft".into(),
            margin: Int2::default(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct WithSpriteControlGroupDecoration {
    info: Rc<WithSpriteControlGroupDecorationInfo>,
}

register_traits!(WithSpriteControlGroupDecoration: IDecoration);
info_impl!(WithSpriteControlGroupDecorationInfo => |info, _init| Some(Rc::new(WithSpriteControlGroupDecoration { info })));

impl IDecoration for WithSpriteControlGroupDecoration {
    fn requires_selection(&self) -> bool {
        true
    }

    fn render_decoration(&self, self_: &ActorRef, _viewer: Option<&PlayerRef>) -> Option<Decoration> {
        let w = self_.world();
        let group = crate::selection::ControlGroups::of(&w)?.control_group_for_actor(self_)?;
        Some(Decoration {
            position: self.info.position.clone(),
            margin: self.info.margin,
            sprites: vec![DecorationSprite {
                image: self.info.image.clone(),
                sequence: self.info.group_sequence.clone(),
                frame: group as i32,
                palette: self.info.palette.clone(),
                index: 0,
                stride: Int2::default(),
            }],
            text: None,
        })
    }
}

/// `RangeCircleAnnotationRenderable` or `DetectionCircleAnnotationRenderable`.
#[derive(Clone, Debug)]
pub struct AnnotationCircle {
    pub center: WPos,
    pub radius: WDist,
    pub color: Color,
    pub width: f32,
    pub border_color: Color,
    pub border_width: f32,
    /// Sweep lines of the detection circle: the count, their separation, and the first angle.
    pub trail: Option<(i32, WAngle, WAngle)>,
}

/// `IRenderAnnotationsWhenSelected` and `IRenderAnnotations` of the circle traits.
pub trait IRenderCircles {
    fn circles(&self, self_: &ActorRef, viewer: Option<&PlayerRef>, selected: bool, out: &mut Vec<AnnotationCircle>);
}

/// `Exts.RenderPlayer`-style test: `self.Owner.IsAlliedWith(self.World.RenderPlayer)`.
fn allied_with_viewer(self_: &ActorRef, viewer: Option<&PlayerRef>) -> bool {
    self_.owner().is_allied_with(viewer.map(|v| &**v))
}

macro_rules! circle_defaults {
    ($color:expr) => {
        (
            Color::from_argb(128, $color.0, $color.1, $color.2),
            1.0,
            Color::from_argb(96, 0, 0, 0),
            3.0,
        )
    };
}

/// `Traits/Render/RenderRangeCircle.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct RenderRangeCircleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub range_circle_type: Option<String>,
    pub fallback_range: WDist,
    pub range_circle_mode: String,
    pub color: Color,
    pub width: f32,
    pub border_color: Color,
    pub border_width: f32,
}

impl Default for RenderRangeCircleInfo {
    fn default() -> Self {
        let (color, width, border_color, border_width) = circle_defaults!((255, 255, 0));
        RenderRangeCircleInfo {
            instance_name: None,
            range_circle_type: None,
            fallback_range: WDist::ZERO,
            range_circle_mode: "Maximum".into(),
            color,
            width,
            border_color,
            border_width,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct RenderRangeCircle {
    info: Rc<RenderRangeCircleInfo>,
}

register_traits!(RenderRangeCircle: IRenderCircles);
info_impl!(RenderRangeCircleInfo => |info, _init| Some(Rc::new(RenderRangeCircle { info })));

impl IRenderCircles for RenderRangeCircle {
    fn circles(&self, self_: &ActorRef, viewer: Option<&PlayerRef>, selected: bool, out: &mut Vec<AnnotationCircle>) {
        if !selected || !allied_with_viewer(self_, viewer) {
            return;
        }
        let Some(attack) = self_.trait_or_default::<crate::attack::AttackBase>() else {
            return;
        };
        let radius = if self.info.range_circle_mode == "Minimum" {
            attack.get_minimum_range()
        } else {
            attack.get_maximum_range()
        };
        if radius == WDist::ZERO {
            return;
        }
        out.push(AnnotationCircle {
            center: self_.center_position(),
            radius,
            color: self.info.color,
            width: self.info.width,
            border_color: self.info.border_color,
            border_width: self.info.border_width,
            trail: None,
        });
    }
}

/// `Traits/Render/RenderDetectionCircle.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct RenderDetectionCircleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub update_line_tick: WAngle,
    pub trail_count: i32,
    pub color: Color,
    pub width: f32,
    pub border_color: Color,
    pub border_width: f32,
    pub visible: String,
}

impl Default for RenderDetectionCircleInfo {
    fn default() -> Self {
        let (color, width, border_color, border_width) = circle_defaults!((50, 205, 50));
        RenderDetectionCircleInfo {
            instance_name: None,
            update_line_tick: WAngle::new(-1),
            trail_count: 0,
            color,
            width,
            border_color,
            border_width,
            visible: "WhenSelected".into(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct RenderDetectionCircle {
    info: Rc<RenderDetectionCircleInfo>,
    line_angle: Cell<WAngle>,
}

register_traits!(RenderDetectionCircle: IRenderCircles, ITick);
info_impl!(RenderDetectionCircleInfo => |info, _init| Some(Rc::new(RenderDetectionCircle { info, line_angle: Cell::new(WAngle::ZERO) })));

impl ITick for RenderDetectionCircle {
    fn tick(&self, _self_: &ActorRef) {
        self.line_angle.set(self.line_angle.get() + self.info.update_line_tick);
    }
}

impl IRenderCircles for RenderDetectionCircle {
    fn circles(&self, self_: &ActorRef, viewer: Option<&PlayerRef>, selected: bool, out: &mut Vec<AnnotationCircle>) {
        let wanted = if selected { "WhenSelected" } else { "Always" };
        if self.info.visible != wanted || !allied_with_viewer(self_, viewer) {
            return;
        }
        let radius = self_
            .traits_implementing::<crate::cloak::DetectCloaked>()
            .iter()
            .map(|d| d.info.range)
            .max()
            .unwrap_or(WDist::ZERO);
        if radius == WDist::ZERO {
            return;
        }
        out.push(AnnotationCircle {
            center: self_.center_position(),
            radius,
            color: self.info.color,
            width: self.info.width,
            border_color: self.info.border_color,
            border_width: self.info.border_width,
            trail: Some((self.info.trail_count, self.info.update_line_tick, self.line_angle.get())),
        });
    }
}

/// `Traits/Render/RenderShroudCircle.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct RenderShroudCircleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub color: Color,
    pub width: f32,
    pub border_color: Color,
    pub border_width: f32,
}

impl Default for RenderShroudCircleInfo {
    fn default() -> Self {
        let (color, width, border_color, border_width) = circle_defaults!((0, 255, 255));
        RenderShroudCircleInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            color,
            width,
            border_color,
            border_width,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct RenderShroudCircle {
    info: Rc<RenderShroudCircleInfo>,
    state: ConditionalState,
}

register_traits!(RenderShroudCircle: IRenderCircles, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(RenderShroudCircleInfo => |info, _init| Some(Rc::new(RenderShroudCircle {
    state: ConditionalState::new(&info.base),
    info,
})));
conditional_simple!(RenderShroudCircle);

impl IRenderCircles for RenderShroudCircle {
    fn circles(&self, self_: &ActorRef, viewer: Option<&PlayerRef>, selected: bool, out: &mut Vec<AnnotationCircle>) {
        if !selected || self.state.is_trait_disabled() || !allied_with_viewer(self_, viewer) {
            return;
        }
        let radius = self_
            .info
            .trait_infos::<crate::visibility::CreatesShroudInfo>()
            .iter()
            .map(|c| c.affects.range)
            .max()
            .unwrap_or(WDist::ZERO);
        out.push(AnnotationCircle {
            center: self_.center_position(),
            radius,
            color: self.info.color,
            width: self.info.width,
            border_color: self.info.border_color,
            border_width: self.info.border_width,
            trail: None,
        });
    }
}

/// `Traits/Render/WithRangeCircle.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithRangeCircleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(name = "Type")]
    pub circle_type: Option<String>,
    pub color: Color,
    pub width: f32,
    pub border_color: Color,
    pub border_width: f32,
    pub use_player_color: bool,
    pub valid_relationships: PlayerRelationship,
    pub visible: String,
    pub range: WDist,
}

impl Default for WithRangeCircleInfo {
    fn default() -> Self {
        let (color, width, border_color, border_width) = circle_defaults!((255, 255, 255));
        WithRangeCircleInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            circle_type: None,
            color,
            width,
            border_color,
            border_width,
            use_player_color: false,
            valid_relationships: PlayerRelationship::ALLY,
            visible: "WhenSelected".into(),
            range: WDist::ZERO,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct WithRangeCircle {
    info: Rc<WithRangeCircleInfo>,
    state: ConditionalState,
}

register_traits!(WithRangeCircle: IRenderCircles, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithRangeCircleInfo => |info, _init| Some(Rc::new(WithRangeCircle {
    state: ConditionalState::new(&info.base),
    info,
})));
conditional_simple!(WithRangeCircle);

impl IRenderCircles for WithRangeCircle {
    fn circles(&self, self_: &ActorRef, viewer: Option<&PlayerRef>, selected: bool, out: &mut Vec<AnnotationCircle>) {
        let info = &self.info;
        let wanted = if selected { "WhenSelected" } else { "Always" };
        if info.visible != wanted || self.state.is_trait_disabled() {
            return;
        }
        let visible = viewer.is_none_or(|v| {
            info.valid_relationships.has_relationship(self_.owner().relationship_with(Some(v))) || (v.spectating() && !v.non_combatant)
        });
        if !visible {
            return;
        }
        out.push(AnnotationCircle {
            center: self_.center_position(),
            radius: info.range,
            color: if info.use_player_color { self_.owner().color } else { info.color },
            width: info.width,
            border_color: info.border_color,
            border_width: info.border_width,
            trail: None,
        });
    }
}

/// `Traits/Render/WithTextDecoration.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithTextDecorationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub deco: WithDecorationBaseInfo,
    #[field(require)]
    pub text: String,
    pub font: String,
    pub color: Color,
    pub use_player_color: bool,
}

impl Default for WithTextDecorationInfo {
    fn default() -> Self {
        WithTextDecorationInfo {
            instance_name: None,
            deco: WithDecorationBaseInfo::default(),
            text: String::new(),
            font: "TinyBold".into(),
            color: Color::from_argb(255, 255, 255, 255),
            use_player_color: false,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct WithTextDecoration {
    info: Rc<WithTextDecorationInfo>,
    state: ConditionalState,
}

register_traits!(WithTextDecoration: IDecoration, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithTextDecorationInfo => |info, _init| {
    Some(Rc::new(WithTextDecoration { state: ConditionalState::new(&info.deco.base), info }))
});
conditional_simple!(WithTextDecoration);

impl IDecoration for WithTextDecoration {
    fn requires_selection(&self) -> bool {
        self.info.deco.requires_selection
    }

    fn render_decoration(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> Option<Decoration> {
        if !is_active(&self.state, self_) || !self.info.deco.should_render(self_, viewer) {
            return None;
        }
        self.info.deco.text_decoration(DecorationText {
            font: self.info.font.clone(),
            color: if self.info.use_player_color {
                self_.owner().color
            } else {
                self.info.color
            },
            label: DecorationLabel::Text(self.info.text.clone()),
        })
    }
}

/// `Traits/Render/WithNameTagDecoration.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithNameTagDecorationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub deco: WithDecorationBaseInfo,
    pub max_length: i32,
    pub font: String,
    pub color: Color,
    pub use_player_color: bool,
}

impl Default for WithNameTagDecorationInfo {
    fn default() -> Self {
        WithNameTagDecorationInfo {
            instance_name: None,
            deco: WithDecorationBaseInfo::default(),
            max_length: 10,
            font: "TinyBold".into(),
            color: Color::from_argb(255, 255, 255, 255),
            use_player_color: false,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct WithNameTagDecoration {
    info: Rc<WithNameTagDecorationInfo>,
    state: ConditionalState,
}

register_traits!(WithNameTagDecoration: IDecoration, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithNameTagDecorationInfo => |info, _init| {
    Some(Rc::new(WithNameTagDecoration { state: ConditionalState::new(&info.deco.base), info }))
});
conditional_simple!(WithNameTagDecoration);

impl IDecoration for WithNameTagDecoration {
    fn requires_selection(&self) -> bool {
        self.info.deco.requires_selection
    }

    fn render_decoration(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> Option<Decoration> {
        if !is_active(&self.state, self_) || !self.info.deco.should_render(self_, viewer) {
            return None;
        }
        self.info.deco.text_decoration(DecorationText {
            font: self.info.font.clone(),
            color: if self.info.use_player_color {
                owner_color(self_, viewer)
            } else {
                self.info.color
            },
            label: DecorationLabel::OwnerName {
                max_length: self.info.max_length.max(0) as usize,
            },
        })
    }
}

/// `Exts.OwnerColor`: the color of the disguise owner while disguised and a render player exists.
fn owner_color(self_: &ActorRef, viewer: Option<&PlayerRef>) -> Color {
    if viewer.is_some()
        && let Some(e) = &self_.cache().effective_owner
        && e.disguised()
        && let Some(o) = e.owner()
    {
        return o.color;
    }
    self_.owner().color
}
