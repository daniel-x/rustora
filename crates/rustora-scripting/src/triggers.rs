// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ScriptTriggers.cs`: the Lua callbacks of an actor's events.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use mlua::{Function, IntoLuaMulti};
use rustora_core::world_types::CPos;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::ActorInits;
use rustora_engine::bitset::BitSet;
use rustora_engine::damage::AttackInfo;
use rustora_engine::player::PlayerRef;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, register_traits};
use rustora_mods_common::capture::CaptureType;
use rustora_mods_common::interfaces::*;
use rustora_mods_common::player::objectives::{INotifyObjectivesUpdated, INotifyTimeLimit, INotifyWinStateChanged};
use rustora_mods_common::sound::INotifyDiscovered;

use crate::context::ScriptContext;
use crate::values::{LuaActor, LuaPlayer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    OnIdle,
    OnDamaged,
    OnKilled,
    OnProduction,
    OnOtherProduction,
    OnPlayerWon,
    OnPlayerLost,
    OnObjectiveAdded,
    OnObjectiveCompleted,
    OnObjectiveFailed,
    OnCapture,
    OnInfiltrated,
    OnAddedToWorld,
    OnRemovedFromWorld,
    OnDiscovered,
    OnPlayerDiscovered,
    OnPassengerEntered,
    OnPassengerExited,
    OnSold,
    OnTimerExpired,
}

impl Trigger {
    pub const ALL: [Trigger; 20] = [
        Trigger::OnIdle,
        Trigger::OnDamaged,
        Trigger::OnKilled,
        Trigger::OnProduction,
        Trigger::OnOtherProduction,
        Trigger::OnPlayerWon,
        Trigger::OnPlayerLost,
        Trigger::OnObjectiveAdded,
        Trigger::OnObjectiveCompleted,
        Trigger::OnObjectiveFailed,
        Trigger::OnCapture,
        Trigger::OnInfiltrated,
        Trigger::OnAddedToWorld,
        Trigger::OnRemovedFromWorld,
        Trigger::OnDiscovered,
        Trigger::OnPlayerDiscovered,
        Trigger::OnPassengerEntered,
        Trigger::OnPassengerExited,
        Trigger::OnSold,
        Trigger::OnTimerExpired,
    ];

