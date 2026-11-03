// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Actor.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use rustora_core::expression::Variables;
use rustora_core::world_types::{CPos, WPos, WRot};

use crate::activity::{self, ActivityRef};
use crate::actor_info::ActorInfo;
use crate::actor_init::{ActorInitializer, ActorInits};
use crate::player::PlayerRef;
use crate::traits::*;
use crate::world::World;

pub type ActorRef = Rc<Actor>;

pub const INVALID_CONDITION_TOKEN: i32 = 0;

#[derive(Default)]
struct ConditionState {
    notifiers: Vec<VariableObserverNotifier>,
    tokens: IndexSet<i32>,
}

/// Read-only view on an actor's condition counts, passed to condition observers.
pub struct ConditionCache<'a>(pub &'a Actor);

impl Variables for ConditionCache<'_> {
    fn get(&self, name: &str) -> i32 {
        self.0.conditions.borrow().cache.get(name).copied().unwrap_or(0)
    }
}

#[derive(Default)]
struct Conditions {
    states: HashMap<String, ConditionState>,
    tokens: HashMap<i32, String>,
    next_token: i32,
    cache: IndexMap<String, i32>,
}

/// Traits cached at creation for fast access (`OccupiesSpace`, `facing`, ...).
#[derive(Default)]
pub struct ActorCache {
    pub occupies_space: Option<Rc<dyn IOccupySpace>>,
    pub effective_owner: Option<Rc<dyn IEffectiveOwner>>,
    pub facing: Option<Rc<dyn IFacing>>,
    pub health: Option<Rc<dyn IHealth>>,
    pub resolve_orders: Vec<Rc<dyn IResolveOrder>>,
    pub visibility_modifiers: Vec<Rc<dyn IVisibilityModifier>>,
    pub default_visibility: Option<Rc<dyn IDefaultVisibility>>,
    pub becoming_idles: Vec<Rc<dyn INotifyBecomingIdle>>,
    pub tick_idles: Vec<Rc<dyn INotifyIdle>>,
    pub crushables: Vec<Rc<dyn ICrushable>>,
    pub sync_hashes: Vec<Rc<dyn ISync>>,
    pub targetables: Vec<Rc<dyn crate::actor_ext::ITargetable>>,
    pub targetable_positions: Vec<Rc<dyn crate::actor_ext::ITargetablePositions>>,
    /// C# class names of the sync traits, for diagnostics.
    pub sync_names: Vec<&'static str>,
}

pub struct Actor {
    pub id: u32,
    pub info: Rc<ActorInfo>,
    world: Weak<World>,
    owner: RefCell<Option<PlayerRef>>,
    is_in_world: Cell<bool>,
    will_dispose: Cell<bool>,
    disposed: Cell<bool>,
    created: Cell<bool>,
    current_activity: RefCell<Option<ActivityRef>>,
    pub generation: Cell<i32>,
    pub replaced_by_actor: RefCell<Option<ActorRef>>,
    conditions: RefCell<Conditions>,
    cache: OnceCell<ActorCache>,
    /// `OccupiesSpace`, available while later traits are being constructed.
    occupies_early: OnceCell<Rc<dyn IOccupySpace>>,
}

impl std::fmt::Debug for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.info.name, self.id)
    }
}

impl PartialEq for Actor {
    fn eq(&self, o: &Actor) -> bool {
        self.id == o.id
    }
}

