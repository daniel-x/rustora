// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Sim relevant parts of render traits: body orientation and sprite image lookup.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::world_types::{WAngle, WRot, WVec};
use rustora_engine::activity::ActivityRef;
use rustora_engine::actor::INVALID_CONDITION_TOKEN;
use rustora_engine::damage::{AttackInfo, DamageState};
use rustora_engine::player::PlayerRef;
use rustora_engine::sequence::{Animation, AnimationWithOffset};
use rustora_engine::{conditional_plumbing, conditional_simple};

use crate::actor_reference::SkipMakeAnimsInit;
use crate::interfaces::{IDockClientBody, INotifyDamageStateChanged, INotifyDeployComplete, INotifyDeployTriggered, INotifyHarvestAction};

use indexmap::IndexMap;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::actor_init::FactionInit;
use rustora_engine::conditional::*;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("RenderSprites", |n| load_info!(RenderSpritesInfo, n));
    r.register("RenderSpritesEditorOnly", |n| {
        let mut info = RenderSpritesInfo {
            editor_only: true,
            instance_name: n.instance_name.clone(),
            ..RenderSpritesInfo::default()
        };
        rustora_core::fieldloader::FieldLoad::load(&mut info, &n.yaml).map_err(|e| format!("Trait name {}: {}", n.display_name(), e))?;
        Ok(Rc::new(info) as Rc<dyn TraitInfo>)
    });
    r.register("WithSpriteBody", |n| load_info!(WithSpriteBodyInfo, n));
    r.register("WithFacingSpriteBody", |n| load_info!(WithFacingSpriteBodyInfo, n));
    r.register("WithResourceLevelSpriteBody", |n| load_info!(WithResourceLevelSpriteBodyInfo, n));
    r.register("WithHarvesterSpriteBody", |n| load_info!(WithHarvesterSpriteBodyInfo, n));
    r.register("WithWallSpriteBody", |n| load_info!(WithWallSpriteBodyInfo, n));
    r.register("WithGateSpriteBody", |n| load_info!(WithGateSpriteBodyInfo, n));
    r.register("WithChargeSpriteBody", |n| load_info!(WithChargeSpriteBodyInfo, n));
    r.register("WithEmbeddedTurretSpriteBody", |n| load_info!(WithEmbeddedTurretSpriteBodyInfo, n));
    r.register("WithMakeAnimation", |n| load_info!(WithMakeAnimationInfo, n));
    r.register("WithBuildingPlacedAnimation", |n| load_info!(WithBuildingPlacedAnimationInfo, n));
    r.register("WithDockingAnimation", |n| load_info!(WithDockingAnimationInfo, n));
    r.register("WithHarvestAnimation", |n| load_info!(WithHarvestAnimationInfo, n));
    r.register("WithIdleOverlay", |n| load_info!(WithIdleOverlayInfo, n));
    r.register("WithProductionDoorOverlay", |n| load_info!(WithProductionDoorOverlayInfo, n));
    r.register("BodyOrientation", |n| load_info!(BodyOrientationInfo, n));
    r.register("ClassicFacingBodyOrientation", |n| {
        let mut info = BodyOrientationInfo::default();
        rustora_core::fieldloader::FieldLoad::load(&mut info, &n.yaml).map_err(|e| format!("Trait name {}: {e}", n.display_name()))?;
        info.instance_name = n.instance_name.clone();
        Ok(Rc::new(ClassicFacingBodyOrientationInfo(info)) as Rc<dyn TraitInfo>)
    });
    r.register("QuantizeFacingsFromSequence", |n| load_info!(QuantizeFacingsFromSequenceInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RenderSpritesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub image: Option<String>,
    pub faction_images: IndexMap<String, String>,
    pub palette: Option<String>,
    pub player_palette: String,
    /// `RenderSpritesEditorOnly`: invisible during games.
    #[field(ignore)]
    pub editor_only: bool,
}

impl Default for RenderSpritesInfo {
    fn default() -> Self {
        RenderSpritesInfo {
            instance_name: None,
            image: None,
            faction_images: IndexMap::new(),
            palette: None,
            player_palette: "player".into(),
            editor_only: false,
        }
    }
}

impl RenderSpritesInfo {
    pub fn get_image(&self, actor: &ActorInfo, faction: &str) -> String {
        if !faction.is_empty()
            && let Some(i) = self.faction_images.get(faction)
        {
            return i.to_lowercase();
        }
        self.image.clone().unwrap_or_else(|| actor.name.clone()).to_lowercase()
    }
}

struct AnimationWrapper {
    animation: AnimationWithOffset,
    palette: Option<String>,
    is_player_palette: bool,
}

/// `RenderSprites`: owns and ticks the animations of an actor.
pub struct RenderSprites {
    pub info: Rc<RenderSpritesInfo>,
    faction: String,
    anims: RefCell<Vec<Rc<AnimationWrapper>>>,
}

register_traits!(RenderSprites: ITick, rustora_engine::render::IRender);
info_impl!(RenderSpritesInfo => |info, init| {
    let faction = init.inits.get::<FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone());
    Some(Rc::new(RenderSprites { info, faction, anims: RefCell::new(Vec::new()) }))
});

impl ITick for RenderSprites {
    fn tick(&self, _self_: &ActorRef) {
        let anims = self.anims.borrow().clone();
        for a in anims {
            a.animation.animation.tick();
        }
    }
}

impl rustora_engine::render::IRender for RenderSprites {
    fn render(&self, self_: &ActorRef, out: &mut Vec<rustora_engine::render::Renderable>) {
        if self.info.editor_only {
            return;
        }
        let owner = self_
            .cache()
            .effective_owner
            .as_ref()
            .filter(|e| e.disguised())
            .and_then(|e| e.owner())
            .unwrap_or_else(|| self_.owner());
        for a in self.anims.borrow().iter() {
            if a.animation.disable_func.as_ref().is_some_and(|f| f()) {
                continue;
            }
            let palette = rustora_engine::render::PaletteRef::resolve(a.palette.as_deref(), a.is_player_palette, &owner);
            rustora_engine::render::render_animation_with_offset(&a.animation, self_, &palette, out);
        }
    }
}

impl RenderSprites {
    pub fn get_image(&self, self_: &ActorRef) -> String {
        self.info.get_image(&self_.info, &self.faction)
    }

    pub fn add(&self, anim: AnimationWithOffset, palette: Option<String>, is_player_palette: bool) {
        let (palette, is_player_palette) = match palette {
            Some(p) => (Some(p), is_player_palette),
            None => (
                self.info.palette.clone().or_else(|| Some(self.info.player_palette.clone())),
                self.info.palette.is_none(),
            ),
        };
        self.anims.borrow_mut().push(Rc::new(AnimationWrapper {
            animation: anim,
            palette,
            is_player_palette,
        }));
    }

    pub fn remove(&self, anim: &Rc<Animation>) {
        self.anims.borrow_mut().retain(|a| !Rc::ptr_eq(&a.animation.animation, anim));
    }

