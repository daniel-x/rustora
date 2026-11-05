// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `TriggerGlobal.cs`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use mlua::{Function, IntoLuaMulti, Lua};
use rustora_core::world_types::{CPos, WDist, WPos};
use rustora_engine::actor::ActorRef;
use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;
use rustora_mods_common::actor_map::WorldExt;
use rustora_mods_common::bridges::DelayedAction;

use crate::context::{ScriptContext, ctx};
use crate::members::Group;
use crate::triggers::{ScriptTriggers, Trigger};
use crate::values::{Args, LuaActor, LuaResult, error};

/// `TriggerGlobal.GetScriptTriggers`.
pub fn script_triggers(actor: &ActorRef) -> LuaResult<Rc<ScriptTriggers>> {
    actor.trait_or_default::<ScriptTriggers>().map_or_else(
        || {
            error(format!(
                "Actor '{}' requires the ScriptTriggers trait before attaching a trigger",
                actor.info.name
            ))
        },
        Ok,
    )
}

/// A copied function reference that is disposed after its first use, as `using (f)` in C#. A later call fails
/// as a call of a disposed `LuaFunction` does.
struct Once {
    f: RefCell<Option<Function>>,
    context: Weak<ScriptContext>,
}

impl Once {
    fn new(f: Function, context: &Rc<ScriptContext>) -> Rc<Once> {
        Rc::new(Once {
            f: RefCell::new(Some(f)),
            context: Rc::downgrade(context),
        })
    }

    /// Calls and disposes the function; returns whether the call succeeded.
    fn call(&self, args: impl IntoLuaMulti) -> bool {
        let Some(context) = self.context.upgrade() else { return false };
        let f = self.f.borrow_mut().take();
        match f {
            Some(f) => context.call(&f, args),
            None => {
                context.fatal_error(&mlua::Error::RuntimeError(
                    "Cannot access a disposed object. Object name: 'LuaFunction'.".into(),
                ));
                false
            }
        }
    }
}

/// Calls `f` with the fatal error handling of the trigger closures.
fn call(context: &Weak<ScriptContext>, f: &Function, args: impl IntoLuaMulti) {
    if let Some(context) = context.upgrade() {
        context.call(f, args);
    }
}

fn register(lua: &Lua, args: &Args, trigger: Trigger) -> LuaResult<()> {
    let actor: ActorRef = args.get(0)?;
    let f: Function = args.get(1)?;
    script_triggers(&actor)?.register_callback(trigger, f, &ctx(lua)?);
    Ok(())
}

fn register_player(lua: &Lua, args: &Args, trigger: Trigger) -> LuaResult<()> {
    let player: PlayerRef = args.get(0)?;
    let f: Function = args.get(1)?;
    script_triggers(player.player_actor())?.register_callback(trigger, f, &ctx(lua)?);
    Ok(())
}

fn register_world(lua: &Lua, args: &Args, trigger: Trigger) -> LuaResult<()> {
    let f: Function = args.get(0)?;
    let world = crate::context::world(lua)?;
    script_triggers(world.world_actor())?.register_callback(trigger, f, &ctx(lua)?);
    Ok(())
}

fn remove_one(group: &RefCell<Vec<ActorRef>>, m: &ActorRef) -> bool {
    let mut g = group.borrow_mut();
    match g.iter().position(|a| Rc::ptr_eq(a, m)) {
        Some(i) => {
            g.remove(i);
            true
        }
        None => false,
    }
}

fn footprint(lua: &Lua, args: &Args, entry: bool) -> LuaResult<i32> {
    let cells: Vec<CPos> = args.get(0)?;
    let f: Function = args.get(1)?;
    let context = Rc::downgrade(&ctx(lua)?);
    let world = crate::context::world(lua)?;
    let id = Rc::new(Cell::new(0));
    let trigger_id = id.clone();
    let callback: Rc<dyn Fn(&ActorRef)> = Rc::new(move |a| call(&context, &f, (LuaActor(a.clone()), trigger_id.get())));
    let actor_map = world.actor_map();
    let value = if entry {
        actor_map.add_cell_trigger(cells, Some(callback), None)
    } else {
        actor_map.add_cell_trigger(cells, None, Some(callback))
    };
    id.set(value);
    Ok(value)
}

