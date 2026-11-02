// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Map/ActorInitializer.cs`: initial values passed to newly created actors.

use std::any::Any;
use std::rc::Rc;

use rustora_core::world_types::{CPos, WAngle, WPos};

use crate::actor::ActorRef;
use crate::player::PlayerRef;
use crate::world::World;

pub trait ActorInit: Any {
    fn instance_name(&self) -> Option<&str> {
        None
    }
    fn as_any(&self) -> &dyn Any;
}

/// Init values of an actor in definition order.
#[derive(Clone, Default)]
pub struct ActorInits(pub Vec<Rc<dyn ActorInit>>);

impl ActorInits {
    pub fn new() -> Self {
        ActorInits(Vec::new())
    }

    pub fn with(mut self, init: impl ActorInit) -> Self {
        self.0.push(Rc::new(init));
        self
    }

    pub fn push(&mut self, init: Rc<dyn ActorInit>) {
        self.0.push(init);
    }

    fn of_type<T: 'static>(&self) -> impl Iterator<Item = &T> {
        self.0.iter().filter_map(|i| i.as_any().downcast_ref::<T>())
    }

    /// `GetOrDefault<T>()` for single instance inits.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.of_type::<T>().last()
    }

    /// `GetOrDefault<T>(info)`: inits with the trait's instance name win, then unnamed ones.
    pub fn get_for<T: ActorInit + 'static>(&self, instance_name: Option<&str>) -> Option<&T> {
        if let Some(n) = instance_name.filter(|n| !n.is_empty()) {
            if let Some(i) = self.of_type::<T>().filter(|i| i.instance_name() == Some(n)).last() {
                return Some(i);
            }
        }
        self.of_type::<T>().filter(|i| i.instance_name().is_none_or(str::is_empty)).last()
    }

    pub fn contains<T: 'static>(&self) -> bool {
        self.get::<T>().is_some()
    }
}

/// Passed to `TraitInfo::create`.
pub struct ActorInitializer {
    pub self_: ActorRef,
    pub inits: ActorInits,
}

impl ActorInitializer {
    pub fn new(self_: ActorRef, inits: ActorInits) -> Self {
        ActorInitializer { self_, inits }
    }

    pub fn world(&self) -> Rc<World> {
        self.self_.world()
    }

    pub fn owner(&self, world: &World) -> Option<PlayerRef> {
        self.inits.get::<OwnerInit>().map(|o| o.value(world))
    }
}

/// Declares a value init struct: `value_init!(LocationInit, CPos);`
#[macro_export]
macro_rules! value_init {
    ($name:ident, $t:ty) => {
        #[derive(Clone, Debug)]
        pub struct $name {
            pub value: $t,
            pub instance_name: Option<String>,
        }

        impl $name {
            pub fn new(value: $t) -> Self {
                $name {
                    value,
                    instance_name: None,
                }
            }
        }

        impl $crate::actor_init::ActorInit for $name {
            fn instance_name(&self) -> Option<&str> {
                self.instance_name.as_deref()
            }
            fn as_any(&self) -> &dyn ::std::any::Any {
                self
            }
        }
    };
}

value_init!(LocationInit, CPos);
value_init!(FacingInit, WAngle);
value_init!(CenterPositionInit, WPos);
value_init!(SubCellInit, u8);
value_init!(FactionInit, String);

/// Owner by player object or by internal name.
#[derive(Clone)]
pub struct OwnerInit {
    pub internal_name: String,
    player: Option<PlayerRef>,
}

impl OwnerInit {
    pub fn from_player(p: &PlayerRef) -> Self {
        OwnerInit {
            internal_name: p.internal_name.clone(),
            player: Some(p.clone()),
        }
    }

    pub fn from_name(n: &str) -> Self {
        OwnerInit {
            internal_name: n.to_string(),
            player: None,
        }
    }

    pub fn value(&self, world: &World) -> PlayerRef {
        self.player.clone().unwrap_or_else(|| {
            world
                .players()
                .into_iter()
                .find(|p| p.internal_name == self.internal_name)
                .expect("owner player exists")
        })
    }
}

impl ActorInit for OwnerInit {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