    /// Animations with their palette, for the renderer.
    pub fn animations(&self) -> Vec<(AnimationWithOffset, Option<String>, bool)> {
        self.anims
            .borrow()
            .iter()
            .map(|a| (a.animation.clone(), a.palette.clone(), a.is_player_palette))
            .collect()
    }
}

const DAMAGE_PREFIXES: [(DamageState, &str); 4] = [
    (DamageState::CRITICAL, "critical-"),
    (DamageState::HEAVY, "damaged-"),
    (DamageState::MEDIUM, "scratched-"),
    (DamageState::LIGHT, "scuffed-"),
];

pub fn unnormalize_sequence(sequence: &str) -> &str {
    for (_, p) in DAMAGE_PREFIXES {
        if let Some(s) = sequence.strip_prefix(p) {
            return s;
        }
    }
    sequence
}

pub fn normalize_sequence(anim: &Animation, state: DamageState, sequence: &str) -> String {
    let sequence = unnormalize_sequence(sequence);
    for (d, p) in DAMAGE_PREFIXES {
        let name = format!("{p}{sequence}");
        if state >= d && anim.has_sequence(&name) {
            return name;
        }
    }
    sequence.to_string()
}

/// `Actor.GetDamageState`.
pub fn get_damage_state(a: &ActorRef) -> DamageState {
    a.trait_or_default::<dyn IHealth>()
        .map_or(DamageState::UNDAMAGED, |h| h.damage_state())
}

pub fn make_facing_func(a: &ActorRef) -> Rc<dyn Fn() -> WAngle> {
    match a.trait_or_default::<dyn IFacing>() {
        Some(f) => Rc::new(move || f.facing()),
        None => Rc::new(|| WAngle::ZERO),
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct WithSpriteBodyInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub start_sequence: Option<String>,
    pub sequence: String,
    pub name: String,
    pub force_to_ground: bool,
    pub palette: Option<String>,
    pub is_player_palette: bool,
}

impl Default for WithSpriteBodyInfo {
    fn default() -> Self {
        WithSpriteBodyInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            start_sequence: None,
            sequence: "idle".into(),
            name: "body".into(),
            force_to_ground: false,
            palette: None,
            is_player_palette: false,
        }
    }
}

/// `WithFacingSpriteBodyInfo`: the body animation follows the actor facing.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct WithFacingSpriteBodyInfo {
    #[field(flatten)]
    pub base: WithSpriteBodyInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
}

enum BodyVariant {
    Plain,
    Wall {
        ty: String,
        adjacent: Rc<Cell<i32>>,
        dirty: Cell<bool>,
    },
    ResourceLevel {
        stages: i32,
        player_resources: RefCell<Rc<crate::player::PlayerResources>>,
    },
    Harvester {
        image_by_fullness: Vec<String>,
        harv: Rc<crate::harvester::Harvester>,
    },
    /// `WithGateSpriteBody`: the frame follows the gate position, walls connect at `wall_connections`.
    Gate {
        ty: String,
        wall_connections: Vec<rustora_core::world_types::CVec>,
        open_sequence: Option<String>,
        gate: Rc<crate::gate::Gate>,
        render_open: Cell<bool>,
    },
    /// `WithChargeSpriteBody`: the frame follows the charge level of `AttackCharges`.
    Charge {
        attack: Rc<crate::attack::AttackBase>,
    },
    /// `WithEmbeddedTurretSpriteBody`: the body faces like the first turret and sets its quantized facings.
    EmbeddedTurret {
        quantized_facings: i32,
        turreted: Rc<crate::turreted::Turreted>,
    },
}

pub struct WithSpriteBody {
    pub info: Rc<WithSpriteBodyInfo>,
    class_name: &'static str,
    variant: BodyVariant,
    state: ConditionalState,
    self_: ActorRef,
    pub default_animation: Rc<Animation>,
}

impl TraitObject for WithSpriteBody {
    fn register(self: Rc<Self>, dict: &rustora_engine::trait_dict::TraitDict, actor: &ActorRef) {
        dict.add::<WithSpriteBody>(actor, self.clone());
        dict.add::<dyn INotifyDamageStateChanged>(actor, self.clone());
        dict.add::<dyn INotifyCreated>(actor, self.clone());
        dict.add::<dyn IObservesVariables>(actor, self.clone());
        dict.add::<dyn IDisabledTrait>(actor, self.clone());
        if let BodyVariant::Harvester { .. } = self.variant {
            dict.add::<dyn ITick>(actor, self.clone());
        }
        if let BodyVariant::ResourceLevel { .. } = self.variant {
            dict.add::<dyn INotifyOwnerChanged>(actor, self.clone());
        }
        if let BodyVariant::Wall { .. } | BodyVariant::Gate { .. } = self.variant {
            dict.add::<dyn ITick>(actor, self.clone());
            dict.add::<dyn INotifyRemovedFromWorld>(actor, self.clone());
        }
        dict.add::<dyn ISync>(actor, self);
    }
    fn type_name(&self) -> &'static str {
        self.class_name
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn new_sprite_body(
    info: Rc<WithSpriteBodyInfo>,
    init: &rustora_engine::actor_init::ActorInitializer,
    class_name: &'static str,
    base_facing: Option<Rc<dyn Fn() -> WAngle>>,
    variant: BodyVariant,
) -> Rc<WithSpriteBody> {
    let self_ = init.self_.clone();
    let rs = self_.trait_::<RenderSprites>();
    let w = init.world();
    let state = ConditionalState::new_pausable(&info.base);
    let seqs = w.map().sequences().clone();
    let image = rs.get_image(&self_);
    let body = Rc::new_cyclic(|me: &std::rc::Weak<WithSpriteBody>| {
        let (m, a) = (me.clone(), self_.clone());
        let paused: Rc<dyn Fn() -> bool> = Rc::new(move || {
            let Some(b) = m.upgrade() else { return false };
            b.state.is_trait_paused()
                && b.default_animation
                    .current_sequence()
                    .is_some_and(|s| s.name == b.normalize_sequence(&a, &b.info.sequence))
        });
        WithSpriteBody {
            class_name,
            variant,
            state,
            default_animation: Animation::new(seqs, &image, base_facing, Some(paused)),
            self_: self_.clone(),
            info: info.clone(),
        }
    });
    let offset_func: Option<Rc<dyn Fn() -> WVec>> = if info.force_to_ground {
        let (a, w2) = (self_.clone(), Rc::downgrade(&w));
        Some(Rc::new(move || {
            WVec::new(
                0,
                0,
                -w2.upgrade()
                    .expect("world")
                    .map()
                    .distance_above_terrain(a.center_position())
                    .length,
            )
        }))
    } else {
        None
    };
    let weak = Rc::downgrade(&body);
    rs.add(
        AnimationWithOffset {
            animation: body.default_animation.clone(),
            offset_func,
            disable_func: Some(Rc::new(move || weak.upgrade().is_some_and(|b| b.state.is_trait_disabled()))),
            zoffset_func: None,
        },
        info.palette.clone(),
        info.is_player_palette,
    );
    body
}

info_impl!(WithSpriteBodyInfo => |info, init| Some(new_sprite_body(info, init, "WithSpriteBody", None, BodyVariant::Plain)));
/// `WithWallSpriteBodyInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithWallSpriteBodyInfo {
    #[field(flatten)]
    pub base: WithSpriteBodyInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(name = "Type")]
    pub wall_type: String,
}

