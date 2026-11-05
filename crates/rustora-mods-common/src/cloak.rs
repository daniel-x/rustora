// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Cloak.cs` and `Traits/DetectCloaked.cs`. Rendering, sounds and cloak effects are left out.
//! `INotifyDemolition`, `INotifyInfiltration` and `INotifySupportPower` are not wired yet.

use std::cell::{Cell, OnceCell};
use std::rc::Rc;

use rustora_core::field_flags;
use rustora_core::world_types::{CPos, WDist};
use rustora_derive::FieldLoad;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::AttackInfo;
use rustora_engine::player::PlayerRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::Target;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::attack::{Armament, Barrel};
use crate::interfaces::*;

pub fn register(r: &mut TraitRegistry) {
    r.register("Cloak", |n| load_info!(CloakInfo, n));
    r.register("DetectCloaked", |n| load_info!(DetectCloakedInfo, n));
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UncloakType(pub i32);

#[allow(non_upper_case_globals)]
impl UncloakType {
    pub const None: UncloakType = UncloakType(0);
    pub const Attack: UncloakType = UncloakType(1);
    pub const Move: UncloakType = UncloakType(2);
    pub const Load: UncloakType = UncloakType(4);
    pub const Unload: UncloakType = UncloakType(8);
    pub const Infiltrate: UncloakType = UncloakType(16);
    pub const Demolish: UncloakType = UncloakType(32);
    pub const Damage: UncloakType = UncloakType(64);
    pub const Heal: UncloakType = UncloakType(128);
    pub const SelfHeal: UncloakType = UncloakType(256);
    pub const Dock: UncloakType = UncloakType(512);
    pub const SupportPower: UncloakType = UncloakType(1024);

    pub fn has(self, f: UncloakType) -> bool {
        self.0 & f.0 == f.0
    }
}

field_flags!(
    UncloakType,
    [
        None,
        Attack,
        Move,
        Load,
        Unload,
        Infiltrate,
        Demolish,
        Damage,
        Heal,
        SelfHeal,
        Dock,
        SupportPower
    ]
);

/// Tag of `BitSet<DetectionType>`.
pub struct DetectionType;

#[derive(Clone, Debug, FieldLoad)]
pub struct CloakInfo {
    pub cloak_sound: Option<String>,
    pub uncloak_sound: Option<String>,
    pub cloak_style: String,
    pub cloaked_alpha: f32,
    pub cloaked_color: rustora_core::fieldloader::Color,
    pub cloaked_palette: Option<String>,
    pub is_player_palette: bool,
    pub effect_image: Option<String>,
    pub cloak_effect_sequence: Option<String>,
    pub uncloak_effect_sequence: Option<String>,
    pub effect_palette: String,
    pub effect_palette_is_player_palette: bool,
    pub effect_offset: rustora_core::world_types::WVec,
    pub effect_tracks_actor: bool,
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub initial_delay: i32,
    pub cloak_delay: i32,
    pub uncloak_on: UncloakType,
    pub detection_types: BitSet<DetectionType>,
    pub cloaked_condition: Option<String>,
    pub cloak_type: Option<String>,
}

impl Default for CloakInfo {
    fn default() -> Self {
        CloakInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            initial_delay: 10,
            cloak_delay: 30,
            cloak_sound: None,
            uncloak_sound: None,
            cloak_style: "Alpha".into(),
            cloaked_alpha: 0.55,
            cloaked_color: rustora_core::fieldloader::Color::from_argb(140, 0, 0, 0),
            cloaked_palette: None,
            is_player_palette: false,
            effect_image: None,
            cloak_effect_sequence: None,
            uncloak_effect_sequence: None,
            effect_palette: "effect".into(),
            effect_palette_is_player_palette: false,
            effect_offset: rustora_core::world_types::WVec::ZERO,
            effect_tracks_actor: true,
            uncloak_on: UncloakType(
                UncloakType::Attack.0 | UncloakType::Unload.0 | UncloakType::Infiltrate.0 | UncloakType::Demolish.0 | UncloakType::Dock.0,
            ),
            detection_types: BitSet::new(&["Cloak"]),
            cloaked_condition: None,
            cloak_type: None,
        }
    }
}

pub struct Cloak {
    pub info: Rc<CloakInfo>,
    state: ConditionalState,
    remaining_time: Cell<i32>,
    is_docking: Cell<bool>,
    other_cloaks: OnceCell<Option<Vec<Rc<Cloak>>>>,
    last_pos: Cell<Option<CPos>>,
    was_cloaked: Cell<bool>,
    first_tick: Cell<bool>,
    cloaked_token: Cell<i32>,
}

