// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Player/FrozenActorLayer.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::Rectangle;
use rustora_core::world_types::{CPos, CVec, Int2, PPos, WDist, WPos};
use rustora_derive::FieldLoad;

use crate::actor::ActorRef;
use crate::actor_ext::{enabled_target_types, targetable_positions};
use crate::actor_info::ActorInfo;
use crate::bitset::BitSet;
use crate::damage::DamageState;
use crate::net_collections::NetMap;
use crate::player::PlayerRef;
use crate::shroud::{Shroud, VISIBILITY_EXPLORED, VISIBILITY_VISIBLE};
use crate::spatial::SpatiallyPartitioned;
use crate::terrain::TargetableType;
use crate::traits::*;
use crate::world::World;
use crate::{info_impl, register_traits};

pub type FrozenActorRef = Rc<FrozenActor>;

pub trait ICreatesFrozenActors {
    fn on_visibility_changed(&self, frozen: &FrozenActor);
}

pub struct FrozenActor {
    pub footprint: Vec<PPos>,
    pub center_position: WPos,
    actor: ActorRef,
    frozen_trait: Rc<dyn ICreatesFrozenActors>,
    shroud: Rc<Shroud>,
    targetable_positions: RefCell<Vec<WPos>>,
    pub viewer: PlayerRef,
    owner: RefCell<Option<PlayerRef>>,
    target_types: Cell<BitSet<TargetableType>>,
    hp: Cell<i32>,
    damage_state: Cell<DamageState>,
    visible: Cell<bool>,
    hidden: Cell<bool>,
    shrouded: Cell<bool>,
    pub need_renderables: Cell<bool>,
    /// The simulation's copy of `NeedRenderables`, consumed by [`FrozenActorLayer::emulate_tick_render`].
    render_pending: Cell<bool>,
    /// Whether `TickRender` would have captured renderables, see [`FrozenActor::has_renderables`].
    rendered: Cell<bool>,
    /// `FrozenActor.Renderables`: what the client draws while the actor is hidden.
    pub renderables: RefCell<Rc<Vec<crate::render::Renderable>>>,
    pub update_visibility_next_tick: Cell<bool>,
}

impl FrozenActor {
    pub fn new(
        actor: &ActorRef,
        frozen_trait: Rc<dyn ICreatesFrozenActors>,
        footprint: &[PPos],
        viewer: &PlayerRef,
        starts_revealed: bool,
    ) -> FrozenActorRef {
        let shroud = viewer.shroud();
        let footprint: Vec<PPos> = footprint.iter().copied().filter(|m| shroud.contains(*m)).collect();
        assert!(!footprint.is_empty(), "This frozen actor has no footprint: {actor:?}");
        let fa = Rc::new(FrozenActor {
            footprint,
            center_position: actor.center_position(),
            actor: actor.clone(),
            frozen_trait,
            shroud,
            targetable_positions: RefCell::new(Vec::new()),
            viewer: viewer.clone(),
            owner: RefCell::new(None),
            target_types: Cell::new(BitSet::default()),
            hp: Cell::new(0),
            damage_state: Cell::new(DamageState(0)),
            visible: Cell::new(true),
            hidden: Cell::new(false),
            shrouded: Cell::new(false),
            renderables: RefCell::new(Rc::new(Vec::new())),
            need_renderables: Cell::new(starts_revealed),
            render_pending: Cell::new(starts_revealed),
            rendered: Cell::new(false),
            update_visibility_next_tick: Cell::new(false),
        });
        fa.update_visibility();
        fa
    }

    pub fn id(&self) -> u32 {
        self.actor.id
    }

    pub fn is_valid(&self) -> bool {
        self.owner.borrow().is_some()
    }

    pub fn owner(&self) -> Option<PlayerRef> {
        self.owner.borrow().clone()
    }

    pub fn info(&self) -> &Rc<ActorInfo> {
        &self.actor.info
    }

    /// The backing actor unless it died (`FrozenActor.Actor`).
    pub fn actor(&self) -> Option<ActorRef> {
        if self.actor.is_dead() { None } else { Some(self.actor.clone()) }
    }