impl Default for WithWallSpriteBodyInfo {
    fn default() -> Self {
        WithWallSpriteBodyInfo {
            base: WithSpriteBodyInfo::default(),
            instance_name: None,
            wall_type: "wall".into(),
        }
    }
}

info_impl!(WithWallSpriteBodyInfo => |info, init| {
    let v = BodyVariant::Wall { ty: info.wall_type.clone(), adjacent: Rc::new(Cell::new(0)), dirty: Cell::new(true) };
    Some(new_sprite_body(Rc::new(info.base.clone()), init, "WithWallSpriteBody", None, v))
});

/// `WithChargeSpriteBodyInfo`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct WithChargeSpriteBodyInfo {
    #[field(flatten)]
    pub base: WithSpriteBodyInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
}

info_impl!(WithChargeSpriteBodyInfo => |info, init| {
    let attack = init
        .self_
        .traits_implementing::<crate::attack::AttackBase>()
        .into_iter()
        .find(|a| a.type_name() == "AttackCharges")
        .expect("WithChargeSpriteBody requires AttackCharges");
    let body = new_sprite_body(Rc::new(info.base.clone()), init, "WithChargeSpriteBody", None, BodyVariant::Charge { attack });
    body.configure_animation(&init.self_);
    Some(body)
});

/// `WithGateSpriteBodyInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithGateSpriteBodyInfo {
    #[field(flatten)]
    pub base: WithSpriteBodyInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub wall_connections: Vec<rustora_core::world_types::CVec>,
    #[field(name = "Type")]
    pub wall_type: String,
    pub open_sequence: Option<String>,
}

impl Default for WithGateSpriteBodyInfo {
    fn default() -> Self {
        WithGateSpriteBodyInfo {
            base: WithSpriteBodyInfo::default(),
            instance_name: None,
            wall_connections: Vec::new(),
            wall_type: "wall".into(),
            open_sequence: None,
        }
    }
}

info_impl!(WithGateSpriteBodyInfo => |info, init| {
    let v = BodyVariant::Gate {
        ty: info.wall_type.clone(),
        wall_connections: info.wall_connections.clone(),
        open_sequence: info.open_sequence.clone(),
        gate: init.self_.trait_::<crate::gate::Gate>(),
        render_open: Cell::new(false),
    };
    Some(new_sprite_body(Rc::new(info.base.clone()), init, "WithGateSpriteBody", None, v))
});

/// `WithEmbeddedTurretSpriteBodyInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithEmbeddedTurretSpriteBodyInfo {
    #[field(flatten)]
    pub base: WithSpriteBodyInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub quantized_facings: i32,
}

impl Default for WithEmbeddedTurretSpriteBodyInfo {
    fn default() -> Self {
        WithEmbeddedTurretSpriteBodyInfo {
            base: WithSpriteBodyInfo::default(),
            instance_name: None,
            quantized_facings: -1,
        }
    }
}

info_impl!(WithEmbeddedTurretSpriteBodyInfo => |info, init| {
    let turreted = init
        .self_
        .traits_implementing::<crate::turreted::Turreted>()
        .into_iter()
        .next()
        .expect("WithEmbeddedTurretSpriteBody requires Turreted");
    let t = turreted.clone();
    let facing: Rc<dyn Fn() -> WAngle> = Rc::new(move || t.world_orientation().yaw);
    let v = BodyVariant::EmbeddedTurret { quantized_facings: info.quantized_facings, turreted };
    Some(new_sprite_body(Rc::new(info.base.clone()), init, "WithEmbeddedTurretSpriteBody", Some(facing), v))
});

impl WithSpriteBody {
    fn update_turret_facings(&self) {
        if let BodyVariant::EmbeddedTurret {
            quantized_facings,
            turreted,
        } = &self.variant
        {
            let facings = if *quantized_facings >= 0 {
                *quantized_facings
            } else {
                self.default_animation.current_sequence().map_or(0, |s| s.facings_or_interpolated())
            };
            turreted.quantized_facings.set(facings);
        }
    }
}

info_impl!(WithFacingSpriteBodyInfo => |info, init| {
    let facing = make_facing_func(&init.self_);
    Some(new_sprite_body(Rc::new(info.base.clone()), init, "WithFacingSpriteBody", Some(facing), BodyVariant::Plain))
});

#[derive(Clone, Debug, FieldLoad)]
pub struct WithResourceLevelSpriteBodyInfo {
    #[field(flatten)]
    pub base: WithSpriteBodyInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub stages: i32,
}

impl Default for WithResourceLevelSpriteBodyInfo {
    fn default() -> Self {
        WithResourceLevelSpriteBodyInfo {
            base: WithSpriteBodyInfo::default(),
            instance_name: None,
            stages: 10,
        }
    }
}

info_impl!(WithResourceLevelSpriteBodyInfo => |info, init| {
    let v = BodyVariant::ResourceLevel {
        stages: info.stages,
        player_resources: RefCell::new(init.self_.owner().player_actor().trait_::<crate::player::PlayerResources>()),
    };
    Some(new_sprite_body(Rc::new(info.base.clone()), init, "WithResourceLevelSpriteBody", None, v))
});

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct WithHarvesterSpriteBodyInfo {
    #[field(flatten)]
    pub base: WithSpriteBodyInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub image_by_fullness: Vec<String>,
}

info_impl!(WithHarvesterSpriteBodyInfo => |info, init| {
    let facing = make_facing_func(&init.self_);
    let v = BodyVariant::Harvester { image_by_fullness: info.image_by_fullness.clone(), harv: init.self_.trait_::<crate::harvester::Harvester>() };
    Some(new_sprite_body(Rc::new(info.base.clone()), init, "WithHarvesterSpriteBody", Some(facing), v))
});

impl ITick for WithSpriteBody {
    fn tick(&self, _self_: &ActorRef) {
        self.tick_wall(_self_);
        self.tick_gate(_self_);
        let BodyVariant::Harvester { image_by_fullness, harv } = &self.variant else {
            return;
        };
        if image_by_fullness.is_empty() {
            return;
        }
        let desired = harv.fullness() * (image_by_fullness.len() as i32 - 1) / 100;
        let cur = self.default_animation.current_sequence().expect("body sequence").name.clone();
        self.default_animation.change_image(&image_by_fullness[desired as usize], &cur);
    }
}

