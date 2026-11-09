// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Binary serialization of the simulation state for save games.
//!
//! [`Persist`] writes and reads values. [`PersistState`] writes the mutable state of an object that the loader
//! recreates first, such as a trait, and reads it back in place. References keep their meaning across a save:
//! actors are written as ids, players as indices, traits as the id of their actor and their index in the
//! actor's trait list, rule objects as their names, and shared objects such as activities and effects keep their
//! identity through an object table ([`RcPersist`]).
//!
//! `#[derive(Persist)]` of `rustora_derive` implements these traits for structs and enums.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use rustora_core::fieldloader::{Color, Float2, Float3, Rectangle, Size};
use rustora_core::world_types::{CPos, CVec, Int2, MPos, MersenneTwister, PPos, WAngle, WDist, WPos, WRot, WVec};

use crate::actor::{Actor, ActorRef};
use crate::actor_info::ActorInfo;
use crate::player::{Player, PlayerRef};
use crate::world::World;

pub use rustora_derive::Persist;

/// A value that can be written to a save game and read back.
pub trait Persist: Sized {
    fn save(&self, w: &mut SaveWriter);
    fn load(r: &mut SaveReader) -> Self;
}

/// The mutable state of an object that the loader creates before it reads the state.
pub trait PersistState {
    fn save_state(&self, w: &mut SaveWriter);
    fn load_state(&self, r: &mut SaveReader);
}

/// Marks types whose `Rc`s are owned by the object that holds them, such as the animations of a trait: an
/// `Rc<T>` field is then written in place like the value itself. Traits of other actors are not owned.
pub trait OwnedState: PersistState {}

impl<T: OwnedState + ?Sized> PersistState for Rc<T> {
    fn save_state(&self, w: &mut SaveWriter) {
        (**self).save_state(w)
    }
    fn load_state(&self, r: &mut SaveReader) {
        (**self).load_state(r)
    }
}

/// A function that save games can keep, such as the one that runs after an animation: a value that is
/// registered with [`persist_values!`] for `dyn SavedFn`.
pub trait SavedFn: PersistDyn {
    fn call(&self);
}

/// A function that does nothing.
pub struct NoOp;

impl Persist for NoOp {
    fn save(&self, _w: &mut SaveWriter) {}
    fn load(_r: &mut SaveReader) -> Self {
        NoOp
    }
}

impl SavedFn for NoOp {
    fn call(&self) {}
}

/// A condition that save games can keep, registered with [`persist_values!`] for `dyn SavedPredicate`.
pub trait SavedPredicate: PersistDyn {
    fn test(&self) -> bool;
}

/// A type that is loaded inside `Rc::new_cyclic`, so that it can hold a `Weak` reference to itself and its parts
/// can refer back to it.
pub trait PersistShared: Persist {
    fn load_shared(r: &mut SaveReader, me: &Weak<Self>) -> Self;
}

/// How `Rc<Self>` and `Weak<Self>` are written: as a reference to an object that exists elsewhere (traits, rules),
/// or as a shared object written once.
pub trait RcPersist: 'static {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter);
    fn load_rc(r: &mut SaveReader) -> Rc<Self>;
    fn load_weak(r: &mut SaveReader) -> Weak<Self> {
        Rc::downgrade(&Self::load_rc(r))
    }
}

/// Object safe access to [`Persist`], for trait objects such as `dyn Activity`.
pub trait PersistDyn {
    /// The key of the loader in the registry of the trait object type.
    fn persist_type(&self) -> &'static str;
    fn persist_save(&self, w: &mut SaveWriter);
}

impl<T: Persist + 'static> PersistDyn for T {
    fn persist_type(&self) -> &'static str {
        std::any::type_name::<T>()
    }
    fn persist_save(&self, w: &mut SaveWriter) {
        self.save(w)
    }
}

/// Wraps a field for the in place serialization of `#[derive(Persist)]`: fields whose type implements
/// [`PersistState`] use it ([`ProbeState`]), all others are skipped ([`ProbeNone`]).
pub struct Probe<'a, T: ?Sized>(pub &'a T);

pub trait ProbeState {
    fn probe_save(&self, w: &mut SaveWriter);
    fn probe_load(&self, r: &mut SaveReader);
}

impl<T: PersistState + ?Sized> ProbeState for Probe<'_, T> {
    fn probe_save(&self, w: &mut SaveWriter) {
        self.0.save_state(w)
    }
    fn probe_load(&self, r: &mut SaveReader) {
        self.0.load_state(r)
    }
}

pub trait ProbeNone {
    fn probe_save(&self, w: &mut SaveWriter);
    fn probe_load(&self, _r: &mut SaveReader) {}
}

impl<T: ?Sized> ProbeNone for &Probe<'_, T> {
    fn probe_save(&self, w: &mut SaveWriter) {
        if let Some(skipped) = &mut w.skipped {
            skipped.insert(std::any::type_name::<T>());
        }
    }
}

/// A value that save games cannot hold, such as a closure of a Lua script: saving it fails.
pub struct Unsaved<T: ?Sized>(pub Rc<T>);

impl<T: ?Sized> Clone for Unsaved<T> {
    fn clone(&self) -> Self {
        Unsaved(self.0.clone())
    }
}

impl<T: ?Sized> Persist for Unsaved<T> {
    fn save(&self, _w: &mut SaveWriter) {
        panic!("save game: the state holds a {}", std::any::type_name::<T>());
    }
    fn load(r: &mut SaveReader) -> Self {
        r.fail(format!("save game: {} cannot be loaded", std::any::type_name::<T>()))
    }
}

/// The error of a failed load, carried by the panic that [`SaveReader::fail`] raises.
#[derive(Debug)]
pub struct LoadFailure(pub String);

const TAG_REF: u8 = 1;
const TAG_NEW: u8 = 2;

/// The rule objects that references are written as names of.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RulePath {
    TraitInfo(String, u32),
    Weapon(String),
    Projectile(String),
    Warhead(String, u32),
}

fn addr<T: ?Sized>(rc: *const T) -> usize {
    rc as *const () as usize
}

/// Writes a save game.
pub struct SaveWriter {
    buf: Vec<u8>,
    world: Rc<World>,
    rules: HashMap<usize, RulePath>,
    objects: HashMap<usize, u32>,
    /// Keeps the written shared objects alive, so that their addresses stay unique while writing.
    keep: Vec<Box<dyn Any>>,
    actors_seen: HashSet<u32>,
    actor_queue: VecDeque<ActorRef>,
    /// With `RUSTORA_PERSIST_AUDIT` set, the types of the fields that were skipped because they have no state.
    pub skipped: Option<BTreeSet<&'static str>>,
    /// The actor whose state is being written.
    pub current_actor: Option<ActorRef>,
}

