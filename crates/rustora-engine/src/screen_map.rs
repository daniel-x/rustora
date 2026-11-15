// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The simulation side of `ScreenMap.cs`: the `AddOrUpdate` and `Remove` calls for actors, collected for the client,
//! which recomputes the screen bounds of these actors only. Worlds without a client collect nothing.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use crate::actor::ActorRef;

#[derive(Default)]
pub struct ScreenMapUpdates {
    enabled: Cell<bool>,
    add_or_update: RefCell<BTreeMap<u32, ActorRef>>,
    remove: RefCell<BTreeMap<u32, ActorRef>>,
}

impl ScreenMapUpdates {
    /// Starts collecting. The client calls it before it fills its screen map with the actors already in the world.
    pub fn enable(&self) {
        self.enabled.set(true);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.get()
    }

    /// `ScreenMap.AddOrUpdate(Actor)`.
    pub fn add_or_update(&self, a: &ActorRef) {
        if self.enabled.get() {
            self.remove.borrow_mut().remove(&a.id);
            self.add_or_update.borrow_mut().insert(a.id, a.clone());
        }
    }

    /// `ScreenMap.Remove(Actor)`. A removal wins over an `AddOrUpdate` of the same drain, like `ScreenMap.TickRender`.
    pub fn remove(&self, a: &ActorRef) {
        if self.enabled.get() {
            self.remove.borrow_mut().insert(a.id, a.clone());
        }
    }

    /// The actors to add or update and the actors to remove since the last call, by actor id.
    pub fn take(&self) -> (Vec<ActorRef>, Vec<ActorRef>) {
        let add = std::mem::take(&mut *self.add_or_update.borrow_mut());
        let remove = std::mem::take(&mut *self.remove.borrow_mut());
        (add.into_values().collect(), remove.into_values().collect())
    }
}