    /// `Enum.Parse(typeof(Trigger), name)`.
    pub fn parse(name: &str) -> Option<Trigger> {
        Trigger::ALL.into_iter().find(|t| format!("{t:?}") == name)
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ScriptTriggersInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

info_impl!(ScriptTriggersInfo => |_info, init| Some(Rc::new(ScriptTriggers {
    world: Rc::downgrade(&init.world()),
    self_: Rc::downgrade(&init.self_),
    triggerables: RefCell::new(vec![Vec::new(); Trigger::ALL.len()]),
    on_killed_internal: RefCell::default(),
    on_captured_internal: RefCell::default(),
    on_removed_internal: RefCell::default(),
    on_added_internal: RefCell::default(),
    on_produced_internal: RefCell::default(),
    on_other_produced_internal: RefCell::default(),
})));

#[derive(Clone)]
struct Triggerable {
    function: Function,
    context: Weak<ScriptContext>,
}

pub type ActorEvent = Rc<dyn Fn(&ActorRef)>;
pub type ActorPairEvent = Rc<dyn Fn(&ActorRef, &ActorRef)>;

pub struct ScriptTriggers {
    world: Weak<World>,
    self_: Weak<rustora_engine::actor::Actor>,
    triggerables: RefCell<Vec<Vec<Triggerable>>>,
    pub on_killed_internal: RefCell<Vec<ActorEvent>>,
    pub on_captured_internal: RefCell<Vec<ActorEvent>>,
    pub on_removed_internal: RefCell<Vec<ActorEvent>>,
    pub on_added_internal: RefCell<Vec<ActorEvent>>,
    pub on_produced_internal: RefCell<Vec<ActorPairEvent>>,
    pub on_other_produced_internal: RefCell<Vec<ActorPairEvent>>,
}

register_traits!(ScriptTriggers: INotifyIdle, INotifyDamage, INotifyKilled, INotifyProduction, INotifyOtherProduction,
    INotifyObjectivesUpdated, INotifyCapture, INotifyInfiltrated, INotifyAddedToWorld, INotifyRemovedFromWorld,
    INotifyDiscovered, INotifyActorDisposing, INotifyPassengerEntered, INotifyPassengerExited, INotifySold,
    INotifyWinStateChanged, INotifyTimeLimit);

fn fire(events: &RefCell<Vec<ActorEvent>>, a: &ActorRef) {
    let events = events.borrow().clone();
    for e in events {
        e(a);
    }
}

fn fire_pair(events: &RefCell<Vec<ActorPairEvent>>, a: &ActorRef, b: &ActorRef) {
    let events = events.borrow().clone();
    for e in events {
        e(a, b);
    }
}

fn actor(a: &ActorRef) -> LuaActor {
    LuaActor(a.clone())
}

fn player(p: &PlayerRef) -> LuaPlayer {
    LuaPlayer(p.clone())
}

fn opt_actor(a: &Option<ActorRef>) -> Option<LuaActor> {
    a.as_ref().map(actor)
}

impl ScriptTriggers {
    pub fn register_callback(&self, trigger: Trigger, function: Function, context: &Rc<ScriptContext>) {
        self.triggerables.borrow_mut()[trigger as usize].push(Triggerable {
            function,
            context: Rc::downgrade(context),
        });
    }

    pub fn has_any_callbacks_for(&self, trigger: Trigger) -> bool {
        !self.triggerables.borrow()[trigger as usize].is_empty()
    }

    fn world_disposing(&self) -> bool {
        self.world.upgrade().is_none_or(|w| w.disposing.get())
    }

    /// Calls the callbacks of `trigger` with the arguments from `args`. Returns `false` when a callback failed,
    /// which stops the dispatch as the C# `return` in the `catch` block does.
    fn dispatch<A: IntoLuaMulti>(&self, trigger: Trigger, args: impl Fn() -> A) -> bool {
        if self.world_disposing() {
            return false;
        }
        if self.triggerables.borrow()[trigger as usize].is_empty() {
            return true;
        }
        let list = self.triggerables.borrow()[trigger as usize].clone();
        for f in list {
            let Some(context) = f.context.upgrade() else { continue };
            if !context.call(&f.function, args()) {
                return false;
            }
        }
        true
    }

    /// `Clear`: removes the callbacks at the end of the frame.
    pub fn clear(self: &Rc<Self>, trigger: Trigger) {
        let Some(world) = self.world.upgrade() else { return };
        let this = Rc::downgrade(self);
        world.add_frame_end_task(Box::new(move |_| {
            if let Some(t) = this.upgrade() {
                t.triggerables.borrow_mut()[trigger as usize].clear();
            }
        }));
    }

    pub fn clear_all(self: &Rc<Self>) {
        for t in Trigger::ALL {
            self.clear(t);
        }
    }

    fn rc(&self) -> Option<Rc<ScriptTriggers>> {
        self.self_.upgrade().and_then(|a| a.trait_or_default::<ScriptTriggers>())
    }
}

impl INotifyIdle for ScriptTriggers {
    fn tick_idle(&self, self_: &ActorRef) {
        self.dispatch(Trigger::OnIdle, || actor(self_));
    }
}

impl INotifyDamage for ScriptTriggers {
    fn damaged(&self, self_: &ActorRef, e: &AttackInfo) {
        self.dispatch(Trigger::OnDamaged, || (actor(self_), opt_actor(&e.attacker), e.damage.value));
    }
}

impl INotifyKilled for ScriptTriggers {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        if self.world_disposing() {
            return;
        }
        if self.dispatch(Trigger::OnKilled, || (actor(self_), opt_actor(&e.attacker))) {
            fire(&self.on_killed_internal, self_);
        }
    }
}

impl INotifyProduction for ScriptTriggers {
    fn unit_produced(&self, self_: &ActorRef, other: &ActorRef, _exit: CPos) {
        if self.world_disposing() {
            return;
        }
        if self.dispatch(Trigger::OnProduction, || (actor(self_), actor(other))) {
            fire_pair(&self.on_produced_internal, self_, other);
        }
    }
}

impl INotifyWinStateChanged for ScriptTriggers {
    fn on_player_won(&self, p: &PlayerRef) {
        self.dispatch(Trigger::OnPlayerWon, || player(p));
    }

    fn on_player_lost(&self, p: &PlayerRef) {
        self.dispatch(Trigger::OnPlayerLost, || player(p));
    }
}

impl INotifyObjectivesUpdated for ScriptTriggers {
    fn on_objective_added(&self, p: &PlayerRef, id: usize) {
        self.dispatch(Trigger::OnObjectiveAdded, || (player(p), id as i32));
    }

    fn on_objective_completed(&self, p: &PlayerRef, id: usize) {
        self.dispatch(Trigger::OnObjectiveCompleted, || (player(p), id as i32));
    }