impl SaveWriter {
    pub fn new(world: &Rc<World>) -> SaveWriter {
        let mut rules = HashMap::new();
        for (name, info) in world.rules().actors() {
            for (i, t) in info.traits.iter().enumerate() {
                rules.insert(addr(Rc::as_ptr(t)), RulePath::TraitInfo(name.clone(), i as u32));
            }
        }
        for (name, w) in world.rules().weapons() {
            rules.insert(addr(Rc::as_ptr(w)), RulePath::Weapon(name.clone()));
            if let Some(p) = &w.projectile {
                rules.insert(addr(Rc::as_ptr(p)), RulePath::Projectile(name.clone()));
            }
            for (i, h) in w.warheads.iter().enumerate() {
                rules.insert(addr(Rc::as_ptr(h)), RulePath::Warhead(name.clone(), i as u32));
            }
        }
        SaveWriter {
            buf: Vec::new(),
            world: world.clone(),
            rules,
            objects: HashMap::new(),
            keep: Vec::new(),
            actors_seen: HashSet::new(),
            actor_queue: VecDeque::new(),
            skipped: std::env::var_os("RUSTORA_PERSIST_AUDIT").map(|_| BTreeSet::new()),
            current_actor: None,
        }
    }

    pub fn world(&self) -> &Rc<World> {
        &self.world
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Moves the bytes written so far out of the writer, keeping its tables.
    pub fn take_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn len_prefix(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("length fits in 32 bits"));
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn str(&mut self, s: &str) {
        self.len_prefix(s.len());
        self.bytes(s.as_bytes());
    }

    /// Writes an actor as its id and queues it to be written in full by [`SaveWriter::next_actor`].
    pub fn actor(&mut self, a: &ActorRef) {
        self.u32(a.id);
        if self.actors_seen.insert(a.id) {
            self.actor_queue.push_back(a.clone());
        }
    }

    /// Marks `a` as known without writing anything, for actors that are written anyway.
    pub fn queue_actor(&mut self, a: &ActorRef) {
        if self.actors_seen.insert(a.id) {
            self.actor_queue.push_back(a.clone());
        }
    }

    /// True if actors were referenced that are not yet written.
    pub fn next_actor_pending(&self) -> bool {
        !self.actor_queue.is_empty()
    }

    /// The number of shared objects written so far.
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// Writes a placeholder for the length of the bytes up to [`SaveWriter::end_len`].
    pub fn begin_len(&mut self) -> usize {
        self.u32(0);
        self.buf.len()
    }

    pub fn end_len(&mut self, start: usize) {
        let n = u32::try_from(self.buf.len() - start).expect("length fits in 32 bits");
        self.buf[start - 4..start].copy_from_slice(&n.to_le_bytes());
    }

    /// The next actor that was referenced but not yet written.
    pub fn next_actor(&mut self) -> Option<ActorRef> {
        self.actor_queue.pop_front()
    }

    pub fn player(&mut self, p: &Player) {
        let players = self.world.players();
        let i = players
            .iter()
            .position(|x| std::ptr::eq(&**x, p))
            .unwrap_or_else(|| panic!("save game: player {} is not in the world", p.internal_name));
        self.u32(i as u32);
    }

    /// Writes a reference to the trait at `ptr`.
    pub fn trait_ref<T: ?Sized>(&mut self, ptr: *const T, type_name: &str) {
        let (actor, index) = self
            .world
            .trait_owner(addr(ptr))
            .unwrap_or_else(|| panic!("save game: {type_name} is not a trait of a known actor"));
        self.actor(&actor);
        self.u32(index);
    }

    fn rule_path(&mut self, ptr: usize, type_name: &str) {
        let path = self
            .rules
            .get(&ptr)
            .cloned()
            .unwrap_or_else(|| panic!("save game: {type_name} is not part of the rules"));
        match path {
            RulePath::TraitInfo(a, i) => {
                self.u8(0);
                self.str(&a);
                self.u32(i);
            }
            RulePath::Weapon(n) => {
                self.u8(1);
                self.str(&n);
            }
            RulePath::Projectile(n) => {
                self.u8(2);
                self.str(&n);
            }
            RulePath::Warhead(n, i) => {
                self.u8(3);
                self.str(&n);
                self.u32(i);
            }
        }
    }

    /// Writes a reference to a rule object, such as a trait info or a weapon.
    pub fn rule<T: ?Sized>(&mut self, ptr: *const T) {
        self.rule_path(addr(ptr), std::any::type_name::<T>());
    }

    /// Writes the reference tag of a shared object; true when the object is new and its content must follow.
    pub fn shared_tag<T: ?Sized + 'static>(&mut self, rc: &Rc<T>) -> bool {
        let a = addr(Rc::as_ptr(rc));
        if let Some(&i) = self.objects.get(&a) {
            self.u8(TAG_REF);
            self.u32(i);
            return false;
        }
        self.objects.insert(a, self.objects.len() as u32);
        self.keep.push(Box::new(rc.clone()));
        self.u8(TAG_NEW);
        true
    }

    /// The bytes that `f` writes, which are not added to the output. `f` must not write shared objects.
    pub fn capture(&mut self, f: impl FnOnce(&mut Self)) -> Vec<u8> {
        let objects = self.objects.len();
        let outer = std::mem::take(&mut self.buf);
        f(self);
        assert_eq!(objects, self.objects.len(), "save game: a hash map key holds a shared object");
        std::mem::replace(&mut self.buf, outer)
    }

    /// Writes a shared object of a concrete type.
    pub fn shared_direct<T: Persist + 'static>(&mut self, rc: &Rc<T>) {
        if self.shared_tag(rc) {
            rc.save(self);
        }
    }

    /// Writes a shared trait object with the registry key of its type.
    pub fn shared_dyn<D: ?Sized + PersistDyn + 'static>(&mut self, rc: &Rc<D>) {
        if self.shared_tag(rc) {
            self.str(rc.persist_type());
            rc.persist_save(self);
        }
    }

    /// Writes a trait object by value with the registry key of its type.
    pub fn value_dyn<D: ?Sized + PersistDyn>(&mut self, v: &D) {
        self.str(v.persist_type());
        v.persist_save(self);
    }
}

