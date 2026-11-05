// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Scripting/Properties/*.cs`: the actor and player properties.

use std::rc::Rc;

use mlua::{Function, Lua};
use rustora_engine::activity::ActivityRef;
use rustora_engine::actor::ActorRef;
use rustora_engine::player::PlayerRef;
use rustora_mods_common::activities::CallFunc;

use crate::context::ctx;
use crate::members::Group;
use crate::values::LuaResult;

mod abilities;
mod combat;
mod general;
mod misc;
mod movement;
mod player;
mod production;

/// `CallLuaFunc`: an activity that calls `f` once without arguments, reporting errors as fatal.
pub fn call_lua_func(lua: &Lua, f: Function) -> LuaResult<ActivityRef> {
    let context = Rc::downgrade(&ctx(lua)?);
    Ok(CallFunc::new(Box::new(move || {
        if let Some(c) = context.upgrade() {
            c.call(&f, ());
        }
    })))
}

pub fn actor_groups() -> Vec<Group<ActorRef>> {
    let mut groups = Vec::new();
    groups.extend(general::groups());
    groups.extend(movement::groups());
    groups.extend(combat::groups());
    groups.extend(misc::actor_groups());
    groups.extend(production::actor_groups());
    groups.extend(abilities::actor_groups());
    groups
}

pub fn player_groups() -> Vec<Group<PlayerRef>> {
    let mut groups = Vec::new();
    groups.extend(player::groups());
    groups.extend(misc::player_groups());
    groups.extend(production::player_groups());
    groups
}