impl INotifyOwnerChanged for WithSpriteBody {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        if let BodyVariant::ResourceLevel { player_resources, .. } = &self.variant {
            *player_resources.borrow_mut() = new_owner.player_actor().trait_::<crate::player::PlayerResources>();
        }
    }
}

impl ConditionalHooks for WithSpriteBody {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        if let BodyVariant::Charge { .. } = self.variant {
            return;
        }
        self.base_trait_enabled(self_);
        self.update_turret_facings();
        if let BodyVariant::ResourceLevel { .. } = self.variant {
            self.configure_animation(self_);
        }
        if let BodyVariant::Wall { dirty, .. } = &self.variant {
            dirty.set(true);
            self.play_wall_animation(self_);
            update_wall_neighbours(self_);
            let anim = self.default_animation.clone();
            self_.world().add_frame_end_task(Box::new(move |_| anim.tick()));
        }
        if let BodyVariant::Gate { .. } = self.variant {
            self.update_gate_state(self_);
            self.update_gate_neighbours(self_);
        }
    }
}

conditional_plumbing!(WithSpriteBody);

const WALL_DIRECTIONS: [(i32, i32); 8] = [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)];

fn actors_around(self_: &ActorRef) -> Vec<ActorRef> {
    use crate::actor_map::WorldExt;
    let w = self_.world();
    let map = w.actor_map();
    WALL_DIRECTIONS
        .iter()
        .flat_map(|&(x, y)| map.get_actors_at(self_.location() + rustora_core::world_types::CVec::new(x, y)))
        .collect()
}

/// `WithWallSpriteBody.UpdateNeighbours`.
fn update_wall_neighbours(self_: &ActorRef) {
    for a in actors_around(self_) {
        for b in a.traits_implementing::<WithSpriteBody>() {
            if let BodyVariant::Wall { dirty, .. } = &b.variant {
                dirty.set(true);
            }
        }
    }
}

impl INotifyRemovedFromWorld for WithSpriteBody {
    fn removed_from_world(&self, self_: &ActorRef) {
        if let BodyVariant::Gate { .. } = self.variant {
            self.update_gate_neighbours(self_);
        } else {
            update_wall_neighbours(self_);
        }
    }
}

impl WithSpriteBody {
    /// `WithGateSpriteBody.UpdateState`.
    fn update_gate_state(&self, self_: &ActorRef) {
        let BodyVariant::Gate {
            open_sequence,
            gate,
            render_open,
            ..
        } = &self.variant
        else {
            return;
        };
        if let Some(open) = open_sequence
            && (render_open.get() || self.state.is_trait_paused())
        {
            self.default_animation.play_repeating(&self.normalize_sequence(self_, open));
            return;
        }
        let (anim, gate) = (self.default_animation.clone(), gate.clone());
        self.default_animation.play_fetch_index(
            &self.normalize_sequence(self_, &self.info.sequence),
            Rc::new(move || {
                let len = anim.current_sequence().map_or(1, |s| s.length());
                (len - 1) * gate.position() / gate.open_position()
            }),
        );
    }

    /// `WithGateSpriteBody.ITick.Tick`.
    fn tick_gate(&self, self_: &ActorRef) {
        let BodyVariant::Gate {
            open_sequence,
            gate,
            render_open,
            ..
        } = &self.variant
        else {
            return;
        };
        if open_sequence.is_none() {
            return;
        }
        let open = gate.position() == gate.open_position();
        if open != render_open.get() {
            render_open.set(open);
            self.update_gate_state(self_);
        }
    }

    /// `WithGateSpriteBody.UpdateNeighbours`: marks the walls around the footprint dirty.
    fn update_gate_neighbours(&self, self_: &ActorRef) {
        use crate::actor_map::WorldExt;
        let BodyVariant::Gate { gate, .. } = &self.variant else { return };
        let w = self_.world();
        let footprint = gate.footprint();
        let adjacent = crate::util::expand_footprint(footprint, true)
            .into_iter()
            .filter(|c| !footprint.contains(c) && w.map().contains(*c));
        for c in adjacent {
            for a in w.actor_map().get_actors_at(c) {
                for b in a.traits_implementing::<WithSpriteBody>() {
                    if let BodyVariant::Wall { dirty, .. } = &b.variant {
                        dirty.set(true);
                    }
                }
            }
        }
    }
}

impl WithSpriteBody {
    fn play_wall_animation(&self, self_: &ActorRef) {
        if let BodyVariant::Wall { adjacent, .. } = &self.variant {
            let adjacent = adjacent.clone();
            self.default_animation.play_fetch_index(
                &self.normalize_sequence(self_, &self.info.sequence),
                Rc::new(move || adjacent.get()),
            );
        }
    }

    /// `WithWallSpriteBody.ITick.Tick`.
    fn tick_wall(&self, self_: &ActorRef) {
        let BodyVariant::Wall { ty, adjacent, dirty } = &self.variant else {
            return;
        };
        if !dirty.get() {
            return;
        }
        let mut bits = 0;
        for a in actors_around(self_) {
            let connector = a
                .traits_implementing::<WithSpriteBody>()
                .into_iter()
                .find(|b| !b.is_trait_disabled());
            let Some(b) = connector else { continue };
            let facing = self_.location() - a.location();
            let connects = match &b.variant {
                BodyVariant::Wall { ty: other, .. } => other == ty && facing.x.abs() + facing.y.abs() == 1,
                BodyVariant::Gate {
                    ty: other,
                    wall_connections,
                    ..
                } => other == ty && wall_connections.contains(&facing),
                _ => false,
            };
            if !connects {
                continue;
            }
            if facing.y > 0 {
                bits |= 1;
            } else if facing.x < 0 {
                bits |= 2;
            } else if facing.y < 0 {
                bits |= 4;
            } else if facing.x > 0 {
                bits |= 8;
            }
        }
        adjacent.set(bits);
        dirty.set(false);
    }

    fn configure_animation(&self, self_: &ActorRef) {
        if let BodyVariant::Charge { attack } = &self.variant {
            let (anim, attack) = (self.default_animation.clone(), attack.clone());
            let a2 = anim.clone();
            anim.play_fetch_index(
                &self.normalize_sequence(self_, &self.info.sequence),
                Rc::new(move || {
                    let len = a2.current_sequence().map_or(0, |s| s.length());
                    len * attack.charge_level() / (attack.info.charge_level + 1)
                }),
            );
            return;
        }
        let BodyVariant::ResourceLevel { stages, player_resources } = &self.variant else {
            return;
        };
        let (anim, stages, pr) = (self.default_animation.clone(), *stages, player_resources.borrow().clone());
        let a2 = anim.clone();
        anim.play_fetch_index(
            &self.normalize_sequence(self_, &self.info.sequence),
            Rc::new(move || {
                let cap = pr.resource_capacity.get();
                if cap != 0 {
                    (stages.wrapping_mul(a2.current_sequence().expect("sequence").length()) - 1).wrapping_mul(pr.resources.get())
                        / (stages.wrapping_mul(cap))
                } else {
                    0
                }
            }),
        );
    }