enum Slot {
    /// Being loaded; holds the `Weak<T>` of `Rc::new_cyclic`.
    Pending(Box<dyn Any>),
    /// Loaded; `any` is the `Rc<T>` of the concrete type, `forms` are its `Rc<dyn Trait>` forms.
    Done { any: Rc<dyn Any>, forms: Vec<Box<dyn Any>> },
    /// A shared value without a concrete `Rc<dyn Any>` form.
    Value(Box<dyn Any>),
}

/// Reads a save game.
pub struct SaveReader<'a> {
    data: &'a [u8],
    pos: usize,
    world: Rc<World>,
    actors: HashMap<u32, ActorRef>,
    slots: Vec<Slot>,
    /// The slot of the shared object that a registry loader is about to construct.
    pending: Option<usize>,
    /// The actor whose state is being read.
    current_actor: Option<ActorRef>,
}

type Loader<D> = fn(&mut SaveReader) -> Rc<D>;

thread_local! {
    static LOADERS: RefCell<HashMap<(TypeId, &'static str), Box<dyn Any>>> = RefCell::new(HashMap::new());
}

/// Registers the loader of the type `name` for trait objects `Rc<D>`.
pub fn register_loader<D: ?Sized + 'static>(name: &'static str, f: Loader<D>) {
    LOADERS.with(|l| l.borrow_mut().insert((TypeId::of::<D>(), name), Box::new(f)));
}

/// True if a loader for `name` is registered for `D`.
pub fn has_loader<D: ?Sized + 'static>(name: &str) -> bool {
    LOADERS.with(|l| l.borrow().keys().any(|(t, n)| *t == TypeId::of::<D>() && *n == name))
}

fn loader<D: ?Sized + 'static>(name: &str) -> Option<Loader<D>> {
    LOADERS.with(|l| {
        l.borrow()
            .iter()
            .find(|((t, n), _)| *t == TypeId::of::<D>() && *n == name)
            .and_then(|(_, f)| f.downcast_ref::<Loader<D>>().copied())
    })
}

