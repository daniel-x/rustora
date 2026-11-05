// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Scripting/Global/*.cs`: the global tables.

use std::rc::Rc;

use mlua::Lua;
use rustora_engine::actor::ActorRef;
use rustora_engine::world::World;
use rustora_mods_common::world::SpawnMapActors;

use crate::members::Group;
use crate::values::LuaResult;

pub mod map;
pub mod media;
pub mod reinforcements;
pub mod trigger;
pub mod utils;

/// The global tables.
pub fn all(_lua: &Lua, _world: &Rc<World>) -> LuaResult<Vec<Group<()>>> {
    let mut groups = vec![map::actor(), utils::angle(), media::beacon(), media::camera(), media::hsl_color()];
    groups.extend(utils::coordinates());
    groups.extend([
        utils::date_time(),
        media::lighting(),
        map::map(),
        media::media(),
        map::player(),
        media::radar(),
        reinforcements::global(),
        trigger::global(),
        media::user_interface(),
        utils::utils(),
    ]);
    Ok(groups)
}

/// The named map actors registered as globals by the `MapGlobal` constructor.
pub fn map_actors(world: &Rc<World>) -> Vec<(String, ActorRef)> {
    world
        .world_actor()
        .trait_or_default::<SpawnMapActors>()
        .map(|s| s.actors.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}