impl Actor {
    /// `new Actor(world, name, initDict)`: assigns the next actor ID and creates the traits.
    pub fn new(world: &Rc<World>, name: &str, inits: ActorInits) -> ActorRef {
        let name = name.to_lowercase();
        let info = world
            .rules()
            .actor(&name)
            .unwrap_or_else(|| panic!("No rules definition for unit {name}"));
        let actor = Rc::new(Actor {
            id: world.next_aid(),
            info: info.clone(),
            world: Rc::downgrade(world),
            owner: RefCell::new(None),
            is_in_world: Cell::new(false),
            will_dispose: Cell::new(false),
            disposed: Cell::new(false),
            created: Cell::new(false),
            current_activity: RefCell::new(None),
            generation: Cell::new(0),
            replaced_by_actor: RefCell::new(None),
            conditions: RefCell::new(Conditions {
                next_token: 1,
                ..Default::default()
            }),
            cache: OnceCell::new(),
            occupies_early: OnceCell::new(),
        });

        let init = ActorInitializer::new(actor.clone(), inits);
        if let Some(owner) = init.owner(world) {
            *actor.owner.borrow_mut() = Some(owner);
        }

        let dict = world.trait_dict();
        let mut sync_names = Vec::new();
        for trait_info in info.traits_in_construct_order() {
            if let Some(t) = trait_info.clone().create(&init) {
                let before = dict.with_interface::<dyn ISync>(&actor).len();
                let name = t.type_name();
                t.register(dict, &actor);
                if actor.occupies_early.get().is_none()
                    && let Some(o) = dict.get_or_default::<dyn IOccupySpace>(&actor)
                {
                    let _ = actor.occupies_early.set(o);
                }
                if dict.with_interface::<dyn ISync>(&actor).len() > before {
                    sync_names.push(name);
                }
            }
        }

        let cache = ActorCache {
            occupies_space: dict.get_or_default::<dyn IOccupySpace>(&actor),
            effective_owner: dict.get_or_default::<dyn IEffectiveOwner>(&actor),
            facing: dict.get_or_default::<dyn IFacing>(&actor),
            health: dict.get_or_default::<dyn IHealth>(&actor),
            resolve_orders: dict.with_interface::<dyn IResolveOrder>(&actor),
            visibility_modifiers: dict.with_interface::<dyn IVisibilityModifier>(&actor),
            default_visibility: dict.get_or_default::<dyn IDefaultVisibility>(&actor),
            becoming_idles: dict.with_interface::<dyn INotifyBecomingIdle>(&actor),
            tick_idles: dict.with_interface::<dyn INotifyIdle>(&actor),
            crushables: dict.with_interface::<dyn ICrushable>(&actor),
            sync_hashes: dict.with_interface::<dyn ISync>(&actor),
            targetables: dict.with_interface::<dyn crate::actor_ext::ITargetable>(&actor),
            targetable_positions: dict.with_interface::<dyn crate::actor_ext::ITargetablePositions>(&actor),
            sync_names,
        };
        let _ = actor.cache.set(cache);
        actor
    }

    pub fn world(&self) -> Rc<World> {
        self.world.upgrade().expect("world alive")
    }

    pub fn cache(&self) -> &ActorCache {
        self.cache.get().expect("actor initialized")
    }

    pub fn cache_opt(&self) -> Option<&ActorCache> {
        self.cache.get()
    }

    pub fn owner(&self) -> PlayerRef {
        self.owner.borrow().clone().expect("actor has an owner")
    }

    pub fn owner_opt(&self) -> Option<PlayerRef> {
        self.owner.borrow().clone()
    }

    pub(crate) fn set_owner(&self, p: Option<PlayerRef>) {
        *self.owner.borrow_mut() = p;
    }

    pub fn is_in_world(&self) -> bool {
        self.is_in_world.get()
    }

    pub(crate) fn set_in_world(&self, v: bool) {
        self.is_in_world.set(v);
    }

    pub fn will_dispose(&self) -> bool {
        self.will_dispose.get()
    }

    pub fn disposed(&self) -> bool {
        self.disposed.get()
    }

    pub fn is_dead(&self) -> bool {
        self.disposed.get() || self.cache.get().and_then(|c| c.health.as_ref()).is_some_and(|h| h.is_dead())
    }

    pub fn location(&self) -> CPos {
        self.occupies_early.get().expect("IOccupySpace").top_left()
    }

    pub fn center_position(&self) -> WPos {
        self.occupies_early.get().expect("IOccupySpace").center_position()
    }

    pub fn orientation(&self) -> WRot {
        self.cache().facing.as_ref().map_or(WRot::NONE, |f| f.orientation())
    }

    pub fn current_activity(&self) -> Option<ActivityRef> {
        activity::skip_done(self.current_activity.borrow().clone())
    }