impl<'a> SaveReader<'a> {
    pub fn new(world: &Rc<World>, data: &'a [u8]) -> SaveReader<'a> {
        SaveReader {
            data,
            pos: 0,
            world: world.clone(),
            actors: HashMap::new(),
            slots: Vec::new(),
            pending: None,
            current_actor: None,
        }
    }

    pub fn world(&self) -> &Rc<World> {
        &self.world
    }

    /// Aborts the load with `msg`.
    pub fn fail(&self, msg: String) -> ! {
        std::panic::panic_any(LoadFailure(format!("{msg} (at byte {})", self.pos)))
    }

    pub fn at_end(&self) -> bool {
        self.pos == self.data.len()
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn take(&mut self, n: usize) -> &'a [u8] {
        if self.data.len() - self.pos < n {
            self.fail("unexpected end of save game".into());
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        s
    }

    fn array<const N: usize>(&mut self) -> [u8; N] {
        self.take(N).try_into().expect("length checked")
    }

    pub fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }

    pub fn bool(&mut self) -> bool {
        match self.u8() {
            0 => false,
            1 => true,
            v => self.fail(format!("invalid bool {v}")),
        }
    }

    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.array())
    }

    pub fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.array())
    }

    pub fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.array())
    }

    pub fn len_prefix(&mut self) -> usize {
        let n = self.u32() as usize;
        if n > self.data.len() - self.pos {
            self.fail(format!("invalid length {n}"));
        }
        n
    }

    pub fn string(&mut self) -> String {
        let n = self.len_prefix();
        match std::str::from_utf8(self.take(n)) {
            Ok(s) => s.to_string(),
            Err(e) => self.fail(format!("invalid string: {e}")),
        }
    }

    /// The actor whose state is being read, for traits that set up lazy state before they read it.
    pub fn current_actor(&self) -> ActorRef {
        self.current_actor
            .clone()
            .unwrap_or_else(|| self.fail("no actor is being read".into()))
    }

    pub fn set_current_actor(&mut self, a: Option<ActorRef>) {
        self.current_actor = a;
    }

    /// Makes `a` known to [`SaveReader::actor`].
    pub fn add_actor(&mut self, a: ActorRef) {
        self.actors.insert(a.id, a);
    }

    pub fn actor_by_id(&self, id: u32) -> ActorRef {
        self.actors
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.fail(format!("unknown actor {id}")))
    }

    pub fn actor(&mut self) -> ActorRef {
        let id = self.u32();
        self.actor_by_id(id)
    }

    pub fn player(&mut self) -> PlayerRef {
        let i = self.u32() as usize;
        self.world
            .players()
            .get(i)
            .cloned()
            .unwrap_or_else(|| self.fail(format!("unknown player {i}")))
    }

    /// Reads a trait reference as `Rc<T>`, where `T` is the trait type or one of its registered interfaces.
    pub fn trait_ref<T: ?Sized + 'static>(&mut self) -> Rc<T> {
        let actor = self.actor();
        let index = self.u32() as usize;
        let Some(t) = actor.trait_objects().get(index).cloned() else {
            self.fail(format!("actor {actor:?} has no trait {index}"))
        };
        let name = t.type_name();
        match t.cast_rc(TypeId::of::<T>()).and_then(|b| b.downcast::<Rc<T>>().ok()) {
            Some(rc) => *rc,
            None => self.fail(format!("trait {name} of {actor:?} is not a {}", std::any::type_name::<T>())),
        }
    }

    fn rule_any(&mut self) -> Rc<dyn Any> {
        let rules = self.world.rules().clone();
        match self.u8() {
            0 => {
                let name = self.string();
                let i = self.u32() as usize;
                let info = rules
                    .actor(&name)
                    .unwrap_or_else(|| self.fail(format!("unknown actor type {name}")));
                let Some(t) = info.traits.get(i).cloned() else {
                    self.fail(format!("actor type {name} has no trait {i}"))
                };
                t.as_any_rc()
            }
            1 => {
                let name = self.string();
                rules.weapon(&name).unwrap_or_else(|| self.fail(format!("unknown weapon {name}")))
            }
            t => self.fail(format!("invalid rule reference {t}")),
        }
    }

    /// Reads a reference to a rule object of a concrete type.
    pub fn rule<T: 'static>(&mut self) -> Rc<T> {
        let any = self.rule_any();
        any.downcast::<T>()
            .unwrap_or_else(|_| self.fail(format!("rule reference is not a {}", std::any::type_name::<T>())))
    }

    /// Reads a reference to a weapon part: `(weapon, warhead index)`, where the index is `None` for the projectile.
    pub fn weapon_part(&mut self) -> (Rc<crate::weapon::WeaponInfo>, Option<usize>) {
        let tag = self.u8();
        let name = self.string();
        let w = self
            .world
            .rules()
            .weapon(&name)
            .unwrap_or_else(|| self.fail(format!("unknown weapon {name}")));
        match tag {
            2 => (w, None),
            3 => (w, Some(self.u32() as usize)),
            t => self.fail(format!("invalid weapon part reference {t}")),
        }
    }

    fn slot_done<T: 'static>(&self, i: usize) -> Rc<T> {
        match self.slots.get(i) {
            Some(Slot::Done { any, .. }) => any
                .clone()
                .downcast::<T>()
                .unwrap_or_else(|_| self.fail(format!("object {i} is not a {}", std::any::type_name::<T>()))),
            Some(Slot::Pending(_)) => self.fail(format!(
                "object {i} ({}) is referenced while it is being loaded",
                std::any::type_name::<T>()
            )),
            _ => self.fail(format!("invalid object reference {i}")),
        }
    }

    fn slot_weak<T: 'static>(&self, i: usize) -> Weak<T> {
        match self.slots.get(i) {
            Some(Slot::Pending(w)) => w
                .downcast_ref::<Weak<T>>()
                .cloned()
                .unwrap_or_else(|| self.fail(format!("object {i} is not a {}", std::any::type_name::<T>()))),
            _ => Rc::downgrade(&self.slot_done::<T>(i)),
        }
    }

    /// Constructs the shared object of the slot that the current registry loader was called for.
    pub fn construct_shared<T: PersistShared + 'static>(&mut self) -> Rc<T> {
        let i = self.pending.take().unwrap_or_else(|| self.fail("no pending shared object".into()));
        self.construct_at::<T>(i)
    }

    fn construct_at<T: PersistShared + 'static>(&mut self, i: usize) -> Rc<T> {
        let rc = Rc::new_cyclic(|me: &Weak<T>| {
            self.slots[i] = Slot::Pending(Box::new(me.clone()));
            T::load_shared(self, me)
        });
        self.slots[i] = Slot::Done {
            any: rc.clone(),
            forms: Vec::new(),
        };
        rc
    }

    /// Reads a tag; `Some(slot)` of a reference, or `None` after allocating the slot of a new object.
    fn shared_tag(&mut self) -> Result<usize, usize> {
        match self.u8() {
            TAG_REF => {
                let i = self.u32() as usize;
                if i >= self.slots.len() {
                    self.fail(format!("invalid object reference {i}"));
                }
                Ok(i)
            }
            TAG_NEW => {
                self.slots.push(Slot::Pending(Box::new(())));
                Err(self.slots.len() - 1)
            }
            t => self.fail(format!("invalid object tag {t}")),
        }
    }

    pub fn shared_direct<T: PersistShared + 'static>(&mut self) -> Rc<T> {
        match self.shared_tag() {
            Ok(i) => self.slot_done(i),
            Err(i) => self.construct_at::<T>(i),
        }
    }

    pub fn shared_direct_weak<T: PersistShared + 'static>(&mut self) -> Weak<T> {
        match self.shared_tag() {
            Ok(i) => self.slot_weak(i),
            Err(i) => Rc::downgrade(&self.construct_at::<T>(i)),
        }
    }

    /// Reads a shared trait object through the loader registry of `D`.
    pub fn shared_dyn<D: ?Sized + 'static>(&mut self) -> Rc<D> {
        match self.shared_tag() {
            Ok(i) => {
                if let Some(Slot::Done { forms, .. }) = self.slots.get(i)
                    && let Some(rc) = forms.iter().find_map(|f| f.downcast_ref::<Rc<D>>())
                {
                    return rc.clone();
                }
                self.fail(format!("object {i} is not a {}", std::any::type_name::<D>()))
            }
            Err(i) => {
                let name = self.string();
                let f = loader::<D>(&name).unwrap_or_else(|| self.fail(format!("no loader for {name} as {}", std::any::type_name::<D>())));
                self.pending = Some(i);
                let rc = f(self);
                match &mut self.slots[i] {
                    Slot::Done { forms, .. } => forms.push(Box::new(rc.clone())),
                    _ => self.fail(format!("the loader of {name} did not construct its object")),
                }
                rc
            }
        }
    }

    fn concrete_of_dyn<T: 'static, D: ?Sized + RcPersist>(&mut self, weak: bool) -> Result<Rc<T>, Weak<T>> {
        let start = self.pos;
        if let Ok(i) = self.shared_tag() {
            return if weak { Err(self.slot_weak(i)) } else { Ok(self.slot_done(i)) };
        }
        self.slots.pop();
        self.pos = start;
        let i = self.slots.len();
        let _ = D::load_rc(self);
        Ok(self.slot_done(i))
    }

    /// Reads `Rc<T>` of an object written as `Rc<D>`, like an activity of type `T` written as `dyn Activity`.
    pub fn shared_concrete<T: 'static, D: ?Sized + RcPersist>(&mut self) -> Rc<T> {
        match self.concrete_of_dyn::<T, D>(false) {
            Ok(rc) => rc,
            Err(_) => unreachable!(),
        }
    }

    pub fn shared_concrete_weak<T: 'static, D: ?Sized + RcPersist>(&mut self) -> Weak<T> {
        match self.concrete_of_dyn::<T, D>(true) {
            Ok(rc) => Rc::downgrade(&rc),
            Err(w) => w,
        }
    }

    /// Reads a shared value in place into `rc`, which then stands for it in the object table.
    pub fn shared_in_place<T: ?Sized + 'static>(&mut self, rc: &Rc<T>, load: impl FnOnce(&mut Self, &T)) {
        match self.shared_tag() {
            Ok(i) => match self.slots.get(i) {
                Some(Slot::Value(v)) if v.downcast_ref::<Rc<T>>().is_some_and(|o| Rc::ptr_eq(o, rc)) => {}
                _ => self.fail(format!("the shared {} was loaded before its owner", std::any::type_name::<T>())),
            },
            Err(i) => {
                self.slots[i] = Slot::Value(Box::new(rc.clone()));
                load(self, rc);
            }
        }
    }

    /// Reads a shared value that cannot refer back to itself.
    pub fn shared_value<T: Clone + 'static>(&mut self, load: impl FnOnce(&mut Self) -> T) -> T {
        match self.shared_tag() {
            Ok(i) => match self.slots.get(i) {
                Some(Slot::Value(v)) => v
                    .downcast_ref::<T>()
                    .cloned()
                    .unwrap_or_else(|| self.fail(format!("object {i} is not a {}", std::any::type_name::<T>()))),
                _ => self.fail(format!("object {i} is not a shared value")),
            },
            Err(i) => {
                let v = load(self);
                self.slots[i] = Slot::Value(Box::new(v.clone()));
                v
            }
        }
    }

    /// Reads a trait object by value through the loader registry of `D`.
    pub fn value_dyn<D: ?Sized + 'static>(&mut self) -> Rc<D> {
        let name = self.string();
        let f = loader::<D>(&name).unwrap_or_else(|| self.fail(format!("no loader for {name} as {}", std::any::type_name::<D>())));
        f(self)
    }
}