fn proximity(lua: &Lua, args: &Args, entry: bool) -> LuaResult<i32> {
    let pos: WPos = args.get(0)?;
    let range: WDist = args.get(1)?;
    let f: Function = args.get(2)?;
    let context = Rc::downgrade(&ctx(lua)?);
    let world = crate::context::world(lua)?;
    let id = Rc::new(Cell::new(0));
    let trigger_id = id.clone();
    let callback: Rc<dyn Fn(&ActorRef)> = Rc::new(move |a| call(&context, &f, (LuaActor(a.clone()), trigger_id.get())));
    let actor_map = world.actor_map();
    let value = if entry {
        actor_map.add_proximity_trigger(pos, range, WDist::ZERO, Some(callback), None)
    } else {
        actor_map.add_proximity_trigger(pos, range, WDist::ZERO, None, Some(callback))
    };
    id.set(value);
    Ok(value)
}

pub fn global() -> Group<()> {
    Group::new("Trigger", &[])
        .method("AfterDelay", |lua, _, a| {
            let delay: i32 = a.get(0)?;
            let f: Function = a.get(1)?;
            let context = Rc::downgrade(&ctx(lua)?);
            let world = crate::context::world(lua)?;
            world.add_frame_end_task(Box::new(move |w: &Rc<World>| {
                w.add_effect(DelayedAction::new(delay, Box::new(move |_| call(&context, &f, ()))));
            }));
            Ok(())
        })
        .method("OnPassengerEntered", |lua, _, a| register(lua, a, Trigger::OnPassengerEntered))
        .method("OnPassengerExited", |lua, _, a| register(lua, a, Trigger::OnPassengerExited))
        .method("OnIdle", |lua, _, a| register(lua, a, Trigger::OnIdle))
        .method("OnDamaged", |lua, _, a| register(lua, a, Trigger::OnDamaged))
        .method("OnKilled", |lua, _, a| register(lua, a, Trigger::OnKilled))
        .method("OnAllKilled", |lua, _, a| {
            let actors: Vec<ActorRef> = a.get(0)?;
            let f = Once::new(a.get(1)?, &ctx(lua)?);
            let group = Rc::new(RefCell::new(actors.clone()));
            for actor in &actors {
                let (group, f) = (group.clone(), f.clone());
                script_triggers(actor)?.on_killed_internal.borrow_mut().push(Rc::new(move |m| {
                    remove_one(&group, m);
                    if group.borrow().is_empty() {
                        f.call(());
                    }
                }));
            }
            Ok(())
        })
        .method("OnAnyKilled", |lua, _, a| {
            let f = Once::new(a.get(1)?, &ctx(lua)?);
            let called = Rc::new(Cell::new(false));
            let actors: Vec<ActorRef> = a.get(0)?;
            for actor in &actors {
                let (called, f) = (called.clone(), f.clone());
                script_triggers(actor)?.on_killed_internal.borrow_mut().push(Rc::new(move |m| {
                    if called.get() {
                        return;
                    }
                    if f.call(LuaActor(m.clone())) {
                        called.set(true);
                    }
                }));
            }
            Ok(())
        })
        .method("OnProduction", |lua, _, a| register(lua, a, Trigger::OnProduction))
        .method("OnAnyProduction", |lua, _, a| register_world(lua, a, Trigger::OnOtherProduction))
        .method("OnPlayerWon", |lua, _, a| register_player(lua, a, Trigger::OnPlayerWon))
        .method("OnPlayerLost", |lua, _, a| register_player(lua, a, Trigger::OnPlayerLost))
        .method("OnObjectiveAdded", |lua, _, a| register_player(lua, a, Trigger::OnObjectiveAdded))
        .method("OnObjectiveCompleted", |lua, _, a| {
            register_player(lua, a, Trigger::OnObjectiveCompleted)
        })
        .method("OnObjectiveFailed", |lua, _, a| register_player(lua, a, Trigger::OnObjectiveFailed))
        .method("OnAddedToWorld", |lua, _, a| register(lua, a, Trigger::OnAddedToWorld))
        .method("OnRemovedFromWorld", |lua, _, a| register(lua, a, Trigger::OnRemovedFromWorld))
        .method("OnAllRemovedFromWorld", |lua, _, a| {
            let actors: Rc<Vec<ActorRef>> = Rc::new(a.get(0)?);
            let f: Function = a.get(1)?;
            let context = Rc::downgrade(&ctx(lua)?);
            let group = Rc::new(RefCell::new(actors.to_vec()));
            for actor in actors.iter() {
                let triggers = script_triggers(actor)?;
                let (g, f, context) = (group.clone(), f.clone(), context.clone());
                triggers.on_removed_internal.borrow_mut().push(Rc::new(move |m| {
                    if remove_one(&g, m) && g.borrow().is_empty() {
                        call(&context, &f, ());
                    }
                }));
                let (g, all) = (group.clone(), actors.clone());
                triggers.on_added_internal.borrow_mut().push(Rc::new(move |m| {
                    let known = all.iter().any(|a| Rc::ptr_eq(a, m));
                    let present = g.borrow().iter().any(|a| Rc::ptr_eq(a, m));
                    if known && !present {
                        g.borrow_mut().push(m.clone());
                    }
                }));
            }
            Ok(())
        })
        .method("OnCapture", |lua, _, a| register(lua, a, Trigger::OnCapture))
        .method("OnKilledOrCaptured", |lua, _, a| {
            let f = Once::new(a.get(1)?, &ctx(lua)?);
            let called = Rc::new(Cell::new(false));
            let actor: ActorRef = a.get(0)?;
            let handler: Rc<dyn Fn(&ActorRef)> = Rc::new(move |_| {
                if !called.get() && f.call(()) {
                    called.set(true);
                }
            });
            let triggers = script_triggers(&actor)?;
            triggers.on_captured_internal.borrow_mut().push(handler.clone());
            triggers.on_killed_internal.borrow_mut().push(handler);
            Ok(())
        })
        .method("OnAllKilledOrCaptured", |lua, _, a| {
            let actors: Vec<ActorRef> = a.get(0)?;
            let f = Once::new(a.get(1)?, &ctx(lua)?);
            let group = Rc::new(RefCell::new(actors.clone()));
            let handler: Rc<dyn Fn(&ActorRef)> = Rc::new(move |m| {
                if remove_one(&group, m) && group.borrow().is_empty() {
                    f.call(());
                }
            });
            for actor in &actors {
                let triggers = script_triggers(actor)?;
                triggers.on_captured_internal.borrow_mut().push(handler.clone());
                triggers.on_killed_internal.borrow_mut().push(handler.clone());
            }
            Ok(())
        })
        .method("OnEnteredFootprint", |lua, _, a| footprint(lua, a, true))
        .method("OnExitedFootprint", |lua, _, a| footprint(lua, a, false))
        .method("RemoveFootprintTrigger", |lua, _, a| {
            crate::context::world(lua)?.actor_map().remove_cell_trigger(a.get(0)?);
            Ok(())
        })
        .method("OnEnteredProximityTrigger", |lua, _, a| proximity(lua, a, true))
        .method("OnExitedProximityTrigger", |lua, _, a| proximity(lua, a, false))
        .method("RemoveProximityTrigger", |lua, _, a| {
            crate::context::world(lua)?.actor_map().remove_proximity_trigger(a.get(0)?);
            Ok(())
        })
        .method("OnInfiltrated", |lua, _, a| register(lua, a, Trigger::OnInfiltrated))
        .method("OnDiscovered", |lua, _, a| register(lua, a, Trigger::OnDiscovered))
        .method("OnPlayerDiscovered", |lua, _, a| {
            register_player(lua, a, Trigger::OnPlayerDiscovered)
        })
        .method("OnSold", |lua, _, a| register(lua, a, Trigger::OnSold))
        .method("OnTimerExpired", |lua, _, a| register_world(lua, a, Trigger::OnTimerExpired))
        .method("ClearAll", |_, _, a| {
            let actor: ActorRef = a.get(0)?;
            script_triggers(&actor)?.clear_all();
            Ok(())
        })
        .method("Clear", |_, _, a| {
            let name: String = a.get(1)?;
            let Some(trigger) = Trigger::parse(&name) else {
                return error(format!("Requested value '{name}' was not found."));
            };
            let actor: ActorRef = a.get(0)?;
            script_triggers(&actor)?.clear(trigger);
            Ok(())
        })
}
