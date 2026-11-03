// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Activities/Activity.cs` and `ActivityUtils`.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::actor::ActorRef;
use crate::target::Target;

pub type ActivityRef = Rc<dyn Activity>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    Queued,
    Active,
    Canceling,
    Done,
}

/// State shared by all activities (the private fields of the C# base class).
pub struct ActivityBase {
    state: Cell<ActivityState>,
    child: RefCell<Option<ActivityRef>>,
    next: RefCell<Option<ActivityRef>>,
    pub is_interruptible: Cell<bool>,
    pub child_has_priority: Cell<bool>,
    finishing: Cell<bool>,
    first_run_completed: Cell<bool>,
    last_run: Cell<bool>,
}

impl Default for ActivityBase {
    fn default() -> Self {
        ActivityBase {
            state: Cell::new(ActivityState::Queued),
            child: RefCell::new(None),
            next: RefCell::new(None),
            is_interruptible: Cell::new(true),
            child_has_priority: Cell::new(true),
            finishing: Cell::new(false),
            first_run_completed: Cell::new(false),
            last_run: Cell::new(false),
        }
    }
}

impl ActivityBase {
    pub fn state(&self) -> ActivityState {
        self.state.get()
    }

    pub fn is_canceling(&self) -> bool {
        self.state.get() == ActivityState::Canceling
    }

    pub fn child_activity(&self) -> Option<ActivityRef> {
        skip_done(self.child.borrow().clone())
    }

    pub fn next_activity(&self) -> Option<ActivityRef> {
        skip_done(self.next.borrow().clone())
    }

    /// `QueueChild`.
    pub fn queue_child(&self, activity: ActivityRef) {
        let child = self.child.borrow().clone();
        match child {
            Some(c) => queue(&c, Some(activity)),
            None => *self.child.borrow_mut() = Some(activity),
        }
    }

    /// `Queue`: appends to the end of the queue this activity belongs to.
    pub fn queue(&self, activity: Option<ActivityRef>) {
        let next = self.next.borrow().clone();
        match next {
            Some(n) => queue(&n, activity),
            None => *self.next.borrow_mut() = activity,
        }
    }

    /// `TickChild`: runs the child activity chain; true when no child remains.
    pub fn tick_child(&self, self_: &ActorRef) -> bool {
        let next = run_activity(self_, self.child_activity());
        let done = next.is_none();
        *self.child.borrow_mut() = next;
        done
    }

    /// Base implementation of `Cancel`.
    pub fn cancel(&self, self_: &ActorRef, keep_queue: bool) {
        if !keep_queue {
            *self.next.borrow_mut() = None;
        }
        if !self.is_interruptible.get() {
            return;
        }
        if let Some(c) = self.child_activity() {
            c.cancel(self_, false);
        }
        self.state.set(if self.state.get() == ActivityState::Queued {
            ActivityState::Done
        } else {
            ActivityState::Canceling
        });
    }
}

/// An actor activity. Override the hooks; `base()` returns the embedded [`ActivityBase`].
pub trait Activity: Any {
    fn base(&self) -> &ActivityBase;

    /// Returns true when the activity is complete.
    fn tick(&self, _self_: &ActorRef) -> bool {
        true
    }

    fn on_first_run(&self, _self_: &ActorRef) {}

    fn on_last_run(&self, _self_: &ActorRef) {}

    fn on_actor_dispose(&self, _self_: &ActorRef) {}

    fn cancel(&self, self_: &ActorRef, keep_queue: bool) {
        self.base().cancel(self_, keep_queue);
    }

    fn get_targets(&self, _self_: &ActorRef) -> Vec<Target> {
        Vec::new()
    }

    fn type_name(&self) -> &'static str;

    fn as_any(&self) -> &dyn Any;
}

/// Implements the accessor methods of [`Activity`] for a struct with a `base: ActivityBase` field.
#[macro_export]
macro_rules! activity_common {
    ($name:literal) => {
        fn base(&self) -> &$crate::activity::ActivityBase {
            &self.base
        }
        fn type_name(&self) -> &'static str {
            $name
        }
        fn as_any(&self) -> &dyn ::std::any::Any {
            self
        }
    };
}

/// Skips activities that were canceled while queued.
pub fn skip_done(mut first: Option<ActivityRef>) -> Option<ActivityRef> {
    while let Some(a) = &first {
        if a.base().state.get() != ActivityState::Done {
            break;
        }
        let next = a.base().next.borrow().clone();
        first = next;
    }
    first
}

/// Appends `activity` at the end of the queue starting at `this`.
pub fn queue(this: &ActivityRef, activity: Option<ActivityRef>) {
    let mut it = this.clone();
    loop {
        let next = it.base().next.borrow().clone();
        match next {
            Some(n) => it = n,
            None => break,
        }
    }
    *it.base().next.borrow_mut() = activity;
}

pub fn cancel(this: &ActivityRef, self_: &ActorRef, keep_queue: bool) {
    this.cancel(self_, keep_queue);
}

pub fn on_actor_dispose_outer(this: &ActivityRef, self_: &ActorRef) {
    if let Some(c) = this.base().child_activity() {
        on_actor_dispose_outer(&c, self_);
    }
    this.on_actor_dispose(self_);
}

/// `Activity.TickOuter`.
pub fn tick_outer(this: &ActivityRef, self_: &ActorRef) -> Option<ActivityRef> {
    let b = this.base();
    assert!(
        b.state.get() != ActivityState::Done,
        "Actor {self_:?} attempted to tick activity {} after it had already completed.",
        this.type_name()
    );
    if b.state.get() == ActivityState::Queued {
        this.on_first_run(self_);
        b.first_run_completed.set(true);
        b.state.set(ActivityState::Active);
    }
    assert!(b.first_run_completed.get(), "activity ticked before OnFirstRun");

    if b.child_has_priority.get() {
        let last = b.tick_child(self_) && (b.finishing.get() || this.tick(self_));
        b.last_run.set(last);
        b.finishing.set(b.finishing.get() | last);
    } else {
        b.last_run.set(this.tick(self_));
    }

    if let Some(ca) = b.child_activity()
        && ca.base().state.get() == ActivityState::Queued
    {
        if b.child_has_priority.get() {
            b.last_run.set(b.tick_child(self_) && b.finishing.get());
        } else {
            b.tick_child(self_);
        }
    }

    if b.last_run.get() {
        b.state.set(ActivityState::Done);
        this.on_last_run(self_);
        return b.next_activity();
    }
    Some(this.clone())
}

/// `ActivityUtils.RunActivity`.
pub fn run_activity(self_: &ActorRef, act: Option<ActivityRef>) -> Option<ActivityRef> {
    let mut act = act?;
    loop {
        let prev = act.clone();
        {
            let next = tick_outer(&act, self_)?;
            if Rc::ptr_eq(&next, &prev) {
                return Some(next);
            }
            act = next;
        }
    }
}

/// Walks the activity tree like `ActivitiesImplementing<T>`, calling `f` for each activity.
pub fn for_each_activity(first: Option<ActivityRef>, include_children: bool, f: &mut dyn FnMut(&ActivityRef)) {
    let Some(a) = first else { return };
    if include_children {
        for_each_activity(a.base().child_activity(), true, f);
    }
    f(&a);
    for_each_activity(a.base().next_activity(), include_children, f);
}
