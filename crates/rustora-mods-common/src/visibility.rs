// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Visibility traits: `AffectsShroud` (with `RevealsShroud` and `CreatesShroud`), `FrozenUnderFog`,
//! `HiddenUnderFog`, `HiddenUnderShroud` and `AlwaysVisible`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::IndexSet;
use rustora_core::field_enum;
use rustora_core::world_types::{CPos, PPos, WDist, WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::ActorInit;
use rustora_engine::conditional::*;
use rustora_engine::frozen::{FrozenActor, FrozenActorRef, ICreatesFrozenActors};
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::shroud::{Shroud, ShroudInfo, SourceType};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::actor_reference::SpawnedByMapInit;
use crate::building::BuildingInfo;
use crate::interfaces::*;
use crate::util::apply_percentage_modifiers;

pub fn register(r: &mut TraitRegistry) {
    r.register("Shroud", |n| load_info!(ShroudInfo, n));
    r.register("FrozenActorLayer", |n| load_info!(rustora_engine::frozen::FrozenActorLayerInfo, n));
    r.register("RevealsShroud", |n| load_info!(RevealsShroudInfo, n));
    r.register("CreatesShroud", |n| load_info!(CreatesShroudInfo, n));
    r.register("FrozenUnderFog", |n| load_info!(FrozenUnderFogInfo, n));
    r.register("HiddenUnderFog", |n| load_info!(HiddenUnderFogInfo, n));
    r.register("HiddenUnderShroud", |n| load_info!(HiddenUnderShroudInfo, n));
    r.register("AlwaysVisible", |n| load_info!(AlwaysVisibleInfo, n));
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VisibilityType {
    #[default]
    Footprint,
    CenterPosition,
    GroundPosition,
}

field_enum!(VisibilityType, [Footprint, CenterPosition, GroundPosition]);

#[derive(Clone, Debug, FieldLoad)]
pub struct AffectsShroudInfo {
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub min_range: WDist,
    pub range: WDist,
    pub max_height_delta: i32,
    pub move_recalculation_threshold: WDist,
    #[field(name = "Type")]
    pub ty: VisibilityType,
}

impl Default for AffectsShroudInfo {
    fn default() -> Self {
        AffectsShroudInfo {
            base: ConditionalTraitInfo::default(),
            min_range: WDist::ZERO,
            range: WDist::ZERO,
            max_height_delta: -1,
            move_recalculation_threshold: WDist::new(256),
            ty: VisibilityType::Footprint,
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RevealsShroudInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub affects: AffectsShroudInfo,
    pub valid_relationships: PlayerRelationship,
    pub reveal_generated_shroud: bool,
}

impl Default for RevealsShroudInfo {
    fn default() -> Self {
        RevealsShroudInfo {
            instance_name: None,
            affects: AffectsShroudInfo::default(),
            valid_relationships: PlayerRelationship::ALLY,
            reveal_generated_shroud: true,
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CreatesShroudInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub affects: AffectsShroudInfo,
    pub valid_relationships: PlayerRelationship,
}

impl Default for CreatesShroudInfo {
    fn default() -> Self {
        CreatesShroudInfo {
            instance_name: None,
            affects: AffectsShroudInfo::default(),
            valid_relationships: PlayerRelationship(PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0),
        }
    }
}

enum Modifiers {
    Reveals(Vec<Rc<dyn IRevealsShroudModifier>>),
    Creates(Vec<Rc<dyn ICreatesShroudModifier>>),
}

/// `AffectsShroud`, shared by `RevealsShroud` and `CreatesShroud`.
pub struct AffectsShroud {
    info: AffectsShroudInfo,
    valid_relationships: PlayerRelationship,
    source_type: SourceType,
    creates: bool,
    state: ConditionalState,
    cached_location: Cell<CPos>,
    cached_range: Cell<WDist>,
    cached_trait_disabled: Cell<bool>,
    cached_pos: Cell<WPos>,
    modifiers: OnceCell<Modifiers>,
    key: Cell<usize>,
}

impl TraitObject for AffectsShroud {
    fn register(self: Rc<Self>, dict: &rustora_engine::trait_dict::TraitDict, actor: &ActorRef) {
        self.key.set(Rc::as_ptr(&self) as *const () as usize);
        dict.add::<AffectsShroud>(actor, self.clone());
        dict.add::<dyn ISync>(actor, self.clone());
        dict.add::<dyn INotifyAddedToWorld>(actor, self.clone());
        dict.add::<dyn INotifyRemovedFromWorld>(actor, self.clone());
        dict.add::<dyn INotifyMoving>(actor, self.clone());
        dict.add::<dyn INotifyCenterPositionChanged>(actor, self.clone());
        dict.add::<dyn ITick>(actor, self.clone());
        dict.add::<dyn INotifyCreated>(actor, self.clone());
        dict.add::<dyn IObservesVariables>(actor, self.clone());
        dict.add::<dyn IDisabledTrait>(actor, self);
    }
    fn type_name(&self) -> &'static str {
        if self.creates { "CreatesShroud" } else { "RevealsShroud" }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn new_affects(a: &AffectsShroudInfo, valid: PlayerRelationship, source_type: SourceType, creates: bool) -> Rc<AffectsShroud> {
    Rc::new(AffectsShroud {
        info: a.clone(),
        valid_relationships: valid,
        source_type,
        creates,
        state: ConditionalState::new(&a.base),
        cached_location: Cell::new(CPos::ZERO),
        cached_range: Cell::new(WDist::ZERO),
        cached_trait_disabled: Cell::new(false),
        cached_pos: Cell::new(WPos::ZERO),
        modifiers: OnceCell::new(),
        key: Cell::new(0),
    })
}

info_impl!(RevealsShroudInfo => |info, _| {
    let t = if info.reveal_generated_shroud { SourceType::Visibility } else { SourceType::PassiveVisibility };
    Some(new_affects(&info.affects, info.valid_relationships, t, false))
});
info_impl!(CreatesShroudInfo => |info, _| Some(new_affects(&info.affects, info.valid_relationships, SourceType::Shroud, true)));

impl ConditionalHooks for AffectsShroud {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl ISync for AffectsShroud {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ sync_bool(self.cached_trait_disabled.get())
    }
}

impl IDisabledTrait for AffectsShroud {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl IObservesVariables for AffectsShroud {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        conditional_observers(&self)
    }
}

impl INotifyCreated for AffectsShroud {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
        let m = if self.creates {
            Modifiers::Creates(self_.traits_implementing::<dyn ICreatesShroudModifier>())
        } else {
            Modifiers::Reveals(self_.traits_implementing::<dyn IRevealsShroudModifier>())
        };
        let _ = self.modifiers.set(m);
    }
}

impl AffectsShroud {
    pub fn is_reveals_shroud(&self) -> bool {
        !self.creates
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn range(&self) -> WDist {
        if self.cached_trait_disabled.get() {
            return WDist::ZERO;
        }
        let r = match self.modifiers.get() {
            Some(Modifiers::Reveals(m)) => {
                apply_percentage_modifiers(self.info.range.length, m.iter().map(|x| x.get_reveals_shroud_modifier()))
            }
            Some(Modifiers::Creates(m)) => {
                apply_percentage_modifiers(self.info.range.length, m.iter().map(|x| x.get_creates_shroud_modifier()))
            }
            None => self.info.range.length,
        };
        WDist::new(r)
    }

    fn projected_cells(&self, self_: &ActorRef) -> Rc<[PPos]> {
        let w = self_.world();
        let map = w.map();
        let min_range = self.info.min_range;
        let max_range = self.range();
        if max_range.length <= min_range.length {
            return Rc::from(Vec::new());
        }
        let delta = self.info.max_height_delta;
        if self.info.ty == VisibilityType::Footprint {
            let mut set = IndexSet::new();
            for (c, _) in self_.cache().occupies_space.as_ref().expect("occupies space").occupied_cells() {
                for p in Shroud::projected_cells_in_range(map, map.center_of_cell(c), min_range, max_range, delta) {
                    set.insert(p);
                }
            }
            return Rc::from(set.into_iter().collect::<Vec<_>>());
        }
        let mut pos = self_.center_position();
        if self.info.ty == VisibilityType::GroundPosition {
            pos = pos - WVec::new(0, 0, map.distance_above_terrain(pos).length);
        }
        Rc::from(Shroud::projected_cells_in_range(map, pos, min_range, max_range, delta))
    }

    fn add_cells(&self, self_: &ActorRef, p: &PlayerRef, cells: &Rc<[PPos]>) {
        if !self.valid_relationships.has_relationship(self_.owner().relationship_with(Some(p))) {
            return;
        }
        p.shroud().add_source(self.key.get(), self.source_type, cells.clone());
    }

    fn remove_cells(&self, self_: &ActorRef, p: &PlayerRef) {
        if !self.valid_relationships.has_relationship(self_.owner().relationship_with(Some(p))) {
            return;
        }
        p.shroud().remove_source(self.key.get());
    }

    fn update_shroud_cells(&self, self_: &ActorRef) {
        let cells = self.projected_cells(self_);
        for p in self_.world().players() {
            self.remove_cells(self_, &p);
            self.add_cells(self_, &p, &cells);
        }
    }

    fn projected_location(&self, self_: &ActorRef) -> (CPos, WPos) {
        let c = self_.center_position();
        (self_.world().map().cell_containing(c - WVec::new(0, c.z, c.z)), c)
    }
}

impl INotifyCenterPositionChanged for AffectsShroud {
    fn center_position_changed(&self, self_: &ActorRef, _old: u8, _new: u8) {
        if !self_.is_in_world() {
            return;
        }
        let (loc, pos) = self.projected_location(self_);
        let t = self.info.move_recalculation_threshold;
        let dirty = t.length > 0 && (pos - self.cached_pos.get()).length_squared() > t.length_squared();
        if !dirty && self.cached_location.get() == loc {
            return;
        }
        self.cached_location.set(loc);
        self.cached_pos.set(pos);
        self.update_shroud_cells(self_);
    }
}

impl ITick for AffectsShroud {
    fn tick(&self, self_: &ActorRef) {
        if !self_.is_in_world() {
            return;
        }
        let disabled = self.state.is_trait_disabled();
        let range = self.range();
        if self.cached_range.get() == range && disabled == self.cached_trait_disabled.get() {
            return;
        }
        self.cached_range.set(range);
        self.cached_trait_disabled.set(disabled);
        self.update_shroud_cells(self_);
    }
}

impl INotifyAddedToWorld for AffectsShroud {
    fn added_to_world(&self, self_: &ActorRef) {
        let (loc, pos) = self.projected_location(self_);
        self.cached_location.set(loc);
        self.cached_pos.set(pos);
        self.cached_trait_disabled.set(self.state.is_trait_disabled());
        let cells = self.projected_cells(self_);
        for p in self_.world().players() {
            self.add_cells(self_, &p, &cells);
        }
    }
}

impl INotifyRemovedFromWorld for AffectsShroud {
    fn removed_from_world(&self, self_: &ActorRef) {
        for p in self_.world().players() {
            self.remove_cells(self_, &p);
        }
    }
}

impl INotifyMoving for AffectsShroud {
    fn movement_type_changed(&self, self_: &ActorRef, ty: u8) {
        if ty == crate::locomotor::movement_type::NONE && self_.is_in_world() {
            let (loc, pos) = self.projected_location(self_);
            self.cached_location.set(loc);
            self.cached_pos.set(pos);
            self.update_shroud_cells(self_);
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct FrozenUnderFogInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub always_visible_relationships: PlayerRelationship,
}

impl Default for FrozenUnderFogInfo {
    fn default() -> Self {
        FrozenUnderFogInfo {
            instance_name: None,
            always_visible_relationships: PlayerRelationship::ALLY,
        }
    }
}

pub struct HiddenUnderFogInit;

impl ActorInit for HiddenUnderFogInit {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct FrozenState {
    frozen_actor: FrozenActorRef,
    is_visible: Cell<bool>,
}

pub struct FrozenUnderFog {
    info: Rc<FrozenUnderFogInfo>,
    me: Weak<FrozenUnderFog>,
    visibility_hash: Cell<i32>,
    starts_revealed: bool,
    footprint: Vec<PPos>,
    frozen_states: RefCell<Vec<(PlayerRef, Rc<FrozenState>)>>,
    created: Cell<bool>,
}

register_traits!(FrozenUnderFog: IDefaultVisibility, ISync, INotifyCreated, INotifyOwnerChanged, INotifyActorDisposing);
info_impl!(FrozenUnderFogInfo => |info, init| {
    let w = init.world();
    let shroud_info = w.rules().actor("player").expect("player actor").trait_info::<ShroudInfo>().expect("Shroud");
    let explored = w.lobby_info.global_settings.option_bool("explored", shroud_info.explored_map_checkbox_enabled);
    let starts_revealed = explored && init.inits.contains::<SpawnedByMapInit>() && !init.inits.contains::<HiddenUnderFogInit>();
    let cells = match init.self_.info.trait_info::<BuildingInfo>() {
        Some(b) => b.frozen_under_fog_tiles(init.self_.location()),
        None => vec![init.self_.location()],
    };
    let footprint = cells.iter().filter(|c| w.map().tiles.contains_cell(**c)).map(|c| PPos::new(c.x(), c.y())).collect();
    Some(Rc::new_cyclic(|me| FrozenUnderFog {
        info,
        me: me.clone(),
        visibility_hash: Cell::new(0),
        starts_revealed,
        footprint,
        frozen_states: RefCell::new(Vec::new()),
        created: Cell::new(false),
    }))
});

impl ISync for FrozenUnderFog {
    fn sync_hash(&self) -> i32 {
        self.visibility_hash.get()
    }
}

impl INotifyCreated for FrozenUnderFog {
    fn created(&self, self_: &ActorRef) {
        let w = self_.world();
        let me: Rc<dyn ICreatesFrozenActors> = self.me.upgrade().expect("trait alive");
        for player in w.players() {
            let fa = FrozenActor::new(self_, me.clone(), &self.footprint, &player, self.starts_revealed);
            player
                .player_actor()
                .trait_::<rustora_engine::frozen::FrozenActorLayer>()
                .add(fa.clone());
            let vis = !fa.visible();
            self.frozen_states.borrow_mut().push((
                player,
                Rc::new(FrozenState {
                    frozen_actor: fa,
                    is_visible: Cell::new(vis),
                }),
            ));
        }
        let weak = self.me.clone();
        w.add_frame_end_task(Box::new(move |_| {
            let Some(me) = weak.upgrade() else { return };
            let states = me.frozen_states.borrow().clone();
            for (i, (_, state)) in states.iter().enumerate() {
                if me.starts_revealed || state.is_visible.get() {
                    me.update_frozen_actor(&state.frozen_actor, i as i32);
                }
                state.frozen_actor.refresh_hidden();
            }
        }));
        self.created.set(true);
    }
}

impl FrozenUnderFog {
    fn update_frozen_actor(&self, f: &FrozenActor, player_index: i32) {
        self.visibility_hash.set(self.visibility_hash.get() | (1i32 << (player_index % 32)));
        f.refresh_state();
    }

    fn state_of(&self, p: &PlayerRef) -> (usize, Rc<FrozenState>) {
        let states = self.frozen_states.borrow();
        let i = states.iter().position(|(q, _)| Rc::ptr_eq(q, p)).expect("player state");
        (i, states[i].1.clone())
    }

    fn is_visible_inner(&self, by: &PlayerRef) -> bool {
        let s = by.shroud();
        if !s.fog_enabled() {
            return s.any_explored(&self.footprint);
        }
        self.state_of(by).1.is_visible.get()
    }
}

impl FrozenUnderFog {
    /// `ITickRender.TickRender`: captures the actor's renderables for the frozen actors that need them.
    pub fn tick_render(&self, self_: &ActorRef) {
        let mut renderables = None;
        for (_, state) in self.frozen_states.borrow().iter() {
            let frozen = &state.frozen_actor;
            if !frozen.need_renderables.get() {
                continue;
            }
            let captured = renderables.get_or_insert_with(|| {
                let mut out = Vec::new();
                rustora_engine::render::render_actor(self_, &mut out);
                Rc::new(out)
            });
            frozen.need_renderables.set(false);
            *frozen.renderables.borrow_mut() = captured.clone();
        }
    }
}

impl ICreatesFrozenActors for FrozenUnderFog {
    fn on_visibility_changed(&self, frozen: &FrozenActor) {
        if !self.created.get() {
            return;
        }
        let (i, state) = self.state_of(&frozen.viewer);
        let is_visible = !frozen.visible();
        state.is_visible.set(is_visible);
        if is_visible {
            self.update_frozen_actor(frozen, i as i32);
        }
        frozen.refresh_hidden();
    }
}

impl IDefaultVisibility for FrozenUnderFog {
    fn is_visible(&self, self_: &ActorRef, by_player: Option<&PlayerRef>) -> bool {
        let Some(by) = by_player else { return true };
        let r = self_.owner().relationship_with(Some(by));
        self.info.always_visible_relationships.has_relationship(r) || self.is_visible_inner(by)
    }
}

impl INotifyOwnerChanged for FrozenUnderFog {
    fn on_owner_changed(&self, _self_: &ActorRef, old: &PlayerRef, _new: &PlayerRef) {
        let (i, state) = self.state_of(old);
        self.update_frozen_actor(&state.frozen_actor, i as i32);
        state.frozen_actor.refresh_hidden();
    }
}

impl INotifyActorDisposing for FrozenUnderFog {
    fn disposing(&self, self_: &ActorRef) {
        self.state_of(&self_.owner()).1.frozen_actor.invalidate();
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct HiddenUnderShroudInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub always_visible_relationships: PlayerRelationship,
    #[field(name = "Type")]
    pub ty: VisibilityType,
}

impl Default for HiddenUnderShroudInfo {
    fn default() -> Self {
        HiddenUnderShroudInfo {
            instance_name: None,
            always_visible_relationships: PlayerRelationship::ALLY,
            ty: VisibilityType::Footprint,
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct HiddenUnderFogInfo {
    #[field(flatten)]
    pub base: HiddenUnderShroudInfo,
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct HiddenUnderShroud {
    always_visible: PlayerRelationship,
    ty: VisibilityType,
    fog: bool,
}

register_traits!(HiddenUnderShroud: IDefaultVisibility);
info_impl!(HiddenUnderShroudInfo => |info, _| Some(Rc::new(HiddenUnderShroud { always_visible: info.always_visible_relationships, ty: info.ty, fog: false })));
info_impl!(HiddenUnderFogInfo => |info, _| Some(Rc::new(HiddenUnderShroud { always_visible: info.base.always_visible_relationships, ty: info.base.ty, fog: true })));

fn ground_pos(w: &World, self_: &ActorRef, ty: VisibilityType) -> WPos {
    let mut pos = self_.center_position();
    if ty == VisibilityType::GroundPosition {
        pos = pos - WVec::new(0, 0, w.map().distance_above_terrain(pos).length);
    }
    pos
}

impl HiddenUnderShroud {
    fn shroud_inner(&self, self_: &ActorRef, by: &PlayerRef) -> bool {
        let s = by.shroud();
        if self.ty == VisibilityType::Footprint {
            return self_
                .cache()
                .occupies_space
                .as_ref()
                .expect("occupies space")
                .occupied_cells()
                .iter()
                .any(|(c, _)| s.is_explored(*c));
        }
        s.is_explored_wpos(ground_pos(&self_.world(), self_, self.ty))
    }

    fn fog_inner(&self, self_: &ActorRef, by: &PlayerRef) -> bool {
        let s = by.shroud();
        if !s.fog_enabled() {
            return self.shroud_inner(self_, by);
        }
        if self.ty == VisibilityType::Footprint {
            return self_
                .cache()
                .occupies_space
                .as_ref()
                .expect("occupies space")
                .occupied_cells()
                .iter()
                .any(|(c, _)| s.is_visible(*c));
        }
        s.is_visible_wpos(ground_pos(&self_.world(), self_, self.ty))
    }
}

impl IDefaultVisibility for HiddenUnderShroud {
    fn is_visible(&self, self_: &ActorRef, by_player: Option<&PlayerRef>) -> bool {
        let Some(by) = by_player else { return true };
        let r = self_.owner().relationship_with(Some(by));
        self.always_visible.has_relationship(r)
            || if self.fog {
                self.fog_inner(self_, by)
            } else {
                self.shroud_inner(self_, by)
            }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct AlwaysVisibleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct AlwaysVisible;

register_traits!(AlwaysVisible: IDefaultVisibility);
info_impl!(AlwaysVisibleInfo => |_, _| Some(Rc::new(AlwaysVisible)));

impl IDefaultVisibility for AlwaysVisible {
    fn is_visible(&self, _self_: &ActorRef, _by_player: Option<&PlayerRef>) -> bool {
        true
    }
}
