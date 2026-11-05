// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ReinforcementsGlobal.cs`.

use std::rc::{Rc, Weak};

use mlua::{Function, IntoLuaMulti, Lua};
use rustora_core::world_types::{CPos, WAngle, WDist, WVec};
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::*;
use rustora_engine::player::PlayerRef;
use rustora_engine::target::Target;
use rustora_engine::world::World;
use rustora_mods_common::activities::CallFunc;
use rustora_mods_common::aircraft::{Aircraft, AircraftInfo, Land, LandArgs, RemoveSelf};
use rustora_mods_common::cargo::{Cargo, UnloadCargo};
use rustora_mods_common::effects::SpawnActorEffect;
use rustora_mods_common::interfaces::IMove;

use crate::context::{ScriptContext, ctx, world};
use crate::members::Group;
use crate::values::{LuaActor, LuaResult, actors_table, error};

fn create_actor(
    w: &Rc<World>,
    owner: &PlayerRef,
    actor_type: &str,
    add_to_world: bool,
    entry: Option<CPos>,
    next: Option<CPos>,
) -> LuaResult<ActorRef> {
    let Some(ai) = w.rules().actor(actor_type) else {
        return error(format!("Unknown actor type '{actor_type}'"));
    };
    let mut inits = ActorInits::new();
    inits.push(Rc::new(OwnerInit::from_player(owner)));
    if let Some(entry) = entry {
        inits.push(Rc::new(LocationInit::new(entry)));
        if let Some(pi) = ai.trait_info::<AircraftInfo>() {
            let pos = w.map().center_of_cell(entry) + WVec::new(0, 0, pi.cruise_altitude.length);
            inits.push(Rc::new(CenterPositionInit::new(pos)));
        }
        if let Some(next) = next {
            let facing = w.map().facing_between(CPos::ZERO, CPos::ZERO + (next - entry), WAngle::ZERO);
            inits.push(Rc::new(FacingInit::new(facing)));
        }
    }
    let a = w.create_actor_add(false, actor_type, inits);
    if add_to_world {
        let added = a.clone();
        w.add_frame_end_task(Box::new(move |w: &Rc<World>| w.add(&added)));
    }
    Ok(a)
}

fn move_to(actor: &ActorRef, dest: CPos) {
    if let Some(m) = actor.trait_or_default::<dyn IMove>() {
        actor.queue_activity(m.move_to(dest, 2, None, false));
    }
}

/// A `CallFunc` activity that calls `f` with the arguments from `args`, reporting errors as fatal.
fn call_func<A: IntoLuaMulti>(
    context: Weak<ScriptContext>,
    f: Function,
    args: impl FnOnce() -> A + 'static,
) -> rustora_engine::activity::ActivityRef {
    CallFunc::new(Box::new(move || {
        if let Some(c) = context.upgrade() {
            c.call(&f, args());
        }
    }))
}

fn reinforce(lua: &Lua, a: &crate::values::Args) -> LuaResult<mlua::Value> {
    let owner: PlayerRef = a.get(0)?;
    let types: Vec<String> = a.get(1)?;
    let path: Vec<CPos> = a.get(2)?;
    let interval: i32 = a.opt(3, 25)?;
    let action: Option<Function> = a.opt(4, None)?;
    let w = world(lua)?;
    let context = Rc::downgrade(&ctx(lua)?);
    let Some(&entry) = path.first() else {
        return error("Index was outside the bounds of the array.");
    };
    let mut actors = Vec::new();
    for (i, ty) in types.iter().enumerate() {
        let actor = create_actor(&w, &owner, ty, false, Some(entry), path.get(1).copied())?;
        actors.push(actor.clone());
        let delay = (i as i32).wrapping_mul(interval);
        let queued = action.clone().map(|f| {
            let target = actor.clone();
            call_func(context.clone(), f, move || LuaActor(target))
        });
        let rest = path[1..].to_vec();
        w.add_frame_end_task(Box::new(move |w: &Rc<World>| {
            w.add_effect(SpawnActorEffect::new(actor, delay, rest, queued));
        }));
    }
    actors_table(lua, &actors)
}

fn reinforce_with_transport(lua: &Lua, a: &crate::values::Args) -> LuaResult<mlua::Value> {
    let owner: PlayerRef = a.get(0)?;
    let actor_type: String = a.get(1)?;
    let cargo_types: Option<Vec<String>> = a.get(2)?;
    let entry_path: Vec<CPos> = a.get(3)?;
    let exit_path: Option<Vec<CPos>> = a.opt(4, None)?;
    let action: Option<Function> = a.opt(5, None)?;
    let exit: Option<Function> = a.opt(6, None)?;
    let drop_range: i32 = a.opt(7, 3)?;
    let w = world(lua)?;
    let context = Rc::downgrade(&ctx(lua)?);
    let (Some(&entry), Some(&last)) = (entry_path.first(), entry_path.last()) else {
        return error("Index was outside the bounds of the array.");
    };
    let transport = create_actor(&w, &owner, &actor_type, true, Some(entry), entry_path.get(1).copied())?;
    let cargo = transport.trait_or_default::<Cargo>();
    let mut passengers = Vec::new();
    if let (Some(cargo), Some(types)) = (&cargo, &cargo_types) {
        for ty in types {
            let passenger = create_actor(&w, &owner, ty, false, Some(entry), None)?;
            passengers.push(passenger.clone());
            cargo.load(&transport, &passenger);
        }
    }
    for &cell in &entry_path[1..] {
        move_to(&transport, cell);
    }
    if let Some(f) = action {
        let (t, p) = (transport.clone(), passengers.clone());
        let lua_ref = lua.clone();
        transport.queue_activity(call_func(context.clone(), f, move || {
            (LuaActor(t), actors_table(&lua_ref, &p).unwrap_or(mlua::Value::Nil))
        }));
    } else {
        if transport.trait_or_default::<Aircraft>().is_some() {
            let args = LandArgs {
                land_range: WDist::from_cells(drop_range),
                ..LandArgs::default()
            };
            transport.queue_activity(Land::full(&transport, Target::from_cell(&w, last, 0), args));
        }
        if cargo.is_some() {
            transport.queue_activity(UnloadCargo::new(&transport, WDist::from_cells(drop_range)));
        }
    }
    if let Some(f) = exit {
        let t = transport.clone();
        transport.queue_activity(call_func(context, f, move || LuaActor(t)));
    } else if let Some(exit_path) = exit_path {
        for cell in exit_path {
            move_to(&transport, cell);
        }
        transport.queue_activity(RemoveSelf::new());
    }
    let ret = lua.create_table()?;
    ret.set(1, LuaActor(transport))?;
    ret.set(2, actors_table(lua, &passengers)?)?;
    Ok(mlua::Value::Table(ret))
}

pub fn global() -> Group<()> {
    Group::new("Reinforcements", &[])
        .method("Reinforce", |lua, _, a| reinforce(lua, a))
        .method("ReinforceWithTransport", |lua, _, a| reinforce_with_transport(lua, a))
}