    fn set_current_activity(&self, a: Option<ActivityRef>) {
        *self.current_activity.borrow_mut() = a;
    }

    pub fn is_idle(&self) -> bool {
        self.current_activity().is_none()
    }

    pub fn trait_<T: ?Sized + 'static>(&self) -> Rc<T> {
        assert!(!self.disposed.get(), "Attempted to get trait from destroyed object ({self:?})");
        self.world().trait_dict().get::<T>(self)
    }

    pub fn trait_or_default<T: ?Sized + 'static>(&self) -> Option<Rc<T>> {
        assert!(!self.disposed.get(), "Attempted to get trait from destroyed object ({self:?})");
        self.world().trait_dict().get_or_default::<T>(self)
    }

    pub fn traits_implementing<T: ?Sized + 'static>(&self) -> Vec<Rc<T>> {
        assert!(!self.disposed.get(), "Attempted to get trait from destroyed object ({self:?})");
        self.world().trait_dict().with_interface::<T>(self)
    }

    /// `Actor.Initialize`: runs `INotifyCreated`, initializes condition observers, queues the
    /// creation activity and optionally adds the actor to the world.
    pub fn initialize(self: &ActorRef, add_to_world: bool) {
        self.created.set(true);
        for t in self.traits_implementing::<dyn INotifyCreated>() {
            t.created(self);
        }

        let mut all_notifiers: Vec<VariableObserverNotifier> = Vec::new();
        for provider in self.traits_implementing::<dyn IObservesVariables>() {
            for user in provider.get_variable_observers() {
                if !all_notifiers.iter().any(|n| Rc::ptr_eq(n, &user.notifier)) {
                    all_notifiers.push(user.notifier.clone());
                }
                for variable in &user.variables {
                    let mut c = self.conditions.borrow_mut();
                    let count = {
                        let cs = c.states.entry(variable.clone()).or_default();
                        cs.notifiers.push(user.notifier.clone());
                        cs.tokens.len() as i32
                    };
                    c.cache.insert(variable.clone(), count);
                }
            }
        }

        for notify in &all_notifiers {
            notify(self, &ConditionCache(self));
        }

        let mut creation: Option<&'static str> = None;
        for ica in self.traits_implementing::<dyn ICreationActivity>() {
            if !ica.is_trait_enabled() {
                continue;
            }
            assert!(creation.is_none(), "More than one enabled ICreationActivity trait");
            let Some(act) = ica.get_creation_activity() else { continue };
            creation = Some("creation");
            activity::queue(&act, self.current_activity());
            self.set_current_activity(Some(act));
        }

        if add_to_world {
            self.world().add(self);
        }
    }

    pub fn tick(self: &ActorRef) {
        let was_idle = self.is_idle();
        let next = activity::run_activity(self, self.current_activity());
        self.set_current_activity(next);

        if !was_idle && self.is_idle() {
            for n in self.cache().becoming_idles.clone() {
                n.on_becoming_idle(self);
            }
            let next = activity::run_activity(self, self.current_activity());
            self.set_current_activity(next);
        } else if was_idle {
            for t in self.cache().tick_idles.clone() {
                t.tick_idle(self);
            }
        }
    }

    pub fn queue_activity_queued(self: &ActorRef, queued: bool, next: ActivityRef) {
        if !queued {
            self.cancel_activity();
        }
        self.queue_activity(next);
    }

    pub fn queue_activity(self: &ActorRef, next: ActivityRef) {
        assert!(self.created.get(), "An activity was queued before the actor was created.");
        match self.current_activity() {
            None => self.set_current_activity(Some(next)),
            Some(cur) => activity::queue(&cur, Some(next)),
        }
    }

    pub fn cancel_activity(self: &ActorRef) {
        if let Some(a) = self.current_activity() {
            activity::cancel(&a, self, false);
        }
    }

    /// `Actor.Dispose`: runs activity disposal hooks now and removes the actor at frame end.
    pub fn dispose(self: &ActorRef) {
        if let Some(a) = self.current_activity() {
            activity::on_actor_dispose_outer(&a, self);
        }
        self.will_dispose.set(true);
        let me = self.clone();
        self.world().add_frame_end_task(Box::new(move |w: &Rc<World>| {
            if me.disposed.get() {
                return;
            }
            if me.is_in_world.get() {
                w.remove(&me);
            }
            for t in me.traits_implementing::<dyn INotifyActorDisposing>() {
                t.disposing(&me);
            }
            w.trait_dict().remove_actor(me.id);
            me.disposed.set(true);
        }));
    }

    pub fn resolve_order(self: &ActorRef, order: &crate::order::Order) {
        for r in self.cache().resolve_orders.clone() {
            r.resolve_order(self, order);
        }
    }

    pub fn change_owner(self: &ActorRef, new_owner: PlayerRef) {
        let me = self.clone();
        self.world()
            .add_frame_end_task(Box::new(move |_w: &Rc<World>| me.change_owner_sync(new_owner)));
    }

    /// Changes the owner immediately; only call from a frame end task.
    pub fn change_owner_sync(self: &ActorRef, new_owner: PlayerRef) {
        if self.disposed.get() {
            return;
        }
        let world = self.world();
        let old_owner = self.owner();
        let was_in_world = self.is_in_world.get();
        if was_in_world {
            world.remove(self);
        }
        self.set_owner(Some(new_owner.clone()));
        self.generation.set(self.generation.get() + 1);
        for t in self.traits_implementing::<dyn INotifyOwnerChanged>() {
            t.on_owner_changed(self, &old_owner, &new_owner);
        }
        for t in world.world_actor().traits_implementing::<dyn INotifyOwnerChanged>() {
            t.on_owner_changed(self, &old_owner, &new_owner);
        }
        if was_in_world {
            world.add(self);
        }
    }

    pub fn can_be_viewed_by_player(self: &ActorRef, player: Option<&PlayerRef>) -> bool {
        for m in &self.cache().visibility_modifiers {
            if !m.is_visible(self, player) {
                return false;
            }
        }
        self.cache()
            .default_visibility
            .as_ref()
            .expect("IDefaultVisibility")
            .is_visible(self, player)
    }

    fn update_condition_state(self: &ActorRef, condition: &str, token: i32, is_revoke: bool) {
        let notifiers = {
            let mut c = self.conditions.borrow_mut();
            let count = {
                let cs = c.states.entry(condition.to_string()).or_default();
                if is_revoke {
                    cs.tokens.shift_remove(&token);
                } else {
                    cs.tokens.insert(token);
                }
                cs.tokens.len() as i32
            };
            c.cache.insert(condition.to_string(), count);
            if !self.created.get() {
                return;
            }
            c.states[condition].notifiers.clone()
        };
        for n in notifiers {
            n(self, &ConditionCache(self));
        }
    }

    /// Grants `condition` and returns the token needed to revoke it.
    pub fn grant_condition(self: &ActorRef, condition: &str) -> i32 {
        if condition.is_empty() {
            return INVALID_CONDITION_TOKEN;
        }
        let token = {
            let mut c = self.conditions.borrow_mut();
            let t = c.next_token;
            c.next_token += 1;
            c.tokens.insert(t, condition.to_string());
            t
        };
        self.update_condition_state(condition, token, false);
        token
    }

    pub fn revoke_condition(self: &ActorRef, token: i32) -> i32 {
        let condition = self
            .conditions
            .borrow_mut()
            .tokens
            .remove(&token)
            .unwrap_or_else(|| panic!("Attempting to revoke condition with invalid token {token} for {self:?}."));
        self.update_condition_state(&condition, token, true);
        INVALID_CONDITION_TOKEN
    }

    pub fn token_valid(&self, token: i32) -> bool {
        self.conditions.borrow().tokens.contains_key(&token)
    }

    /// `Sync.HashActor`.
    pub fn sync_hash_id(&self) -> i32 {
        (self.id << 16) as i32
    }
}

/// `Sync.HashActor` for optional actors.
pub fn hash_actor(a: Option<&ActorRef>) -> i32 {
    a.map_or(0, |a| a.sync_hash_id())
}
