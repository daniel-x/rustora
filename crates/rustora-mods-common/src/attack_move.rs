// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Activities/Move/AttackMoveActivity.cs` (target scan pending the combat port).

use std::rc::Rc;

use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::actor::ActorRef;
use rustora_engine::activity_common;

pub struct AttackMoveActivity {
    base: ActivityBase,
    get_move: Rc<dyn Fn() -> ActivityRef>,
}

impl AttackMoveActivity {
    pub fn new(_self_: &ActorRef, get_move: Rc<dyn Fn() -> ActivityRef>, _assault_moving: bool) -> ActivityRef {
        let base = ActivityBase::default();
        base.child_has_priority.set(false);
        Rc::new(AttackMoveActivity { base, get_move })
    }
}

impl Activity for AttackMoveActivity {
    activity_common!("AttackMoveActivity");

    fn on_first_run(&self, _self_: &ActorRef) {
        self.base.queue_child((self.get_move)());
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        self.base.tick_child(self_)
    }
}