impl SaveWriter {
    /// Writes a shared value that cannot refer back to itself, such as `Rc<Cell<T>>`.
    pub fn shared_value<T: ?Sized + 'static>(&mut self, rc: &Rc<T>, write: impl FnOnce(&mut Self, &T)) {
        if self.shared_tag(rc) {
            write(self, rc);
        }
    }
}

/// Implements [`Persist`] for a type that save games never contain, such as a cosmetic effect.
#[macro_export]
macro_rules! persist_never {
    ($($t:ty),* $(,)?) => {
        $(impl $crate::persist::Persist for $t {
            fn save(&self, _w: &mut $crate::persist::SaveWriter) {
                panic!("save game: {} cannot be saved", stringify!($t));
            }
            fn load(r: &mut $crate::persist::SaveReader) -> Self {
                r.fail(format!("save game: {} cannot be loaded", stringify!($t)))
            }
        })*
    };
}

/// Registers the loaders of the trait object types of the engine.
pub fn register_engine_loaders() {
    crate::persist_values!(dyn SavedFn: NoOp);
    crate::actor_init::register_persist();
    crate::weapon::register_persist();
}

/// Registers the loaders of activity types: `persist_activities!(Move, Attack)`.
#[macro_export]
macro_rules! persist_activities {
    ($($t:ty),* $(,)?) => {
        $($crate::persist::register_loader::<dyn $crate::activity::Activity>(
            ::std::any::type_name::<$t>(),
            |r| -> ::std::rc::Rc<dyn $crate::activity::Activity> { r.construct_shared::<$t>() },
        );)*
    };
}

/// Registers the loaders of effect types: `persist_effects!(Bullet, Missile)`.
#[macro_export]
macro_rules! persist_effects {
    ($($t:ty),* $(,)?) => {
        $($crate::persist::register_loader::<dyn $crate::effect::Effect>(
            ::std::any::type_name::<$t>(),
            |r| -> ::std::rc::Rc<dyn $crate::effect::Effect> { r.construct_shared::<$t>() },
        );)*
    };
}

/// Registers the loaders of trait object values: `persist_values!(dyn ActorInit: LocationInit, FacingInit)`.
#[macro_export]
macro_rules! persist_values {
    ($d:ty: $($t:ty),* $(,)?) => {
        $($crate::persist::register_loader::<$d>(
            ::std::any::type_name::<$t>(),
            |r| -> ::std::rc::Rc<$d> { ::std::rc::Rc::new(<$t as $crate::persist::Persist>::load(r)) },
        );)*
    };
}

/// Implements [`RcPersist`] for trait objects that are traits of actors, such as `dyn IMove`:
/// `persist_trait_refs!(dyn IMove, dyn IFacing)`.
#[macro_export]
macro_rules! persist_trait_refs {
    ($($t:ty),* $(,)?) => {
        $(impl $crate::persist::RcPersist for $t {
            fn save_rc(this: &::std::rc::Rc<Self>, w: &mut $crate::persist::SaveWriter) {
                w.trait_ref(::std::rc::Rc::as_ptr(this), stringify!($t));
            }
            fn load_rc(r: &mut $crate::persist::SaveReader) -> ::std::rc::Rc<Self> {
                r.trait_ref::<Self>()
            }
        })*
    };
}

/// Implements [`RcPersist`] for trait info types, written as references into the rules.
#[macro_export]
macro_rules! persist_rule_refs {
    ($($t:ty),* $(,)?) => {
        $(impl $crate::persist::RcPersist for $t {
            fn save_rc(this: &::std::rc::Rc<Self>, w: &mut $crate::persist::SaveWriter) {
                w.rule(::std::rc::Rc::as_ptr(this));
            }
            fn load_rc(r: &mut $crate::persist::SaveReader) -> ::std::rc::Rc<Self> {
                r.rule::<Self>()
            }
        })*
    };
}

macro_rules! persist_int {
    ($($t:ty),*) => {
        $(impl Persist for $t {
            fn save(&self, w: &mut SaveWriter) {
                w.bytes(&self.to_le_bytes());
            }
            fn load(r: &mut SaveReader) -> Self {
                <$t>::from_le_bytes(r.take(std::mem::size_of::<$t>()).try_into().expect("length checked"))
            }
        })*
    };
}

persist_int!(i8, i16, i32, i64, u16, u32, u64, i128, u128, f32, f64);

impl Persist for u8 {
    fn save(&self, w: &mut SaveWriter) {
        w.u8(*self)
    }
    fn load(r: &mut SaveReader) -> Self {
        r.u8()
    }
}

impl Persist for usize {
    fn save(&self, w: &mut SaveWriter) {
        w.u64(*self as u64)
    }
    fn load(r: &mut SaveReader) -> Self {
        let v = r.u64();
        usize::try_from(v).unwrap_or_else(|_| r.fail(format!("invalid usize {v}")))
    }
}

impl Persist for isize {
    fn save(&self, w: &mut SaveWriter) {
        (*self as i64).save(w)
    }
    fn load(r: &mut SaveReader) -> Self {
        i64::load(r) as isize
    }
}

impl Persist for bool {
    fn save(&self, w: &mut SaveWriter) {
        w.bool(*self)
    }
    fn load(r: &mut SaveReader) -> Self {
        r.bool()
    }
}

impl Persist for char {
    fn save(&self, w: &mut SaveWriter) {
        w.u32(*self as u32)
    }
    fn load(r: &mut SaveReader) -> Self {
        let v = r.u32();
        char::from_u32(v).unwrap_or_else(|| r.fail(format!("invalid char {v}")))
    }
}