    fn base_trait_enabled(&self, self_: &ActorRef) {
        match &self.info.start_sequence {
            Some(start) => {
                let (a, anim, seq, me) = (
                    self_.clone(),
                    self.default_animation.clone(),
                    self.info.sequence.clone(),
                    self.self_.clone(),
                );
                let _ = me;
                let body_seq = move || normalize_sequence(&anim, get_damage_state(&a), &seq);
                let anim2 = self.default_animation.clone();
                self.play_custom_animation(self_, start, Some(Rc::new(move || anim2.play_repeating(&body_seq()))));
            }
            None => self
                .default_animation
                .play_repeating(&self.normalize_sequence(self_, &self.info.sequence)),
        }
    }
}

impl INotifyCreated for WithSpriteBody {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl INotifyDamageStateChanged for WithSpriteBody {
    fn damage_state_changed(&self, self_: &ActorRef, _e: &AttackInfo) {
        if let BodyVariant::Gate { .. } = self.variant {
            self.update_gate_state(self_);
            return;
        }
        if let BodyVariant::Wall { .. } = self.variant {
            if !self.is_trait_disabled() {
                self.play_wall_animation(self_);
            }
            return;
        }
        if let Some(s) = self.default_animation.current_sequence() {
            self.default_animation.replace_anim(&self.normalize_sequence(self_, &s.name));
        }
        self.update_turret_facings();
    }
}

impl WithSpriteBody {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn normalize_sequence(&self, self_: &ActorRef, sequence: &str) -> String {
        normalize_sequence(&self.default_animation, get_damage_state(self_), sequence)
    }

    fn after_custom(&self, self_: &ActorRef, after: Option<Rc<dyn Fn()>>) -> Rc<dyn Fn()> {
        let a = self_.clone();
        let me = a
            .traits_implementing::<WithSpriteBody>()
            .into_iter()
            .find(|w| std::ptr::eq(Rc::as_ptr(w), self))
            .expect("body registered");
        Rc::new(move || {
            me.cancel_custom_animation(&a);
            if let Some(f) = &after {
                f();
            }
        })
    }

    pub fn play_custom_animation(&self, self_: &ActorRef, name: &str, after: Option<Rc<dyn Fn()>>) {
        self.default_animation
            .play_then(&self.normalize_sequence(self_, name), Some(self.after_custom(self_, after)));
    }

    pub fn play_custom_animation_repeating(&self, self_: &ActorRef, name: &str) {
        self.default_animation.play_repeating(&self.normalize_sequence(self_, name));
    }

    pub fn play_custom_animation_backwards(&self, self_: &ActorRef, name: &str, after: Option<Rc<dyn Fn()>>) {
        self.default_animation
            .play_backwards_then(&self.normalize_sequence(self_, name), Some(self.after_custom(self_, after)));
    }

    pub fn cancel_custom_animation(&self, self_: &ActorRef) {
        if let BodyVariant::ResourceLevel { .. } | BodyVariant::Charge { .. } = self.variant {
            self.configure_animation(self_);
            return;
        }
        self.default_animation
            .play_repeating(&self.normalize_sequence(self_, &self.info.sequence));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct WithMakeAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub sequence: String,
    pub condition: Option<String>,
    pub body_names: Vec<String>,
}

impl Default for WithMakeAnimationInfo {
    fn default() -> Self {
        WithMakeAnimationInfo {
            instance_name: None,
            sequence: "make".into(),
            condition: None,
            body_names: vec!["body".into()],
        }
    }
}

pub struct WithMakeAnimation {
    info: Rc<WithMakeAnimationInfo>,
    wsbs: Vec<Rc<WithSpriteBody>>,
    skip_make_animation: bool,
    token: Rc<Cell<i32>>,
}

register_traits!(WithMakeAnimation: INotifyCreated, INotifyDeployTriggered);
info_impl!(WithMakeAnimationInfo => |info, init| {
    let wsbs = init.self_.traits_implementing::<WithSpriteBody>().into_iter().filter(|w| info.body_names.contains(&w.info.name)).collect();
    Some(Rc::new(WithMakeAnimation {
        skip_make_animation: init.inits.contains::<SkipMakeAnimsInit>(),
        info,
        wsbs,
        token: Rc::new(Cell::new(INVALID_CONDITION_TOKEN)),
    }))
});

impl INotifyCreated for WithMakeAnimation {
    fn created(&self, self_: &ActorRef) {
        if !self.skip_make_animation {
            self.forward(self_, Rc::new(|| {}));
        }
    }
}

impl WithMakeAnimation {
    fn first_enabled(&self) -> Option<Rc<WithSpriteBody>> {
        self.wsbs.iter().find(|w| !w.is_trait_disabled()).cloned()
    }

    fn grant(&self, self_: &ActorRef) {
        if self.token.get() == INVALID_CONDITION_TOKEN {
            self.token.set(self_.grant_condition(self.info.condition.as_deref().unwrap_or("")));
        }
    }

    fn completion(&self, self_: &ActorRef, on_complete: Rc<dyn Fn()>) -> Rc<dyn Fn()> {
        let (a, token) = (self_.clone(), self.token.clone());
        Rc::new(move || {
            let (a2, token, on_complete) = (a.clone(), token.clone(), on_complete.clone());
            a.world().add_frame_end_task(Box::new(move |_| {
                if token.get() != INVALID_CONDITION_TOKEN {
                    token.set(a2.revoke_condition(token.get()));
                }
                on_complete();
            }));
        })
    }

    pub fn forward(&self, self_: &ActorRef, on_complete: Rc<dyn Fn()>) {
        self.grant(self_);
        let Some(wsb) = self.first_enabled() else { return };
        wsb.play_custom_animation(self_, &self.info.sequence, Some(self.completion(self_, on_complete)));
    }

    pub fn reverse(&self, self_: &ActorRef, on_complete: Rc<dyn Fn()>) {
        self.grant(self_);
        let Some(wsb) = self.first_enabled() else { return };
        wsb.play_custom_animation_backwards(self_, &self.info.sequence, Some(self.completion(self_, on_complete)));
    }

