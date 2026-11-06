// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Mods.Cnc/Traits/GpsWatcher.cs`, `Mods.Cnc/Traits/FrozenUnderFogUpdatedByGps.cs`,
//! `Mods.Cnc/Effects/SatelliteLaunch.cs`, `Mods.Cnc/Effects/GpsSatellite.cs` and `Traits/Radar/ProvidesRadar.cs`.
//! `GpsPower` itself is a [`crate::support_powers::PowerKind`].

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use rustora_core::world_types::{WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::*;
use rustora_engine::effect::{Effect, EffectRef};
use rustora_engine::player::PlayerRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sequence::Animation;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("GpsWatcher", |n| load_info!(GpsWatcherInfo, n));
    r.register("FrozenUnderFogUpdatedByGps", |n| load_info!(FrozenUnderFogUpdatedByGpsInfo, n));
    r.register("ProvidesRadar", |n| load_info!(ProvidesRadarInfo, n));
}

pub trait IPreventsShroudReset {
    fn prevent_shroud_reset(&self, self_: &ActorRef) -> bool;
}

pub trait IOnGpsRefreshed {
    fn on_gps_refresh(&self, self_: &ActorRef, player: &PlayerRef);
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GpsWatcherInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct GpsWatcher {
    owner: PlayerRef,
    launched: Cell<bool>,
    granted_allies: Cell<bool>,
    granted: Cell<bool>,
    explored: Cell<bool>,
    actors: RefCell<Vec<ActorRef>>,
    notify_on_refresh: RefCell<Vec<(ActorRef, Rc<dyn IOnGpsRefreshed>)>>,
}

register_traits!(GpsWatcher: ISync, IPreventsShroudReset);
info_impl!(GpsWatcherInfo => |_, init| Some(Rc::new(GpsWatcher {
    owner: init.self_.owner(),
    launched: Cell::new(false),
    granted_allies: Cell::new(false),
    granted: Cell::new(false),
    explored: Cell::new(false),
    actors: RefCell::new(Vec::new()),
    notify_on_refresh: RefCell::new(Vec::new()),
})));

impl ISync for GpsWatcher {
    fn sync_hash(&self) -> i32 {
        let b = |c: &Cell<bool>| c.get() as i32;
        b(&self.launched) ^ b(&self.granted_allies) ^ b(&self.granted) ^ b(&self.explored)
    }
}

impl IPreventsShroudReset for GpsWatcher {
    fn prevent_shroud_reset(&self, _self_: &ActorRef) -> bool {
        self.granted.get() || self.granted_allies.get()
    }
}

pub fn gps_watcher_of(p: &PlayerRef) -> Option<Rc<GpsWatcher>> {
    p.player_actor().trait_or_default::<GpsWatcher>()
}

impl GpsWatcher {
    pub fn granted(&self) -> bool {
        self.granted.get()
    }

    pub fn granted_allies(&self) -> bool {
        self.granted_allies.get()
    }

    pub fn gps_remove(&self, atek: &ActorRef) {
        let pos = self.actors.borrow().iter().position(|a| a.id == atek.id);
        if let Some(i) = pos {
            self.actors.borrow_mut().remove(i);
        }
        self.refresh_gps(&atek.owner());
    }

    pub fn gps_add(&self, atek: &ActorRef) {
        self.actors.borrow_mut().push(atek.clone());
        self.refresh_gps(&atek.owner());
    }

    pub fn reached_orbit(&self, launcher: &PlayerRef) {
        self.launched.set(true);
        self.refresh_gps(launcher);
    }

    pub fn refresh_gps(&self, launcher: &PlayerRef) {
        self.refresh_granted();
        for (_, w) in launcher.player_actor().world().actors_with_trait::<GpsWatcher>() {
            w.refresh_granted();
        }
    }

    fn refresh_granted(&self) {
        let (was_granted, was_granted_allies) = (self.granted.get(), self.granted_allies.get());
        let world = self.owner.player_actor().world();
        let allies: Vec<Rc<GpsWatcher>> = world
            .actors_with_trait::<GpsWatcher>()
            .into_iter()
            .filter(|(a, _)| a.owner().is_allied_with(Some(&self.owner)))
            .map(|(_, w)| w)
            .collect();
        self.granted.set(!self.actors.borrow().is_empty() && self.launched.get());
        self.granted_allies.set(allies.iter().any(|w| w.granted.get()));
        if !self.explored.get() && (self.launched.get() || allies.iter().any(|w| w.launched.get())) {
            self.explored.set(true);
            self.owner.shroud().explore_all();
        }
        if was_granted != self.granted.get() || was_granted_allies != self.granted_allies.get() {
            let list = self.notify_on_refresh.borrow().clone();
            for (a, t) in list {
                t.on_gps_refresh(&a, &self.owner);
            }
        }
    }

    pub fn register_for_on_gps_refreshed(&self, actor: &ActorRef, t: Rc<dyn IOnGpsRefreshed>) {
        let mut l = self.notify_on_refresh.borrow_mut();
        if !l.iter().any(|(a, x)| a.id == actor.id && Rc::ptr_eq(x, &t)) {
            l.push((actor.clone(), t));
        }
    }