impl Persist for () {
    fn save(&self, _w: &mut SaveWriter) {}
    fn load(_r: &mut SaveReader) -> Self {}
}

impl<T> Persist for PhantomData<T> {
    fn save(&self, _w: &mut SaveWriter) {}
    fn load(_r: &mut SaveReader) -> Self {
        PhantomData
    }
}

impl Persist for String {
    fn save(&self, w: &mut SaveWriter) {
        w.str(self)
    }
    fn load(r: &mut SaveReader) -> Self {
        r.string()
    }
}

impl Persist for &'static str {
    fn save(&self, w: &mut SaveWriter) {
        w.str(self)
    }
    fn load(r: &mut SaveReader) -> Self {
        Box::leak(r.string().into_boxed_str())
    }
}

impl<T: Persist> Persist for Option<T> {
    fn save(&self, w: &mut SaveWriter) {
        match self {
            None => w.bool(false),
            Some(v) => {
                w.bool(true);
                v.save(w)
            }
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        r.bool().then(|| T::load(r))
    }
}

impl<T: Persist, E: Persist> Persist for Result<T, E> {
    fn save(&self, w: &mut SaveWriter) {
        match self {
            Ok(v) => {
                w.bool(true);
                v.save(w)
            }
            Err(e) => {
                w.bool(false);
                e.save(w)
            }
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        if r.bool() { Ok(T::load(r)) } else { Err(E::load(r)) }
    }
}

impl<T: Persist> Persist for Box<T> {
    fn save(&self, w: &mut SaveWriter) {
        (**self).save(w)
    }
    fn load(r: &mut SaveReader) -> Self {
        Box::new(T::load(r))
    }
}

impl<T: Persist> Persist for Vec<T> {
    fn save(&self, w: &mut SaveWriter) {
        w.len_prefix(self.len());
        for v in self {
            v.save(w);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        let n = r.len_prefix();
        (0..n).map(|_| T::load(r)).collect()
    }
}

impl<T: Persist> Persist for Box<[T]> {
    fn save(&self, w: &mut SaveWriter) {
        w.len_prefix(self.len());
        for v in self.iter() {
            v.save(w);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        Vec::<T>::load(r).into_boxed_slice()
    }
}

impl<T: Persist> Persist for VecDeque<T> {
    fn save(&self, w: &mut SaveWriter) {
        w.len_prefix(self.len());
        for v in self {
            v.save(w);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        Vec::<T>::load(r).into()
    }
}

impl<T: Persist, const N: usize> Persist for [T; N] {
    fn save(&self, w: &mut SaveWriter) {
        for v in self {
            v.save(w);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        std::array::from_fn(|_| T::load(r))
    }
}

macro_rules! persist_tuple {
    ($($n:tt $t:ident),*) => {
        impl<$($t: Persist),*> Persist for ($($t,)*) {
            fn save(&self, w: &mut SaveWriter) {
                $(self.$n.save(w);)*
            }
            fn load(r: &mut SaveReader) -> Self {
                ($($t::load(r),)*)
            }
        }
    };
}

persist_tuple!(0 A);
persist_tuple!(0 A, 1 B);
persist_tuple!(0 A, 1 B, 2 C);
persist_tuple!(0 A, 1 B, 2 C, 3 D);
persist_tuple!(0 A, 1 B, 2 C, 3 D, 4 E);
persist_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F);

impl<K: Persist + Ord, V: Persist> Persist for BTreeMap<K, V> {
    fn save(&self, w: &mut SaveWriter) {
        w.len_prefix(self.len());
        for (k, v) in self {
            k.save(w);
            v.save(w);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        let n = r.len_prefix();
        (0..n).map(|_| (K::load(r), V::load(r))).collect()
    }
}

impl<K: Persist + Ord> Persist for BTreeSet<K> {
    fn save(&self, w: &mut SaveWriter) {
        w.len_prefix(self.len());
        for k in self {
            k.save(w);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        let n = r.len_prefix();
        (0..n).map(|_| K::load(r)).collect()
    }
}

/// Hash maps iterate in an arbitrary order, so the simulation must not depend on their order. They are written in
/// the order of the written keys, so that equal maps give equal bytes. Keys must not hold shared objects.
impl<K: Persist + Hash + Eq, V: Persist, S: std::hash::BuildHasher + Default> Persist for HashMap<K, V, S> {
    fn save(&self, w: &mut SaveWriter) {
        let mut entries: Vec<(Vec<u8>, &V)> = self.iter().map(|(k, v)| (w.capture(|w| k.save(w)), v)).collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        w.len_prefix(entries.len());
        for (k, v) in entries {
            w.bytes(&k);
            v.save(w);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        let n = r.len_prefix();
        (0..n).map(|_| (K::load(r), V::load(r))).collect()
    }
}

impl<K: Persist + Hash + Eq, S: std::hash::BuildHasher + Default> Persist for HashSet<K, S> {
    fn save(&self, w: &mut SaveWriter) {
        let mut keys: Vec<Vec<u8>> = self.iter().map(|k| w.capture(|w| k.save(w))).collect();
        keys.sort();
        w.len_prefix(keys.len());
        for k in keys {
            w.bytes(&k);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        let n = r.len_prefix();
        (0..n).map(|_| K::load(r)).collect()
    }
}

impl<K: Persist + Hash + Eq, V: Persist, S: std::hash::BuildHasher + Default> Persist for IndexMap<K, V, S> {
    fn save(&self, w: &mut SaveWriter) {
        w.len_prefix(self.len());
        for (k, v) in self {
            k.save(w);
            v.save(w);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        let n = r.len_prefix();
        (0..n).map(|_| (K::load(r), V::load(r))).collect()
    }
}

impl<K: Persist + Hash + Eq, S: std::hash::BuildHasher + Default> Persist for IndexSet<K, S> {
    fn save(&self, w: &mut SaveWriter) {
        w.len_prefix(self.len());
        for k in self {
            k.save(w);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        let n = r.len_prefix();
        (0..n).map(|_| K::load(r)).collect()
    }
}

impl<T: Persist + Copy> Persist for Cell<T> {
    fn save(&self, w: &mut SaveWriter) {
        self.get().save(w)
    }
    fn load(r: &mut SaveReader) -> Self {
        Cell::new(T::load(r))
    }
}

impl<T: Persist + Copy> PersistState for Cell<T> {
    fn save_state(&self, w: &mut SaveWriter) {
        self.get().save(w)
    }
    fn load_state(&self, r: &mut SaveReader) {
        self.set(T::load(r))
    }
}

impl<T: Persist> Persist for RefCell<T> {
    fn save(&self, w: &mut SaveWriter) {
        self.borrow().save(w)
    }
    fn load(r: &mut SaveReader) -> Self {
        RefCell::new(T::load(r))
    }
}

impl<T: Persist> PersistState for RefCell<T> {
    fn save_state(&self, w: &mut SaveWriter) {
        self.borrow().save(w)
    }
    fn load_state(&self, r: &mut SaveReader) {
        let v = T::load(r);
        *self.borrow_mut() = v;
    }
}

/// A once cell that the owner fills when it is created; its value is written in place.
impl<T: PersistState> PersistState for std::cell::OnceCell<T> {
    fn save_state(&self, w: &mut SaveWriter) {
        match self.get() {
            None => w.bool(false),
            Some(v) => {
                w.bool(true);
                v.save_state(w)
            }
        }
    }
    fn load_state(&self, r: &mut SaveReader) {
        match (r.bool(), self.get()) {
            (false, _) => {}
            (true, Some(v)) => v.load_state(r),
            (true, None) => r.fail(format!("{} is empty in the loaded world", std::any::type_name::<Self>())),
        }
    }
}

impl<T: PersistState> PersistState for Option<T> {
    fn save_state(&self, w: &mut SaveWriter) {
        match self {
            None => w.bool(false),
            Some(v) => {
                w.bool(true);
                v.save_state(w)
            }
        }
    }
    fn load_state(&self, r: &mut SaveReader) {
        match (r.bool(), self) {
            (false, None) => {}
            (true, Some(v)) => v.load_state(r),
            _ => r.fail(format!("{} differs in the loaded world", std::any::type_name::<Self>())),
        }
    }
}

/// The elements are written in place; the loaded vector must have the same length.
impl<T: PersistState> PersistState for Vec<T> {
    fn save_state(&self, w: &mut SaveWriter) {
        w.len_prefix(self.len());
        for v in self {
            v.save_state(w);
        }
    }
    fn load_state(&self, r: &mut SaveReader) {
        let n = r.len_prefix();
        if n != self.len() {
            r.fail(format!(
                "{} has {} elements, the save game {n}",
                std::any::type_name::<Self>(),
                self.len()
            ));
        }
        for v in self {
            v.load_state(r);
        }
    }
}

/// The values are written in place in key order; the loaded map must have the same keys.
impl<K: Persist + Ord + Hash + Eq, V: PersistState, S: std::hash::BuildHasher> PersistState for HashMap<K, V, S> {
    fn save_state(&self, w: &mut SaveWriter) {
        let mut keys: Vec<&K> = self.keys().collect();
        keys.sort();
        w.len_prefix(keys.len());
        for k in keys {
            k.save(w);
            self[k].save_state(w);
        }
    }
    fn load_state(&self, r: &mut SaveReader) {
        let n = r.len_prefix();
        for _ in 0..n {
            let k = K::load(r);
            match self.get(&k) {
                Some(v) => v.load_state(r),
                None => r.fail(format!("a key of {} is missing in the loaded world", std::any::type_name::<Self>())),
            }
        }
    }
}

/// A shared cell owned by the holder: written in place, but through the object table, so that other holders of
/// the same `Rc`, such as the function after an animation, get it back. The owner must be loaded first; derived
/// implementations write such fields before the others.
impl<T: Persist + Copy + 'static> PersistState for Rc<Cell<T>> {
    fn save_state(&self, w: &mut SaveWriter) {
        if w.shared_tag(self) {
            self.get().save(w);
        }
    }
    fn load_state(&self, r: &mut SaveReader) {
        r.shared_in_place(self, |r, c| c.set(T::load(r)));
    }
}

impl<T: Persist + 'static> PersistState for Rc<RefCell<T>> {
    fn save_state(&self, w: &mut SaveWriter) {
        if w.shared_tag(self) {
            self.borrow().save(w);
        }
    }
    fn load_state(&self, r: &mut SaveReader) {
        r.shared_in_place(self, |r, c| {
            let v = T::load(r);
            *c.borrow_mut() = v;
        });
    }
}

impl<T: ?Sized + RcPersist> Persist for Rc<T> {
    fn save(&self, w: &mut SaveWriter) {
        T::save_rc(self, w)
    }
    fn load(r: &mut SaveReader) -> Self {
        T::load_rc(r)
    }
}

impl<T: RcPersist> Persist for Weak<T> {
    fn save(&self, w: &mut SaveWriter) {
        match self.upgrade() {
            None => w.bool(false),
            Some(rc) => {
                w.bool(true);
                T::save_rc(&rc, w)
            }
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        if r.bool() { T::load_weak(r) } else { Weak::new() }
    }
}

/// `Rc<Cell<T>>` shared between objects, such as a state that an activity and its child both change.
impl<T: Persist + Copy + 'static> RcPersist for Cell<T> {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.shared_value(this, |w, v| v.save(w));
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.shared_value(|r| Rc::new(Cell::<T>::load(r)))
    }
}

impl<T: Persist + 'static> RcPersist for RefCell<T> {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.shared_value(this, |w, v| v.save(w));
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.shared_value(|r| Rc::new(RefCell::<T>::load(r)))
    }
}

/// Immutable shared slices are written by value.
impl<T: Persist + 'static> RcPersist for [T] {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.len_prefix(this.len());
        for v in this.iter() {
            v.save(w);
        }
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        Vec::<T>::load(r).into()
    }
}

impl RcPersist for IndexSet<String> {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.shared_value(this, |w, v| v.save(w));
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.shared_value(|r| Rc::new(IndexSet::<String>::load(r)))
    }
}

impl RcPersist for str {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.str(this)
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.string().into()
    }
}

impl RcPersist for Actor {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.actor(this)
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.actor()
    }
}

impl RcPersist for Player {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.player(this)
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.player()
    }
}

