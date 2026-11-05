// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `LuaScript.cs`: the world trait that runs the map scripts.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, register_traits};

use crate::context::ScriptContext;

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct LuaScriptInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub scripts: Vec<String>,
}

info_impl!(LuaScriptInfo => |info, _init| Some(Rc::new(LuaScript {
    info,
    context: RefCell::new(None),
})));

pub struct LuaScript {
    info: Rc<LuaScriptInfo>,
    context: RefCell<Option<Rc<ScriptContext>>>,
}

register_traits!(LuaScript: ITick, IWorldLoaded, INotifyActorDisposing);

impl LuaScript {
    pub fn context(&self) -> Option<Rc<ScriptContext>> {
        self.context.borrow().clone()
    }

    pub fn fatal_error_occurred(&self) -> bool {
        self.context().is_some_and(|c| c.fatal_error_occurred())
    }
}

impl IWorldLoaded for LuaScript {
    fn world_loaded(&self, world: &Rc<World>) {
        let mut scripts: Vec<String> = Vec::new();
        for s in &self.info.scripts {
            if !scripts.contains(s) {
                scripts.push(s.clone());
            }
        }
        let context = ScriptContext::new(world, &scripts);
        *self.context.borrow_mut() = Some(context.clone());
        context.world_loaded();
    }
}

impl ITick for LuaScript {
    fn tick(&self, _self_: &ActorRef) {
        if let Some(c) = self.context() {
            c.tick();
        }
    }
}

impl INotifyActorDisposing for LuaScript {
    fn disposing(&self, _self_: &ActorRef) {
        if let Some(c) = self.context.borrow_mut().take() {
            c.dispose();
        }
    }
}