    /// `Reverse(self, activity, queued)`.
    pub fn reverse_then_activity(&self, self_: &ActorRef, activity: ActivityRef, queued: bool) {
        let (a, wsb, seq, token, cond) = (
            self_.clone(),
            self.first_enabled(),
            self.info.sequence.clone(),
            self.token.clone(),
            self.info.condition.clone(),
        );
        self.reverse(
            self_,
            Rc::new(move || {
                if let Some(w) = &wsb {
                    w.default_animation.play_fetch_index(&seq, Rc::new(|| 0));
                }
                token.set(a.grant_condition(cond.as_deref().unwrap_or("")));
                a.queue_activity_queued(queued, activity.clone());
            }),
        );
    }
}

impl INotifyDeployTriggered for WithMakeAnimation {
    fn deploy(&self, self_: &ActorRef, skip_make_anim: bool) {
        let notify = self_.traits_implementing::<dyn INotifyDeployComplete>();
        if skip_make_anim {
            for n in &notify {
                n.finished_deploy(self_);
            }
            return;
        }
        let notified = Rc::new(Cell::new(false));
        for wsb in &self.wsbs {
            if wsb.is_trait_disabled() {
                continue;
            }
            let (a, notified, notify) = (self_.clone(), notified.clone(), notify.clone());
            wsb.play_custom_animation(
                self_,
                &self.info.sequence,
                Some(Rc::new(move || {
                    if notified.get() {
                        return;
                    }
                    for n in &notify {
                        n.finished_deploy(&a);
                        notified.set(true);
                    }
                })),
            );
        }
    }