    pub fn unregister_for_on_gps_refreshed(&self, actor: &ActorRef, t: &Rc<dyn IOnGpsRefreshed>) {
        self.notify_on_refresh
            .borrow_mut()
            .retain(|(a, x)| !(a.id == actor.id && Rc::ptr_eq(x, t)));
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct FrozenUnderFogUpdatedByGpsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

/// `FrozenUnderFogUpdatedByGps`. Its `Refresh` action only acts on frozen actors with renderables, which the C#
/// renderer fills; see [`rustora_engine::frozen::FrozenActor::has_renderables`].
pub struct FrozenUnderFogUpdatedByGps {
    self_: ActorRef,
    me: Weak<FrozenUnderFogUpdatedByGps>,
}

register_traits!(FrozenUnderFogUpdatedByGps: INotifyOwnerChanged, INotifyActorDisposing, IOnGpsRefreshed);
info_impl!(FrozenUnderFogUpdatedByGpsInfo => |_, init| {
    let t = Rc::new_cyclic(|me| FrozenUnderFogUpdatedByGps { self_: init.self_.clone(), me: me.clone() });
    let me: Rc<dyn IOnGpsRefreshed> = t.clone();
    for p in init.world().players() {
        gps_watcher_of(&p).expect("GpsWatcher").register_for_on_gps_refreshed(&init.self_, me.clone());
    }
    Some(t)
});

impl FrozenUnderFogUpdatedByGps {
    /// The `Refresh` action: updates the frozen actor of `p` so that owner changes can be seen.
    fn refresh_for(&self, p: &PlayerRef) {
        let Some(layer) = p.frozen_actor_layer() else { return };
        let Some(gps) = gps_watcher_of(p) else { return };
        if !gps.granted() || !gps.granted_allies() {
            return;
        }
        let Some(fa) = layer.from_id(self.self_.id) else { return };
        if fa.has_renderables() {
            fa.refresh_state();
            fa.request_renderables();
        }
    }

    fn remove_for(&self, p: &PlayerRef) {
        let Some(layer) = p.frozen_actor_layer() else { return };
        let Some(gps) = gps_watcher_of(p) else { return };
        if !gps.granted() || !gps.granted_allies() {
            return;
        }
        let Some(fa) = layer.from_id(self.self_.id) else { return };
        fa.invalidate();
        layer.remove(&fa);
        let me: Rc<dyn IOnGpsRefreshed> = self.me.upgrade().expect("trait");
        gps.unregister_for_on_gps_refreshed(&self.self_, &me);
    }
}

impl INotifyOwnerChanged for FrozenUnderFogUpdatedByGps {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, _new: &PlayerRef) {
        for p in self_.world().players() {
            self.refresh_for(&p);
        }
    }
}

impl INotifyActorDisposing for FrozenUnderFogUpdatedByGps {
    fn disposing(&self, self_: &ActorRef) {
        for p in self_.world().players() {
            self.remove_for(&p);
        }
    }
}

impl IOnGpsRefreshed for FrozenUnderFogUpdatedByGps {
    fn on_gps_refresh(&self, self_: &ActorRef, player: &PlayerRef) {
        if self_.is_dead() {
            self.remove_for(player);
        } else {
            self.refresh_for(player);
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ProvidesRadarInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
}

pub struct ProvidesRadar {
    state: ConditionalState,
}

register_traits!(ProvidesRadar: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ProvidesRadarInfo => |info, _| Some(Rc::new(ProvidesRadar { state: ConditionalState::new(&info.base) })));

rustora_engine::conditional_simple!(ProvidesRadar);

impl ProvidesRadar {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

/// `SatelliteLaunch`: the door animation decides how long the effect lives.
pub struct SatelliteLaunch {
    launcher_owner: PlayerRef,
    reveal_delay: i32,
    doors: Rc<Animation>,
    pos: WPos,
    frame: Cell<i32>,
}

impl SatelliteLaunch {
    pub fn new(world: &World, launcher: &ActorRef, door_image: &str, door_sequence: &str, reveal_delay: i32) -> EffectRef {
        let doors = Animation::new(world.map().sequences().clone(), door_image, None, None);
        let e = Rc::new(SatelliteLaunch {
            launcher_owner: launcher.owner(),
            reveal_delay,
            doors: doors.clone(),
            pos: launcher.center_position(),
            frame: Cell::new(0),
        });
        let weak = Rc::downgrade(&e);
        let w = Rc::downgrade(&launcher.world());
        doors.play_then(
            door_sequence,
            Some(Rc::new(move || {
                let (Some(world), weak) = (w.upgrade(), weak.clone()) else { return };
                world.add_frame_end_task(Box::new(move |w| {
                    if let Some(e) = weak.upgrade() {
                        let e: EffectRef = e;
                        w.remove_effect(&e);
                    }
                }));
            })),
        );
        e
    }
}

impl Effect for SatelliteLaunch {
    fn tick(&self, world: &Rc<World>) {
        self.doors.tick();
        self.frame.set(self.frame.get() + 1);
        if self.frame.get() == 19 {
            let (owner, pos, delay) = (self.launcher_owner.clone(), self.pos, self.reveal_delay);
            world.add_frame_end_task(Box::new(move |w| w.add_effect(GpsSatellite::new(pos, delay, owner))));
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `GpsSatellite`.
pub struct GpsSatellite {
    me: Weak<GpsSatellite>,
    launcher: PlayerRef,
    reveal_delay: i32,
    pos: Cell<WPos>,
    tick: Cell<i32>,
}

impl GpsSatellite {
    pub fn new(pos: WPos, reveal_delay: i32, launcher: PlayerRef) -> EffectRef {
        Rc::new_cyclic(|me| GpsSatellite {
            me: me.clone(),
            launcher,
            reveal_delay,
            pos: Cell::new(pos),
            tick: Cell::new(0),
        })
    }
}

impl Effect for GpsSatellite {
    fn tick(&self, world: &Rc<World>) {
        self.pos.set(self.pos.get() + WVec::new(0, 0, 427));
        self.tick.set(self.tick.get() + 1);
        if self.tick.get() > self.reveal_delay {
            gps_watcher_of(&self.launcher).expect("GpsWatcher").reached_orbit(&self.launcher);
            let me: EffectRef = self.me.upgrade().expect("effect");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