impl RcPersist for ActorInfo {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.str(&this.name)
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        let name = r.string();
        r.world()
            .rules()
            .actor(&name)
            .unwrap_or_else(|| r.fail(format!("unknown actor type {name}")))
    }
}

impl RcPersist for World {
    fn save_rc(_this: &Rc<Self>, _w: &mut SaveWriter) {}
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.world().clone()
    }
}

impl RcPersist for crate::map::Map {
    fn save_rc(_this: &Rc<Self>, _w: &mut SaveWriter) {}
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.world().map().clone()
    }
}

impl RcPersist for crate::rules::Ruleset {
    fn save_rc(_this: &Rc<Self>, _w: &mut SaveWriter) {}
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.world().rules().clone()
    }
}

impl RcPersist for crate::weapon::WeaponInfo {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.rule(Rc::as_ptr(this))
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.rule::<Self>()
    }
}

impl RcPersist for dyn crate::weapon::IWarhead {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.rule(Rc::as_ptr(this))
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        match r.weapon_part() {
            (w, Some(i)) => w
                .warheads
                .get(i)
                .cloned()
                .unwrap_or_else(|| r.fail(format!("weapon has no warhead {i}"))),
            _ => r.fail("expected a warhead reference".into()),
        }
    }
}

impl RcPersist for dyn crate::weapon::IProjectileInfo {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.rule(Rc::as_ptr(this))
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        match r.weapon_part() {
            (w, None) => w.projectile.clone().unwrap_or_else(|| r.fail("weapon has no projectile".into())),
            _ => r.fail("expected a projectile reference".into()),
        }
    }
}