register_traits!(
    Cloak: ITick,
    INotifyDamage,
    INotifyUnloadCargo,
    INotifyLoadCargo,
    INotifyAttack,
    rustora_engine::render::IRenderModifier,
    IVisibilityModifier,
    INotifyDockClient,
    INotifyDockHost,
    INotifyCreated,
    IObservesVariables,
    IDisabledTrait,
    ISync
);
info_impl!(CloakInfo => |info, _| {
    let state = ConditionalState::new_pausable(&info.base);
    Some(Rc::new(Cloak {
        remaining_time: Cell::new(info.initial_delay),
        info,
        state,
        is_docking: Cell::new(false),
        other_cloaks: OnceCell::new(),
        last_pos: Cell::new(None),
        was_cloaked: Cell::new(false),
        first_tick: Cell::new(true),
        cloaked_token: Cell::new(INVALID_CONDITION_TOKEN),
    }))
});

impl ConditionalHooks for Cloak {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, _self_: &ActorRef) {
        self.remaining_time.set(self.info.initial_delay);
    }

    fn trait_disabled(&self, _self_: &ActorRef) {
        self.uncloak();
    }
}
rustora_engine::conditional_observers_impl!(Cloak);

impl ISync for Cloak {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.remaining_time.get()
    }
}

impl INotifyCreated for Cloak {
    fn created(&self, self_: &ActorRef) {
        let others = self.info.cloak_type.as_ref().map(|ct| {
            self_
                .traits_implementing::<Cloak>()
                .into_iter()
                .filter(|c| !std::ptr::eq(c.as_ref(), self) && c.info.cloak_type.as_ref() == Some(ct))
                .collect()
        });
        let _ = self.other_cloaks.set(others);
        if self.cloaked() {
            self.was_cloaked.set(true);
            self.grant_cloaked_condition(self_);
        }
        conditional_created(self, self_);
    }
}

impl rustora_engine::render::IRenderModifier for Cloak {
    /// `ModifyRender`. The scene only asks for actors the render player can see.
    fn modify_render(&self, self_: &ActorRef, renderables: &mut Vec<rustora_engine::render::Renderable>) {
        use rustora_engine::render::{PaletteRef, Renderable};
        if self.remaining_time.get() > 0 || self.state.is_trait_disabled() || self.state.is_trait_paused() {
            return;
        }
        let info = &self.info;
        let c = info.cloaked_color;
        for r in renderables.iter_mut() {
            let Renderable::Sprite(s) = r else { continue };
            if s.is_decoration {
                continue;
            }
            match info.cloak_style.as_str() {
                "Alpha" => s.alpha = info.cloaked_alpha,
                "Color" => {
                    s.replace_color = Some([c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]);
                    s.alpha = c.a() as f32 / 255.0;
                }
                "Palette" => s.palette = PaletteRef::resolve(info.cloaked_palette.as_deref(), info.is_player_palette, &self_.owner()),
                _ => {}
            }
        }
    }
}

