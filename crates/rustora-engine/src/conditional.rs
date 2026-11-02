// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ConditionalTrait` and `PausableConditionalTrait` (`Traits/Conditions`).
//!
//! Concrete traits embed [`ConditionalState`] and implement [`ConditionalHooks`].

use std::cell::Cell;
use std::rc::{Rc, Weak};

use rustora_core::expression::{BooleanExpression, NoVariables};
use rustora_derive::FieldLoad;

use crate::actor::{ActorRef, ConditionCache};
use crate::traits::VariableObserver;

/// Fields of `ConditionalTraitInfo`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ConditionalTraitInfo {
    pub requires_condition: Option<BooleanExpression>,
}

impl ConditionalTraitInfo {
    pub fn enabled_by_default(&self) -> bool {
        self.requires_condition.as_ref().is_none_or(|c| c.evaluate(&NoVariables))
    }
}

/// Fields of `PausableConditionalTraitInfo`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PausableConditionalTraitInfo {
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub pause_on_condition: Option<BooleanExpression>,
}

impl PausableConditionalTraitInfo {
    pub fn paused_by_default(&self) -> bool {
        self.pause_on_condition.as_ref().is_some_and(|c| c.evaluate(&NoVariables))
    }
}

/// Virtual hooks of the conditional base classes.
pub trait ConditionalHooks: 'static {
    fn conditional(&self) -> &ConditionalState;
    fn trait_enabled(&self, _self_: &ActorRef) {}
    fn trait_disabled(&self, _self_: &ActorRef) {}
    fn trait_resumed(&self, _self_: &ActorRef) {}
    fn trait_paused(&self, _self_: &ActorRef) {}
}

/// Runtime state of a (pausable) conditional trait.
pub struct ConditionalState {
    pub requires_condition: Option<BooleanExpression>,
    pub pause_on_condition: Option<BooleanExpression>,
    pub pausable: bool,
    is_trait_disabled: Cell<bool>,
    is_trait_paused: Cell<bool>,
}

impl ConditionalState {
    pub fn new(info: &ConditionalTraitInfo) -> Self {
        ConditionalState {
            requires_condition: info.requires_condition.clone(),
            pause_on_condition: None,
            pausable: false,
            is_trait_disabled: Cell::new(info.requires_condition.is_some()),
            is_trait_paused: Cell::new(false),
        }
    }

    pub fn new_pausable(info: &PausableConditionalTraitInfo) -> Self {
        ConditionalState {
            pause_on_condition: info.pause_on_condition.clone(),
            pausable: true,
            is_trait_paused: Cell::new(info.paused_by_default()),
            ..ConditionalState::new(&info.base)
        }
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.is_trait_disabled.get()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.is_trait_paused.get()
    }

    /// `[Sync]` contribution of `IsTraitDisabled` (and `IsTraitPaused` for pausable traits).
    pub fn sync_hash(&self) -> i32 {
        let mut h = sync_bool(self.is_trait_disabled.get());
        if self.pausable {
            h ^= sync_bool(self.is_trait_paused.get());
        }
        h
    }
}

/// Sync hash value of a `[Sync]` bool. The C# IL generator intends 0xaaa/0x555 but branches on
/// the pushed constant instead of the value, so the bool itself (0 or 1) ends up hashed.
pub fn sync_bool(b: bool) -> i32 {
    i32::from(b)
}

/// `ConditionalTrait.Created` (+ the pausable part); call from `INotifyCreated::created`.
pub fn conditional_created<T: ConditionalHooks>(t: &T, self_: &ActorRef) {
    let s = t.conditional();
    if s.requires_condition.is_none() {
        t.trait_enabled(self_);
    }
    if s.pausable && s.pause_on_condition.is_none() {
        t.trait_resumed(self_);
    }
}

/// `GetVariableObservers` of the conditional base classes.
pub fn conditional_observers<T: ConditionalHooks>(t: &Rc<T>) -> Vec<VariableObserver> {
    let mut out = Vec::new();
    let s = t.conditional();
    if let Some(rc) = &s.requires_condition {
        let w: Weak<T> = Rc::downgrade(t);
        out.push(VariableObserver {
            notifier: Rc::new(move |self_: &ActorRef, c: &ConditionCache| {
                if let Some(t) = w.upgrade() {
                    required_conditions_changed(&*t, self_, c);
                }
            }),
            variables: rc.variables().iter().cloned().collect(),
        });
    }
    if s.pausable {
        if let Some(pc) = &s.pause_on_condition {
            let w: Weak<T> = Rc::downgrade(t);
            out.push(VariableObserver {
                notifier: Rc::new(move |self_: &ActorRef, c: &ConditionCache| {
                    if let Some(t) = w.upgrade() {
                        pause_conditions_changed(&*t, self_, c);
                    }
                }),
                variables: pc.variables().iter().cloned().collect(),
            });
        }
    }
    out
}

fn required_conditions_changed<T: ConditionalHooks>(t: &T, self_: &ActorRef, c: &ConditionCache) {
    let s = t.conditional();
    let Some(rc) = &s.requires_condition else { return };
    let was = s.is_trait_disabled.get();
    let now = !rc.evaluate(c);
    s.is_trait_disabled.set(now);
    if now != was {
        if was { t.trait_enabled(self_) } else { t.trait_disabled(self_) }
    }
}

fn pause_conditions_changed<T: ConditionalHooks>(t: &T, self_: &ActorRef, c: &ConditionCache) {
    let s = t.conditional();
    let Some(pc) = &s.pause_on_condition else { return };
    let was = s.is_trait_paused.get();
    let now = pc.evaluate(c);
    s.is_trait_paused.set(now);
    if now != was {
        if was { t.trait_resumed(self_) } else { t.trait_paused(self_) }
    }
}