    pub fn center_position(&self) -> WPos {
        self.center_position
    }

    pub fn targetable_positions(&self) -> Vec<WPos> {
        self.targetable_positions.borrow().clone()
    }

    pub fn target_types(&self) -> BitSet<TargetableType> {
        self.target_types.get()
    }

    pub fn hp(&self) -> i32 {
        self.hp.get()
    }

    pub fn damage_state(&self) -> DamageState {
        self.damage_state.get()
    }

    pub fn visible(&self) -> bool {
        self.visible.get()
    }

    pub fn hidden(&self) -> bool {
        self.hidden.get()
    }

    pub fn shrouded(&self) -> bool {
        self.shrouded.get()
    }

    pub fn refresh_state(&self) {
        *self.owner.borrow_mut() = self.actor.owner_opt();
        self.target_types.set(enabled_target_types(&self.actor));
        *self.targetable_positions.borrow_mut() = targetable_positions(&self.actor);
        if let Some(h) = self.actor.cache().health.as_ref() {
            self.hp.set(h.hp());
            self.damage_state.set(h.damage_state());
        }
    }

    pub fn refresh_hidden(&self) {
        self.hidden.set(false);
        for m in &self.actor.cache().visibility_modifiers {
            if !m.is_visible(&self.actor, Some(&self.viewer)) {
                self.hidden.set(true);
                break;
            }
        }
    }

    pub fn tick(&self) {
        if self.update_visibility_next_tick.get() {
            self.update_visibility();
        }
    }

    fn update_visibility(&self) {
        self.update_visibility_next_tick.set(false);
        let was_visible = self.visible.get();
        self.shrouded.set(true);
        self.visible.set(true);
        for &puv in &self.footprint {
            let cv = self.shroud.get_visibility(puv);
            if cv & VISIBILITY_VISIBLE != 0 {
                self.visible.set(false);
                self.shrouded.set(false);
                break;
            }
            if self.shrouded.get() && cv & VISIBILITY_EXPLORED != 0 {
                self.shrouded.set(false);
            }
        }
        if self.visible.get() != was_visible {
            self.frozen_trait.on_visibility_changed(self);
        }
        if self.visible.get() && !was_visible {
            self.request_renderables();
        }
    }

    /// Sets `NeedRenderables`.
    pub fn request_renderables(&self) {
        self.need_renderables.set(true);
        self.render_pending.set(true);
    }

    /// `FrozenActor.HasRenderables`. C# fills the renderables in `FrozenUnderFog.TickRender`, so this simulation
    /// state depends on the renderer; the simulation assumes a render after every tick of an actor with sprites.
    pub fn has_renderables(&self) -> bool {
        !self.shrouded.get() && self.rendered.get()
    }

    fn emulate_tick_render(&self) {
        if self.render_pending.get() && self.actor.is_in_world() {
            self.render_pending.set(false);
            self.rendered.set(self.actor.info.has_trait_info_named("RenderSprites"));
        }
    }