impl Cloak {
    /// The effect of a cloak state change.
    fn spawn_effect(&self, self_: &ActorRef, sequence: Option<&str>) {
        let info = &self.info;
        let (Some(image), Some(sequence)) = (info.effect_image.as_deref(), sequence) else {
            return;
        };
        let offset = info.effect_offset;
        let pos = self_.center_position();
        let a = self_.clone();
        let pos_func: Box<dyn Fn() -> rustora_core::world_types::WPos> = if info.effect_tracks_actor {
            Box::new(move || a.center_position() + offset)
        } else {
            Box::new(move || pos + offset)
        };
        let palette =
            rustora_engine::render::PaletteRef::resolve(Some(&info.effect_palette), info.effect_palette_is_player_palette, &self_.owner());
        let w = self_.world();
        let e: rustora_engine::effect::EffectRef = crate::effects::SpriteEffect::new(
            &w,
            pos_func,
            Rc::new(|| rustora_core::world_types::WAngle::ZERO),
            image,
            sequence,
            palette,
            false,
            0,
        );
        w.add_frame_end_task(Box::new(move |w| w.add_effect(e)));
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn cloaked(&self) -> bool {
        !self.state.is_trait_disabled() && !self.state.is_trait_paused() && self.remaining_time.get() <= 0
    }

    pub fn uncloak(&self) {
        self.uncloak_for(self.info.cloak_delay);
    }

    pub fn uncloak_for(&self, time: i32) {
        self.remaining_time.set(self.remaining_time.get().max(time));
    }

    fn uncloak_on(&self, t: UncloakType) {
        if self.info.uncloak_on.has(t) {
            self.uncloak();
        }
    }

    fn grant_cloaked_condition(&self, self_: &ActorRef) {
        if self.cloaked_token.get() == INVALID_CONDITION_TOKEN {
            self.cloaked_token
                .set(self_.grant_condition(self.info.cloaked_condition.as_deref().unwrap_or("")));
        }
    }

    pub fn is_visible_to(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> bool {
        if !self.cloaked() || self_.owner().is_allied_with(viewer.map(|v| v.as_ref())) {
            return true;
        }
        let Some(viewer) = viewer else { return false };
        let w = self_.world();
        let pos = self_.center_position();
        w.actors_with_trait::<DetectCloaked>().iter().any(|(a, d)| {
            a.is_in_world()
                && a.owner().is_allied_with(Some(viewer))
                && self.info.detection_types.overlaps(&d.info.detection_types)
                && (pos - a.center_position()).length_squared() <= d.range().length_squared()
        })
    }
}

impl ITick for Cloak {
    fn tick(&self, self_: &ActorRef) {
        if !self.state.is_trait_disabled() && !self.state.is_trait_paused() {
            if self.remaining_time.get() > 0 && !self.is_docking.get() {
                self.remaining_time.set(self.remaining_time.get() - 1);
            }
            if self.info.uncloak_on.has(UncloakType::Move) && self.last_pos.get() != Some(self_.location()) {
                self.uncloak();
                self.last_pos.set(Some(self_.location()));
            }
        }
        let is_cloaked = self.cloaked();
        let show_change = !(self.first_tick.get() && self.info.initial_delay == 0)
            && self
                .other_cloaks
                .get()
                .and_then(|o| o.as_ref())
                .is_none_or(|others| !others.iter().any(|c| c.cloaked()));
        if is_cloaked && !self.was_cloaked.get() {
            self.grant_cloaked_condition(self_);
            if show_change {
                self.spawn_effect(self_, self.info.cloak_effect_sequence.as_deref());
            }
        } else if !is_cloaked && self.was_cloaked.get() {
            if self.cloaked_token.get() != INVALID_CONDITION_TOKEN {
                self.cloaked_token.set(self_.revoke_condition(self.cloaked_token.get()));
            }
            if show_change {
                self.spawn_effect(self_, self.info.uncloak_effect_sequence.as_deref());
            }
        }
        self.was_cloaked.set(is_cloaked);
        self.first_tick.set(false);
    }
}

impl IVisibilityModifier for Cloak {
    fn is_visible(&self, self_: &ActorRef, by_player: Option<&PlayerRef>) -> bool {
        self.is_visible_to(self_, by_player)
    }
}

impl INotifyAttack for Cloak {
    fn attacking(&self, _self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {
        self.uncloak_on(UncloakType::Attack);
    }

    fn preparing_attack(&self, _self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {}
}

impl INotifyDamage for Cloak {
    fn damaged(&self, self_: &ActorRef, e: &AttackInfo) {
        if e.damage.value == 0 {
            return;
        }
        let t = if e.damage.value < 0 {
            if e.attacker.as_ref().is_some_and(|a| a.id == self_.id) {
                UncloakType::SelfHeal
            } else {
                UncloakType::Heal
            }
        } else {
            UncloakType::Damage
        };
        self.uncloak_on(t);
    }
}

impl INotifyDockClient for Cloak {
    fn docked(&self, _self_: &ActorRef, _host: &ActorRef) {
        if self.info.uncloak_on.has(UncloakType::Dock) {
            self.is_docking.set(true);
            self.uncloak();
        }
    }

    fn undocked(&self, _self_: &ActorRef, _host: &ActorRef) {
        if self.info.uncloak_on.has(UncloakType::Dock) {
            self.is_docking.set(false);
        }
    }
}

impl INotifyDockHost for Cloak {
    fn docked(&self, _self_: &ActorRef, _client: &ActorRef) {
        if self.info.uncloak_on.has(UncloakType::Dock) {
            self.is_docking.set(true);
            self.uncloak();
        }
    }

    fn undocked(&self, _self_: &ActorRef, _client: &ActorRef) {
        if self.info.uncloak_on.has(UncloakType::Dock) {
            self.is_docking.set(false);
        }
    }
}

impl INotifyLoadCargo for Cloak {
    fn loading(&self, _self_: &ActorRef) {
        self.uncloak_on(UncloakType::Load);
    }
}

impl INotifyUnloadCargo for Cloak {
    fn unloading(&self, _self_: &ActorRef) {
        self.uncloak_on(UncloakType::Unload);
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct DetectCloakedInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub detection_types: BitSet<DetectionType>,
    pub range: WDist,
}

impl Default for DetectCloakedInfo {
    fn default() -> Self {
        DetectCloakedInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            detection_types: BitSet::new(&["Cloak"]),
            range: WDist::from_cells(5),
        }
    }
}

/// `DetectCloaked`; RA has no `IDetectCloakedModifier`, so the range is not modified.
pub struct DetectCloaked {
    pub info: Rc<DetectCloakedInfo>,
    state: ConditionalState,
}

register_traits!(DetectCloaked: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(DetectCloakedInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(DetectCloaked { info, state }))
});

rustora_engine::conditional_simple!(DetectCloaked);

impl DetectCloaked {
    pub fn range(&self) -> WDist {
        if self.state.is_trait_disabled() {
            WDist::ZERO
        } else {
            self.info.range
        }
    }
}
