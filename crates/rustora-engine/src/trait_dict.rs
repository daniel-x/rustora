// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `TraitDictionary.cs`: traits per interface, sorted by actor ID and insertion order.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::actor::{Actor, ActorRef};

/// The traits of one key, sorted by actor ID. `ids` repeats the actor IDs in one contiguous array, so searches do not
/// read the actors.
struct Container<T: ?Sized> {
    ids: Vec<u32>,
    actors: Vec<ActorRef>,
    traits: Vec<Rc<T>>,
}

impl<T: ?Sized> Container<T> {
    /// Index of the first entry with actor ID >= `id`.
    fn lower_bound(&self, id: u32) -> usize {
        self.ids.partition_point(|&i| i < id)
    }
}

trait AnyContainer {
    fn remove_actor(&self, id: u32);
    fn as_any(&self) -> &dyn Any;
}

impl<T: ?Sized + 'static> AnyContainer for RefCell<Container<T>> {
    fn remove_actor(&self, id: u32) {
        let mut c = self.borrow_mut();
        let start = c.lower_bound(id);
        let end = c.lower_bound(id + 1);
        c.ids.drain(start..end);
        c.actors.drain(start..end);
        c.traits.drain(start..end);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Efficient queries of actors by the interfaces their traits implement.
#[derive(Default)]
pub struct TraitDict {
    containers: RefCell<HashMap<TypeId, Rc<dyn AnyContainer>>>,
}

impl TraitDict {
    fn container<T: ?Sized + 'static>(&self) -> Rc<dyn AnyContainer> {
        let key = TypeId::of::<T>();
        if let Some(c) = self.containers.borrow().get(&key) {
            return c.clone();
        }
        let c: Rc<dyn AnyContainer> = Rc::new(RefCell::new(Container::<T> {
            ids: Vec::new(),
            actors: Vec::new(),
            traits: Vec::new(),
        }));
        self.containers.borrow_mut().insert(key, c.clone());
        c
    }

    fn with<T: ?Sized + 'static, R>(&self, f: impl FnOnce(&RefCell<Container<T>>) -> R) -> R {
        let c = self.container::<T>();
        f(c.as_any().downcast_ref::<RefCell<Container<T>>>().expect("container type"))
    }

    /// Adds `trait_` for `actor` under the key `T` (an interface `dyn I` or a concrete type).
    pub fn add<T: ?Sized + 'static>(&self, actor: &ActorRef, trait_: Rc<T>) {
        self.with::<T, _>(|c| {
            let mut c = c.borrow_mut();
            let idx = c.lower_bound(actor.id + 1);
            c.ids.insert(idx, actor.id);
            c.actors.insert(idx, actor.clone());
            c.traits.insert(idx, trait_);
        })
    }

    /// The single trait of `actor` implementing `T`. Panics if there are several, like C#.
    pub fn get_or_default<T: ?Sized + 'static>(&self, actor: &Actor) -> Option<Rc<T>> {
        self.with::<T, _>(|c| {
            let c = c.borrow();
            let idx = c.lower_bound(actor.id);
            if c.ids.get(idx) != Some(&actor.id) {
                return None;
            }
            if c.ids.get(idx + 1) == Some(&actor.id) {
                panic!(
                    "Actor {} has multiple traits of type `{}`",
                    actor.info.name,
                    std::any::type_name::<T>()
                );
            }
            Some(c.traits[idx].clone())
        })
    }

    pub fn get<T: ?Sized + 'static>(&self, actor: &Actor) -> Rc<T> {
        self.get_or_default::<T>(actor).unwrap_or_else(|| {
            panic!(
                "Actor {} does not have trait of type `{}`",
                actor.info.name,
                std::any::type_name::<T>()
            )
        })
    }

    /// All traits of `actor` implementing `T`, in insertion order.
    pub fn with_interface<T: ?Sized + 'static>(&self, actor: &Actor) -> Vec<Rc<T>> {
        self.with::<T, _>(|c| {
            let c = c.borrow();
            let start = c.lower_bound(actor.id);
            let end = c.lower_bound(actor.id + 1);
            c.traits[start..end].to_vec()
        })
    }

    /// Snapshot of all (actor, trait) pairs for `T`.
    pub fn actors_with_trait<T: ?Sized + 'static>(&self) -> Vec<(ActorRef, Rc<T>)> {
        self.with::<T, _>(|c| {
            let c = c.borrow();
            c.actors.iter().cloned().zip(c.traits.iter().cloned()).collect()
        })
    }

    /// Calls `f` for each actor and trait pair of `T` without copying the list. `f` must not add
    /// or remove traits.
    pub fn for_each_with_trait<T: ?Sized + 'static>(&self, mut f: impl FnMut(&ActorRef, &Rc<T>)) {
        self.with::<T, _>(|c| {
            let c = c.borrow();
            for (a, t) in c.actors.iter().zip(&c.traits) {
                f(a, t);
            }
        })
    }

    /// Whether `f` holds for an actor and trait pair of `T`, without copying the list. `f` must not add or remove
    /// traits.
    pub fn any_with_trait<T: ?Sized + 'static>(&self, mut f: impl FnMut(&ActorRef, &Rc<T>) -> bool) -> bool {
        self.with::<T, _>(|c| {
            let c = c.borrow();
            c.actors.iter().zip(&c.traits).any(|(a, t)| f(a, t))
        })
    }

    /// Calls `f` once for each distinct actor that has a trait implementing `T`, in actor ID order. `f` must not add
    /// or remove traits.
    pub fn for_each_actor_having_trait<T: ?Sized + 'static>(&self, mut f: impl FnMut(&ActorRef)) {
        self.with::<T, _>(|c| {
            let c = c.borrow();
            for (i, a) in c.actors.iter().enumerate() {
                if i == 0 || c.ids[i - 1] != c.ids[i] {
                    f(a);
                }
            }
        })
    }

    /// Distinct actors that have a trait implementing `T`, in actor ID order.
    pub fn actors_having_trait<T: ?Sized + 'static>(&self) -> Vec<ActorRef> {
        self.with::<T, _>(|c| {
            let c = c.borrow();
            let mut out: Vec<ActorRef> = Vec::new();
            for (i, a) in c.actors.iter().enumerate() {
                if i == 0 || c.ids[i - 1] != c.ids[i] {
                    out.push(a.clone());
                }
            }
            out
        })
    }

    /// Like `actors_having_trait` but only counts traits matching `pred`.
    pub fn actors_having_trait_where<T: ?Sized + 'static>(&self, pred: impl Fn(&T) -> bool) -> Vec<ActorRef> {
        self.with::<T, _>(|c| {
            let c = c.borrow();
            let mut out: Vec<ActorRef> = Vec::new();
            let mut last = None;
            for ((a, t), &id) in c.actors.iter().zip(&c.traits).zip(&c.ids) {
                if last == Some(id) || !pred(t) {
                    continue;
                }
                last = Some(id);
                out.push(a.clone());
            }
            out
        })
    }

    /// `ApplyToActorsWithTrait`: index based iteration over the live container, so entries added
    /// during the iteration (at higher actor IDs) are visited too.
    pub fn apply_to_all<T: ?Sized + 'static>(&self, f: impl Fn(&ActorRef, &Rc<T>)) {
        let c = self.container::<T>();
        let cell = c.as_any().downcast_ref::<RefCell<Container<T>>>().expect("container type");
        let mut i = 0;
        loop {
            let item = {
                let b = cell.borrow();
                if i >= b.actors.len() {
                    break;
                }
                (b.actors[i].clone(), b.traits[i].clone())
            };
            f(&item.0, &item.1);
            i += 1;
        }
    }

    pub fn remove_actor(&self, id: u32) {
        let all: Vec<Rc<dyn AnyContainer>> = self.containers.borrow().values().cloned().collect();
        for c in all {
            c.remove_actor(id);
        }
    }
}
