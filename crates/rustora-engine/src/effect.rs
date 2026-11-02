// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! World effects (`IEffect`), such as projectiles.

use std::any::Any;
use std::rc::Rc;

use crate::world::World;

pub type EffectRef = Rc<dyn Effect>;

pub trait Effect: Any {
    fn tick(&self, world: &Rc<World>);
    /// Whether the effect implements `ISync`.
    fn is_sync(&self) -> bool {
        false
    }
    fn sync_hash(&self) -> i32 {
        0
    }
    fn as_any(&self) -> &dyn Any;
}