    fn undeploy(&self, self_: &ActorRef, skip_make_anim: bool) {
        let notify = self_.traits_implementing::<dyn INotifyDeployComplete>();
        if skip_make_anim {
            for n in &notify {
                n.finished_undeploy(self_);
            }
            return;
        }
        let notified = Rc::new(Cell::new(false));
        for wsb in &self.wsbs {
            if wsb.is_trait_disabled() {
                continue;
            }
            let (a, notified, notify) = (self_.clone(), notified.clone(), notify.clone());
            wsb.play_custom_animation_backwards(
                self_,
                &self.info.sequence,
                Some(Rc::new(move || {
                    if notified.get() {
                        return;
                    }
                    for n in &notify {
                        n.finished_undeploy(&a);
                        notified.set(true);
                    }
                })),
            );
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct WithBuildingPlacedAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub sequence: String,
    pub body: String,
}

impl Default for WithBuildingPlacedAnimationInfo {
    fn default() -> Self {
        WithBuildingPlacedAnimationInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            sequence: "build".into(),
            body: "body".into(),
        }
    }
}

fn body_named(self_: &ActorRef, name: &str) -> Rc<WithSpriteBody> {
    self_
        .traits_implementing::<WithSpriteBody>()
        .into_iter()
        .find(|w| w.info.name == name)
        .unwrap_or_else(|| panic!("{self_:?} has no sprite body named {name}"))
}

pub struct WithBuildingPlacedAnimation {
    info: Rc<WithBuildingPlacedAnimationInfo>,
    state: ConditionalState,
    wsb: Rc<WithSpriteBody>,
}

register_traits!(WithBuildingPlacedAnimation: crate::interfaces::INotifyBuildingPlaced, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithBuildingPlacedAnimationInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    let wsb = body_named(&init.self_, &info.body);
    Some(Rc::new(WithBuildingPlacedAnimation { info, state, wsb }))
});

impl ConditionalHooks for WithBuildingPlacedAnimation {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        self.wsb.cancel_custom_animation(self_);
    }
}

conditional_plumbing!(WithBuildingPlacedAnimation);

impl INotifyCreated for WithBuildingPlacedAnimation {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl crate::interfaces::INotifyBuildingPlaced for WithBuildingPlacedAnimation {
    fn building_placed(&self, self_: &ActorRef) {
        if !self.state.is_trait_disabled() {
            self.wsb.play_custom_animation(self_, &self.info.sequence, None);
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct WithDockingAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub dock_sequence: String,
    pub dock_loop_sequence: String,
}

impl Default for WithDockingAnimationInfo {
    fn default() -> Self {
        WithDockingAnimationInfo {
            instance_name: None,
            dock_sequence: "dock".into(),
            dock_loop_sequence: "dock-loop".into(),
        }
    }
}

pub struct WithDockingAnimation {
    info: Rc<WithDockingAnimationInfo>,
    wsb: Rc<WithSpriteBody>,
}

register_traits!(WithDockingAnimation: IDockClientBody);
info_impl!(WithDockingAnimationInfo => |info, init| {
    let wsb = init.self_.trait_::<WithSpriteBody>();
    Some(Rc::new(WithDockingAnimation { info, wsb }))
});

impl IDockClientBody for WithDockingAnimation {
    fn play_dock_animation(&self, self_: &ActorRef, after: Rc<dyn Fn()>) {
        let (w, a, l) = (self.wsb.clone(), self_.clone(), self.info.dock_loop_sequence.clone());
        self.wsb.play_custom_animation(
            self_,
            &self.info.dock_sequence,
            Some(Rc::new(move || {
                w.play_custom_animation_repeating(&a, &l);
                after();
            })),
        );
    }

    fn play_reverse_dock_animation(&self, self_: &ActorRef, after: Rc<dyn Fn()>) {
        self.wsb
            .play_custom_animation_backwards(self_, &self.info.dock_sequence, Some(after));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct WithHarvestAnimationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub harvest_sequence: String,
    pub body: String,
}

impl Default for WithHarvestAnimationInfo {
    fn default() -> Self {
        WithHarvestAnimationInfo {
            instance_name: None,
            harvest_sequence: "harvest".into(),
            body: "body".into(),
        }
    }
}

pub struct WithHarvestAnimation {
    info: Rc<WithHarvestAnimationInfo>,
    wsb: Rc<WithSpriteBody>,
}

register_traits!(WithHarvestAnimation: INotifyHarvestAction);
info_impl!(WithHarvestAnimationInfo => |info, init| {
    let wsb = body_named(&init.self_, &info.body);
    Some(Rc::new(WithHarvestAnimation { info, wsb }))
});

impl INotifyHarvestAction for WithHarvestAnimation {
    fn harvested(&self, self_: &ActorRef, _resource_type: &str) {
        let sequence = self.wsb.normalize_sequence(self_, &self.info.harvest_sequence);
        let anim = &self.wsb.default_animation;
        if anim.has_sequence(&sequence) && anim.current_sequence().is_none_or(|s| s.name != sequence) {
            self.wsb.play_custom_animation(self_, &sequence, None);
        }
    }

    fn moving_to_resources(&self, _self_: &ActorRef, _target_cell: rustora_core::world_types::CPos) {}

    fn movement_cancelled(&self, _self_: &ActorRef) {}
}

#[derive(Clone, Debug, FieldLoad)]
pub struct WithIdleOverlayInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub image: Option<String>,
    pub start_sequence: Option<String>,
    pub sequence: String,
    pub offset: WVec,
    pub palette: Option<String>,
    pub is_player_palette: bool,
    pub is_decoration: bool,
}

impl Default for WithIdleOverlayInfo {
    fn default() -> Self {
        WithIdleOverlayInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            image: None,
            start_sequence: None,
            sequence: "idle-overlay".into(),
            offset: WVec::ZERO,
            palette: None,
            is_player_palette: false,
            is_decoration: false,
        }
    }
}

pub struct WithIdleOverlay {
    state: ConditionalState,
    overlay: Rc<Animation>,
}

register_traits!(WithIdleOverlay: INotifyDamageStateChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithIdleOverlayInfo => |info, init| {
    let self_ = init.self_.clone();
    let w = init.world();
    let rs = self_.trait_::<RenderSprites>();
    let body = self_.trait_::<BodyOrientation>();
    let facing = self_.trait_or_default::<dyn IFacing>();
    let image = info.image.clone().unwrap_or_else(|| rs.get_image(&self_));
    let state = ConditionalState::new_pausable(&info.base);
    let facing_func: Option<Rc<dyn Fn() -> WAngle>> = facing.map(|f| {
        let b = body.clone();
        Rc::new(move || b.quantize_facing(f.facing())) as Rc<dyn Fn() -> WAngle>
    });
    let t = Rc::new_cyclic(|me: &std::rc::Weak<WithIdleOverlay>| {
        let m = me.clone();
        let paused: Rc<dyn Fn() -> bool> = Rc::new(move || m.upgrade().is_some_and(|t| t.state.is_trait_paused()));
        WithIdleOverlay { state, overlay: Animation::new(w.map().sequences().clone(), &image, facing_func, Some(paused)) }
    });
    t.overlay.is_decoration.set(info.is_decoration);
    let ds = get_damage_state(&self_);
    match &info.start_sequence {
        Some(start) => {
            let (o, a, seq) = (t.overlay.clone(), self_.clone(), info.sequence.clone());
            t.overlay.play_then(
                &normalize_sequence(&t.overlay, ds, start),
                Some(Rc::new(move || o.play_repeating(&normalize_sequence(&o, get_damage_state(&a), &seq)))),
            );
        }
        None => t.overlay.play_repeating(&normalize_sequence(&t.overlay, ds, &info.sequence)),
    }
    let (b, a, off) = (body.clone(), self_.clone(), info.offset);
    let weak = Rc::downgrade(&t);
    rs.add(
        AnimationWithOffset {
            animation: t.overlay.clone(),
            offset_func: Some(Rc::new(move || b.local_to_world(off.rotate(&b.quantize_orientation(a.orientation()))))),
            disable_func: Some(Rc::new(move || weak.upgrade().is_some_and(|t| t.state.is_trait_disabled()))),
            zoffset_func: None,
        },
        info.palette.clone(),
        info.is_player_palette,
    );
    Some(t)
});

conditional_simple!(WithIdleOverlay);

impl INotifyDamageStateChanged for WithIdleOverlay {
    fn damage_state_changed(&self, _self_: &ActorRef, e: &AttackInfo) {
        if let Some(s) = self.overlay.current_sequence() {
            self.overlay
                .replace_anim(&normalize_sequence(&self.overlay, e.damage_state, &s.name));
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct BodyOrientationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub quantized_facings: i32,
    pub camera_pitch: rustora_core::world_types::WAngle,
    pub use_classic_perspective_fudge: bool,
}

impl Default for BodyOrientationInfo {
    fn default() -> Self {
        BodyOrientationInfo {
            instance_name: None,
            quantized_facings: -1,
            camera_pitch: rustora_core::world_types::WAngle::from_degrees(40),
            use_classic_perspective_fudge: true,
        }
    }
}

/// `ClassicFacingBodyOrientationInfo`: same data, different facing quantization (pending).
pub struct ClassicFacingBodyOrientationInfo(pub BodyOrientationInfo);

pub struct BodyOrientation {
    pub info: BodyOrientationInfo,
    class_name: &'static str,
    self_: std::rc::Weak<rustora_engine::actor::Actor>,
    faction: String,
    quantized: Cell<Option<i32>>,
}

impl TraitObject for BodyOrientation {
    fn register(self: Rc<Self>, dict: &rustora_engine::trait_dict::TraitDict, actor: &ActorRef) {
        dict.add::<BodyOrientation>(actor, self.clone());
        dict.add::<dyn ISync>(actor, self);
    }
    fn type_name(&self) -> &'static str {
        self.class_name
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn create_body(
    info: BodyOrientationInfo,
    class_name: &'static str,
    init: &rustora_engine::actor_init::ActorInitializer,
) -> Option<Rc<dyn TraitObject>> {
    let faction = init
        .inits
        .get::<FactionInit>()
        .map(|f| f.value.clone())
        .unwrap_or_else(|| init.self_.owner().faction.clone());
    Some(Rc::new(BodyOrientation {
        info,
        class_name,
        self_: Rc::downgrade(&init.self_),
        faction,
        quantized: Cell::new(None),
    }))
}

info_impl!(BodyOrientationInfo => |info, init| create_body((*info).clone(), "BodyOrientation", init));

impl TraitInfo for ClassicFacingBodyOrientationInfo {
    fn create(self: Rc<Self>, init: &rustora_engine::actor_init::ActorInitializer) -> Option<Rc<dyn TraitObject>> {
        create_body(self.0.clone(), "ClassicFacingBodyOrientation", init)
    }
    fn register_info(self: Rc<Self>, index: &rustora_engine::actor_info::InfoIndex) {
        index.add::<ClassicFacingBodyOrientationInfo>(self);
    }
    fn instance_name(&self) -> Option<&str> {
        self.0.instance_name.as_deref()
    }
    fn info_type_name(&self) -> &'static str {
        "ClassicFacingBodyOrientationInfo"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl BodyOrientationInfo {
    /// `LocalToWorld`.
    pub fn local_to_world(&self, vec: WVec) -> WVec {
        if !self.use_classic_perspective_fudge {
            return WVec::new(vec.y, -vec.x, vec.z);
        }
        WVec::new(vec.y, (-self.camera_pitch.sin()).wrapping_mul(vec.x) / 1024, vec.z)
    }
}

impl BodyOrientation {
    /// `LocalToWorld`.
    pub fn local_to_world(&self, vec: WVec) -> WVec {
        self.info.local_to_world(vec)
    }

    pub fn quantize_orientation(&self, orientation: WRot) -> WRot {
        let f = self.quantized_facings();
        if f == 0 {
            return orientation;
        }
        WRot::from_yaw(self.quantize_facing_to(orientation.yaw, f))
    }

    pub fn quantize_facing(&self, facing: WAngle) -> WAngle {
        self.quantize_facing_to(facing, self.quantized_facings())
    }

    /// `QuantizeFacing(facing, facings)`, with the classic rounding of `ClassicFacingBodyOrientation`.
    pub fn quantize_facing_to(&self, facing: WAngle, facings: i32) -> WAngle {
        if self.class_name == "ClassicFacingBodyOrientation" {
            return crate::render::classic_quantize_facing(facing, facings);
        }
        crate::util::quantize_facing(facing, facings)
    }

    pub fn quantized_facings(&self) -> i32 {
        if let Some(q) = self.quantized.get() {
            return q;
        }
        let q = if self.info.quantized_facings >= 0 {
            self.info.quantized_facings
        } else {
            let a: ActorRef = self.self_.upgrade().expect("actor alive");
            let qi = a
                .info
                .trait_info::<QuantizeFacingsFromSequenceInfo>()
                .unwrap_or_else(|| panic!("Actor '{}' does not define a quantized body orientation.", a.info.name));
            let rsi = a.info.trait_info::<RenderSpritesInfo>().expect("RenderSprites");
            let image = rsi.get_image(&a.info, &self.faction);
            a.world()
                .sequence_facings(&image, &qi.sequence)
                .unwrap_or_else(|| panic!("missing sequence {image}.{}", qi.sequence))
        };
        self.quantized.set(Some(q));
        q
    }
}

impl ISync for BodyOrientation {
    fn sync_hash(&self) -> i32 {
        self.quantized_facings()
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct QuantizeFacingsFromSequenceInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub sequence: String,
}

impl Default for QuantizeFacingsFromSequenceInfo {
    fn default() -> Self {
        QuantizeFacingsFromSequenceInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            sequence: "idle".into(),
        }
    }
}

pub struct QuantizeFacingsFromSequence {
    state: ConditionalState,
}

impl ConditionalHooks for QuantizeFacingsFromSequence {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

register_traits!(QuantizeFacingsFromSequence: ISync, INotifyCreated, IObservesVariables, IDisabledTrait);
info_impl!(QuantizeFacingsFromSequenceInfo => |info, _| Some(Rc::new(QuantizeFacingsFromSequence { state: ConditionalState::new(&info.base) })));

impl ISync for QuantizeFacingsFromSequence {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash()
    }
}

impl INotifyCreated for QuantizeFacingsFromSequence {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl IObservesVariables for QuantizeFacingsFromSequence {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        conditional_observers(&self)
    }
}

impl IDisabledTrait for QuantizeFacingsFromSequence {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

const SPRITE_RANGES: [i32; 32] = [
    20, 56, 88, 132, 156, 184, 212, 240, 268, 296, 324, 352, 384, 416, 452, 488, 532, 568, 604, 644, 668, 696, 724, 752, 780, 808, 836,
    864, 896, 928, 964, 1000,
];

const SPRITE_FACINGS: [i32; 32] = [
    0, 40, 74, 112, 146, 172, 200, 228, 256, 284, 312, 340, 370, 402, 436, 472, 512, 552, 588, 626, 658, 684, 712, 740, 768, 796, 824, 852,
    882, 914, 948, 984,
];

/// `ClassicIndexFacing` of `Mods.Cnc/Util.cs`.
pub fn classic_index_facing(facing: WAngle, num_frames: i32) -> i32 {
    if num_frames == 32 {
        return SPRITE_RANGES.iter().position(|r| facing.angle < *r).map_or(0, |i| i as i32);
    }
    crate::util::index_facing(facing, num_frames)
}

/// `ClassicQuantizeFacing` of `Mods.Cnc/Util.cs`.
pub fn classic_quantize_facing(facing: WAngle, steps: i32) -> WAngle {
    if steps == 32 {
        return WAngle::new(SPRITE_FACINGS[classic_index_facing(facing, steps) as usize]);
    }
    crate::util::quantize_facing(facing, steps)
}

/// `Traits/Render/WithProductionDoorOverlay.cs`: a door that opens while a produced unit leaves the exit cell.
#[derive(Clone, Debug, FieldLoad)]
pub struct WithProductionDoorOverlayInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub sequence: String,
}

impl Default for WithProductionDoorOverlayInfo {
    fn default() -> Self {
        WithProductionDoorOverlayInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            sequence: "build-door".into(),
        }
    }
}

pub struct WithProductionDoorOverlay {
    state: ConditionalState,
    door: Rc<Animation>,
    desired_frame: Rc<Cell<i32>>,
    open_exit: Cell<rustora_core::world_types::CPos>,
    exiting_actor: RefCell<Option<ActorRef>>,
}

register_traits!(WithProductionDoorOverlay: ITick, crate::interfaces::INotifyProduction, INotifyDamageStateChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(WithProductionDoorOverlayInfo => |info, init| {
    let self_ = &init.self_;
    let rs = self_.trait_::<RenderSprites>();
    let door = Animation::new(init.world().map().sequences().clone(), &rs.get_image(self_), None, None);
    let desired_frame = Rc::new(Cell::new(0));
    let (d, f) = (door.clone(), desired_frame.clone());
    door.play_fetch_direction(&normalize_sequence(&door, get_damage_state(self_), &info.sequence), Rc::new(move || f.get() - d.current_frame()));
    let offset = self_.info.trait_info::<crate::building::BuildingInfo>().expect("Building").center_offset(&init.world()).y + 512;
    Some(Rc::new_cyclic(|me: &std::rc::Weak<WithProductionDoorOverlay>| {
        let disabled = me.clone();
        rs.add(
            AnimationWithOffset {
                animation: door.clone(),
                offset_func: None,
                disable_func: Some(Rc::new(move || disabled.upgrade().is_some_and(|t| t.state.is_trait_disabled()))),
                zoffset_func: Some(Rc::new(move |_| offset)),
            },
            None,
            false,
        );
        WithProductionDoorOverlay {
            state: ConditionalState::new(&info.base),
            door,
            desired_frame,
            open_exit: Cell::new(Default::default()),
            exiting_actor: RefCell::new(None),
        }
    }))
});
conditional_simple!(WithProductionDoorOverlay);

/// `Activity.ActivitiesImplementing<Mobile.ReturnToCellActivity>().Any()` over the children and next activities.
fn has_return_to_cell(a: Option<ActivityRef>) -> bool {
    let Some(a) = a else { return false };
    a.type_name() == "ReturnToCellActivity" || has_return_to_cell(a.base().child_activity()) || has_return_to_cell(a.base().next_activity())
}

impl ITick for WithProductionDoorOverlay {
    fn tick(&self, _self_: &ActorRef) {
        let Some(e) = self.exiting_actor.borrow().clone() else { return };
        if !e.is_in_world() || e.location() != self.open_exit.get() || !has_return_to_cell(e.current_activity()) {
            self.desired_frame.set(0);
            *self.exiting_actor.borrow_mut() = None;
        }
    }
}

impl INotifyDamageStateChanged for WithProductionDoorOverlay {
    fn damage_state_changed(&self, _self_: &ActorRef, e: &AttackInfo) {
        if let Some(s) = self.door.current_sequence() {
            self.door.replace_anim(&normalize_sequence(&self.door, e.damage_state, &s.name));
        }
    }
}

impl crate::interfaces::INotifyProduction for WithProductionDoorOverlay {
    fn unit_produced(&self, _self_: &ActorRef, other: &ActorRef, exit: rustora_core::world_types::CPos) {
        self.open_exit.set(exit);
        *self.exiting_actor.borrow_mut() = Some(other.clone());
        self.desired_frame.set(self.door.current_sequence().map_or(1, |s| s.length()) - 1);
    }
}