    fn on_objective_failed(&self, p: &PlayerRef, id: usize) {
        self.dispatch(Trigger::OnObjectiveFailed, || (player(p), id as i32));
    }
}

impl INotifyCapture for ScriptTriggers {
    fn on_capture(&self, self_: &ActorRef, captor: &ActorRef, old_owner: &PlayerRef, new_owner: &PlayerRef, _types: BitSet<CaptureType>) {
        if self.world_disposing() {
            return;
        }
        if self.dispatch(Trigger::OnCapture, || {
            (actor(self_), actor(captor), player(old_owner), player(new_owner))
        }) {
            fire(&self.on_captured_internal, self_);
        }
    }
}

impl INotifyInfiltrated for ScriptTriggers {
    fn infiltrated(&self, self_: &ActorRef, infiltrator: &ActorRef, _types: BitSet<rustora_engine::terrain::TargetableType>) {
        self.dispatch(Trigger::OnInfiltrated, || (actor(self_), actor(infiltrator)));
    }
}

impl INotifyAddedToWorld for ScriptTriggers {
    fn added_to_world(&self, self_: &ActorRef) {
        if self.world_disposing() {
            return;
        }
        if self.dispatch(Trigger::OnAddedToWorld, || actor(self_)) {
            fire(&self.on_added_internal, self_);
        }
    }
}

impl INotifyRemovedFromWorld for ScriptTriggers {
    fn removed_from_world(&self, self_: &ActorRef) {
        if self.world_disposing() {
            return;
        }
        if self.dispatch(Trigger::OnRemovedFromWorld, || actor(self_)) {
            fire(&self.on_removed_internal, self_);
        }
    }
}

impl INotifySold for ScriptTriggers {
    fn selling(&self, _self_: &ActorRef) {}

    fn sold(&self, self_: &ActorRef) {
        self.dispatch(Trigger::OnSold, || actor(self_));
    }
}

impl INotifyOtherProduction for ScriptTriggers {
    fn unit_produced_by_other(
        &self,
        _self_: &ActorRef,
        producer: &ActorRef,
        produced: &ActorRef,
        production_type: &str,
        _init: &ActorInits,
    ) {
        if self.world_disposing() {
            return;
        }
        if self.dispatch(Trigger::OnOtherProduction, || {
            (actor(producer), actor(produced), production_type.to_string())
        }) {
            fire_pair(&self.on_other_produced_internal, producer, produced);
        }
    }
}

impl INotifyDiscovered for ScriptTriggers {
    fn on_discovered(&self, self_: &ActorRef, discoverer: &PlayerRef, _play_notification: bool) {
        if !self.dispatch(Trigger::OnDiscovered, || (actor(self_), player(discoverer))) {
            return;
        }
        self.dispatch(Trigger::OnPlayerDiscovered, || {
            (player(&self_.owner()), player(discoverer), actor(self_))
        });
    }
}

impl INotifyPassengerEntered for ScriptTriggers {
    fn on_passenger_entered(&self, self_: &ActorRef, passenger: &ActorRef) {
        self.dispatch(Trigger::OnPassengerEntered, || (actor(self_), actor(passenger)));
    }
}

impl INotifyPassengerExited for ScriptTriggers {
    fn on_passenger_exited(&self, self_: &ActorRef, passenger: &ActorRef) {
        self.dispatch(Trigger::OnPassengerExited, || (actor(self_), actor(passenger)));
    }
}

impl INotifyTimeLimit for ScriptTriggers {
    fn notify_timer_expired(&self, _self_: &ActorRef) {
        self.dispatch(Trigger::OnTimerExpired, || ());
    }
}

impl INotifyActorDisposing for ScriptTriggers {
    fn disposing(&self, _self_: &ActorRef) {
        if let Some(t) = self.rc() {
            t.clear_all();
        }
    }
}

/// Triggers hold Lua functions, which save games cannot keep: saving fails if any trigger is registered.
impl rustora_engine::persist::PersistState for ScriptTriggers {
    fn save_state(&self, _w: &mut rustora_engine::persist::SaveWriter) {
        let empty = self.triggerables.borrow().iter().all(Vec::is_empty)
            && self.on_killed_internal.borrow().is_empty()
            && self.on_captured_internal.borrow().is_empty()
            && self.on_removed_internal.borrow().is_empty()
            && self.on_added_internal.borrow().is_empty()
            && self.on_produced_internal.borrow().is_empty()
            && self.on_other_produced_internal.borrow().is_empty();
        assert!(empty, "save game: an actor has Lua triggers");
    }
    fn load_state(&self, _r: &mut rustora_engine::persist::SaveReader) {}
}