    pub fn invalidate(&self) {
        *self.owner.borrow_mut() = None;
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct FrozenActorLayerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub bin_size: i32,
}

impl Default for FrozenActorLayerInfo {
    fn default() -> Self {
        FrozenActorLayerInfo {
            instance_name: None,
            bin_size: 10,
        }
    }
}

pub struct FrozenActorLayer {
    visibility_hash: Cell<i32>,
    frozen_hash: Cell<i32>,
    by_id: RefCell<NetMap<u32, FrozenActorRef>>,
    partitioned: Rc<RefCell<SpatiallyPartitioned<u32, FrozenActorRef>>>,
}

register_traits!(FrozenActorLayer: ITick, ISync);
info_impl!(FrozenActorLayerInfo => |info, init| {
    let w = init.world();
    let (mw, mh) = w.map().map_size;
    let partitioned = Rc::new(RefCell::new(SpatiallyPartitioned::<u32, FrozenActorRef>::new(mw, mh, info.bin_size)));
    let p2 = partitioned.clone();
    init.self_.trait_::<Shroud>().on_shroud_changed(Rc::new(move |uv: PPos| {
        for fa in p2.borrow().at(Int2::new(uv.u, uv.v)) {
            fa.update_visibility_next_tick.set(true);
        }
    }));
    Some(Rc::new(FrozenActorLayer {
        visibility_hash: Cell::new(0),
        frozen_hash: Cell::new(0),
        by_id: RefCell::new(NetMap::new()),
        partitioned,
    }))
});

impl ISync for FrozenActorLayer {
    fn sync_hash(&self) -> i32 {
        self.visibility_hash.get() ^ self.frozen_hash.get()
    }
}

impl ITick for FrozenActorLayer {
    fn tick(&self, _self_: &ActorRef) {
        self.visibility_hash.set(0);
        self.frozen_hash.set(0);
        let all: Vec<FrozenActorRef> = self.by_id.borrow().values().cloned().collect();
        let mut to_remove = Vec::new();
        for fa in all {
            let hash = fa.id() as i32;
            self.frozen_hash.set(self.frozen_hash.get().wrapping_add(hash));
            fa.tick();
            if fa.visible() {
                self.visibility_hash.set(self.visibility_hash.get().wrapping_add(hash));
            } else if fa.actor().is_none() {
                to_remove.push(fa);
            }
        }
        for fa in to_remove {
            self.remove(&fa);
        }
    }
}

fn footprint_bounds(fa: &FrozenActor) -> Rectangle {
    let p1 = fa.footprint[0];
    let (mut min_u, mut max_u, mut min_v, mut max_v) = (p1.u, p1.u, p1.v, p1.v);
    for p in &fa.footprint {
        if min_u > p.u {
            min_u = p.u;
        } else if max_u < p.u {
            max_u = p.u;
        }
        if min_v > p.v {
            min_v = p.v;
        } else if max_v < p.v {
            max_v = p.v;
        }
    }
    Rectangle::new(min_u, min_v, max_u + 1 - min_u, max_v + 1 - min_v)
}

impl FrozenActorLayer {
    pub fn add(&self, fa: FrozenActorRef) {
        let id = fa.id();
        assert!(self.by_id.borrow_mut().insert(id, fa.clone()), "duplicate frozen actor");
        let b = footprint_bounds(&fa);
        self.partitioned.borrow_mut().add(id, fa, b);
    }

    pub fn remove(&self, fa: &FrozenActor) {
        self.partitioned.borrow_mut().remove(&fa.id());
        self.by_id.borrow_mut().remove(&fa.id());
    }

    /// Emulates `FrozenUnderFog.TickRender`, which runs after each tick, for [`FrozenActor::has_renderables`].
    pub fn emulate_tick_render(&self) {
        for fa in self.by_id.borrow().values() {
            fa.emulate_tick_render();
        }
    }

    pub fn from_id(&self, id: u32) -> Option<FrozenActorRef> {
        self.by_id.borrow().get(&id).cloned()
    }

    pub fn frozen_actors_in_circle(&self, world: &World, origin: WPos, r: WDist, only_visible: bool) -> Vec<FrozenActorRef> {
        let center = world.map().cell_containing(origin);
        let cell_range = (r.length + 1023) / 1024;
        let tl: CPos = center - CVec::new(cell_range, cell_range);
        let br: CPos = center + CVec::new(cell_range, cell_range);
        self.partitioned
            .borrow()
            .in_box(Rectangle::new(tl.x(), tl.y(), br.x() - tl.x(), br.y() - tl.y()))
            .into_iter()
            .filter(|fa| {
                fa.is_valid()
                    && (!only_visible || fa.visible())
                    && (fa.center_position - origin).horizontal_length_squared() <= r.length_squared()
            })
            .collect()
    }

    pub fn frozen_actors_in_region(&self, tl: CPos, br: CPos, only_visible: bool) -> Vec<FrozenActorRef> {
        self.partitioned
            .borrow()
            .in_box(Rectangle::new(tl.x(), tl.y(), br.x() - tl.x(), br.y() - tl.y()))
            .into_iter()
            .filter(|fa| fa.is_valid() && (!only_visible || fa.visible()))
            .collect()
    }
}