impl RcPersist for dyn crate::activity::Activity {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.shared_dyn(this)
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.shared_dyn::<Self>()
    }
}

impl RcPersist for dyn crate::effect::Effect {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.shared_dyn(this)
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.shared_dyn::<Self>()
    }
}

impl RcPersist for dyn SavedPredicate {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.value_dyn(&**this)
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.value_dyn::<Self>()
    }
}

impl RcPersist for dyn SavedFn {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.value_dyn(&**this)
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.value_dyn::<Self>()
    }
}

impl RcPersist for dyn crate::actor_init::ActorInit {
    fn save_rc(this: &Rc<Self>, w: &mut SaveWriter) {
        w.value_dyn(&**this)
    }
    fn load_rc(r: &mut SaveReader) -> Rc<Self> {
        r.value_dyn::<Self>()
    }
}

macro_rules! persist_fields {
    ($($t:ty { $($f:ident),* }),* $(,)?) => {
        $(impl Persist for $t {
            fn save(&self, w: &mut SaveWriter) {
                $(self.$f.save(w);)*
            }
            fn load(r: &mut SaveReader) -> Self {
                Self { $($f: Persist::load(r)),* }
            }
        })*
    };
}

persist_fields!(
    WDist { length },
    WAngle { angle },
    WPos { x, y, z },
    WVec { x, y, z },
    CVec { x, y },
    Int2 { x, y },
    MPos { u, v },
    PPos { u, v },
    CPos { bits },
    Color { argb },
    Size { width, height },
    Rectangle { x, y, width, height },
    Float2 { x, y },
    Float3 { x, y, z },
);

impl Persist for WRot {
    fn save(&self, w: &mut SaveWriter) {
        self.to_raw().save(w)
    }
    fn load(r: &mut SaveReader) -> Self {
        WRot::from_raw(Persist::load(r))
    }
}

impl Persist for MersenneTwister {
    fn save(&self, w: &mut SaveWriter) {
        let (mt, index) = self.state();
        mt.save(w);
        index.save(w);
        self.last.save(w);
        self.total_count.save(w);
    }
    fn load(r: &mut SaveReader) -> Self {
        let mt = Persist::load(r);
        let index = Persist::load(r);
        let last = Persist::load(r);
        let total_count = Persist::load(r);
        MersenneTwister::from_state(mt, index, last, total_count)
    }
}

/// Bit sets are written as names, because the bits are allocated in first use order.
impl<T: 'static> Persist for crate::bitset::BitSet<T> {
    fn save(&self, w: &mut SaveWriter) {
        self.names().save(w)
    }
    fn load(r: &mut SaveReader) -> Self {
        crate::bitset::BitSet::new(&Vec::<String>::load(r))
    }
}

/// An object address used as a key, such as a shroud source: written as a reference to the trait or the shared
/// object at the address, which must have been written before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AddrKey(pub usize);

impl Persist for AddrKey {
    fn save(&self, w: &mut SaveWriter) {
        if let Some((actor, index)) = w.world.trait_owner(self.0) {
            w.u8(0);
            w.actor(&actor);
            w.u32(index);
        } else if let Some(&i) = w.objects.get(&self.0) {
            w.u8(1);
            w.u32(i);
        } else {
            panic!("save game: the address key {:#x} is neither a trait nor a written object", self.0);
        }
    }
    fn load(r: &mut SaveReader) -> Self {
        match r.u8() {
            0 => {
                let actor = r.actor();
                let index = r.u32() as usize;
                match actor.trait_objects().get(index) {
                    Some(t) => AddrKey(addr(Rc::as_ptr(t))),
                    None => r.fail(format!("actor {actor:?} has no trait {index}")),
                }
            }
            1 => {
                let i = r.u32() as usize;
                match r.slots.get(i) {
                    Some(Slot::Done { any, .. }) => AddrKey(addr(Rc::as_ptr(any))),
                    _ => r.fail(format!("invalid object reference {i}")),
                }
            }
            t => r.fail(format!("invalid address key tag {t}")),
        }
    }
}

impl Persist for rustora_core::miniyaml::MiniYaml {
    fn save(&self, w: &mut SaveWriter) {
        self.value.save(w);
        self.nodes.save(w);
    }
    fn load(r: &mut SaveReader) -> Self {
        let value = Persist::load(r);
        let nodes = Persist::load(r);
        rustora_core::miniyaml::MiniYaml::new(value, nodes)
    }
}

impl Persist for rustora_core::miniyaml::MiniYamlNode {
    fn save(&self, w: &mut SaveWriter) {
        self.key.save(w);
        self.value.save(w);
    }
    fn load(r: &mut SaveReader) -> Self {
        let key: String = Persist::load(r);
        rustora_core::miniyaml::MiniYamlNode::new(key, Persist::load(r))
    }
}

impl<T: Persist> Persist for crate::map::CellLayer<T> {
    fn save(&self, w: &mut SaveWriter) {
        self.width.save(w);
        self.height.save(w);
        self.raw().save(w);
    }
    fn load(r: &mut SaveReader) -> Self {
        let width = Persist::load(r);
        let height = Persist::load(r);
        crate::map::CellLayer::from_raw(width, height, Persist::load(r))
    }
}
