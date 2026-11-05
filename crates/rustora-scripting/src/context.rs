// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ScriptContext.cs`: the sandboxed Lua 5.1 runtime of a world with its global tables and the map
//! scripts.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use mlua::{Function, IntoLuaMulti, Lua, Value};
use rustora_engine::actor::ActorRef;
use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;

use crate::members::{GlobalTable, Group, MemberTable, bind, filter};
use crate::values::{LuaActor, LuaResult, error};

const MAX_USER_SCRIPT_MEMORY: usize = 50 * 1024 * 1024;
const MAX_USER_SCRIPT_INSTRUCTIONS: i32 = 1_000_000;

const ALLOWED_GLOBALS: &[&str] = &[
    "ipairs", "next", "pairs", "pcall", "select", "tonumber", "tostring", "type", "unpack", "xpcall", "math", "string", "table",
];

pub struct ScriptContext {
    pub lua: Lua,
    pub world: Weak<World>,
    pub actor_groups: Rc<Vec<Group<ActorRef>>>,
    pub player_groups: Rc<Vec<Group<PlayerRef>>>,
    pub player_members: MemberTable,
    actor_members: RefCell<HashMap<(*const (), bool), Rc<MemberTable>>>,
    /// The state of the C# properties objects that hold fields, by actor ID and group key.
    group_state: RefCell<HashMap<(u32, &'static str), Rc<dyn std::any::Any>>>,
    fatal_error_occurred: Cell<bool>,
    pub error_message: RefCell<Option<String>>,
    tick: RefCell<Option<Function>>,
    disposed: Cell<bool>,
}

/// The context of the runtime `lua`.
pub fn ctx(lua: &Lua) -> LuaResult<Rc<ScriptContext>> {
    lua.app_data_ref::<Weak<ScriptContext>>()
        .and_then(|w| w.upgrade())
        .map_or_else(|| error("The script context is disposed"), Ok)
}

/// The world of the runtime `lua`.
pub fn world(lua: &Lua) -> LuaResult<Rc<World>> {
    ctx(lua)?.world()
}

impl ScriptContext {
    /// `new ScriptContext(world, worldRenderer, scripts)`.
    pub fn new(world: &Rc<World>, scripts: &[String]) -> Rc<ScriptContext> {
        let actor_groups = Rc::new(crate::properties::actor_groups());
        let player_groups = Rc::new(crate::properties::player_groups());
        let player_members = match world.rules().actor("player") {
            Some(info) => bind(&player_groups, filter(&player_groups, &info).into_iter(), |m| {
                format!("Player defines the command '{m}' on multiple traits")
            }),
            None => Ok(Default::default()),
        };
        let ctx = Rc::new(ScriptContext {
            lua: Lua::new(),
            world: Rc::downgrade(world),
            actor_groups,
            player_groups,
            player_members,
            actor_members: RefCell::new(HashMap::new()),
            group_state: RefCell::new(HashMap::new()),
            fatal_error_occurred: Cell::new(false),
            error_message: RefCell::new(None),
            tick: RefCell::new(None),
            disposed: Cell::new(false),
        });
        ctx.lua.set_app_data(Rc::downgrade(&ctx));
        if let Err(e) = ctx.setup(world) {
            ctx.fatal_error(&e);
            return ctx;
        }
        let _ = ctx.lua.set_memory_limit(ctx.lua.used_memory() + MAX_USER_SCRIPT_MEMORY);
        for script in scripts {
            let loaded = match world.map().open(script) {
                Some(bytes) => {
                    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes).to_vec();
                    ctx.lua.load(text).set_name(script.as_str()).exec()
                }
                None => error(format!("Could not open script '{script}'")),
            };
            if let Err(e) = loaded {
                ctx.fatal_error(&e);
                return ctx;
            }
        }
        *ctx.tick.borrow_mut() = ctx.lua.globals().get::<Option<Function>>("Tick").ok().flatten();
        ctx
    }

    fn setup(self: &Rc<Self>, world: &Rc<World>) -> LuaResult<()> {
        let lua = &self.lua;
        let globals = lua.globals();
        let keys: Vec<Value> = globals.pairs::<Value, Value>().filter_map(|p| p.ok().map(|(k, _)| k)).collect();
        for key in keys {
            let name = crate::values::key_string(&key);
            if !ALLOWED_GLOBALS.contains(&name.as_str()) {
                globals.raw_set(key, Value::Nil)?;
            }
        }
        let math: mlua::Table = globals.get("math")?;
        math.raw_set("random", Value::Nil)?;
        math.raw_set("randomseed", Value::Nil)?;
        globals.raw_set("EngineDir", crate::host::engine_dir())?;
        globals.raw_set(
            "FatalError",
            lua.create_function(|lua, message: String| {
                ctx(lua)?.fatal_error_message(&message);
                Ok(())
            })?,
        )?;
        globals.raw_set("MaxUserScriptInstructions", MAX_USER_SCRIPT_INSTRUCTIONS)?;
        globals.raw_set(
            "print",
            lua.create_function(|_, message: String| {
                println!("Lua debug: {message}");
                log::info!(target: "lua", "{message}");
                Ok(())
            })?,
        )?;
        for group in crate::globals::all(lua, world)? {
            let name = group.name;
            globals.raw_set(name, GlobalTable::new(group)?)?;
        }
        for (name, actor) in crate::globals::map_actors(world) {
            if globals.contains_key(name.as_str())? {
                return error(format!("The global name '{name}' is reserved, and may not be used by a map actor"));
            }
            globals.raw_set(name.as_str(), LuaActor(actor))?;
        }
        Ok(())
    }

    pub fn world(&self) -> LuaResult<Rc<World>> {
        self.world.upgrade().map_or_else(|| error("The world is disposed"), Ok)
    }

    /// The state of the properties group `key` of the actor `id`, created by `init` on first use.
    pub fn group_state<T: 'static>(&self, id: u32, key: &'static str, init: impl FnOnce() -> T) -> Rc<T> {
        if let Some(s) = self.group_state.borrow().get(&(id, key)) {
            return s.clone().downcast::<T>().expect("group state type");
        }
        let state = Rc::new(init());
        self.group_state.borrow_mut().insert((id, key), state.clone());
        state
    }

    /// The bound members of `actor` (`ScriptActorInterface`), cached per actor type and disposed state.
    pub fn actor_members(&self, actor: &ActorRef) -> Rc<MemberTable> {
        let disposed = actor.disposed();
        let key = (Rc::as_ptr(&actor.info) as *const (), disposed);
        if let Some(t) = self.actor_members.borrow().get(&key) {
            return t.clone();
        }
        let groups = &self.actor_groups;
        let selected = filter(groups, &actor.info)
            .into_iter()
            .filter(|&g| !disposed || groups[g].exposed_for_destroyed);
        let name = actor.info.name.clone();
        let table = Rc::new(bind(groups, selected, |m| {
            format!("Actor '{name}' defines the command '{m}' on multiple traits")
        }));
        self.actor_members.borrow_mut().insert(key, table.clone());
        table
    }

    pub fn fatal_error_occurred(&self) -> bool {
        self.fatal_error_occurred.get()
    }

    fn end_game(&self) {
        self.fatal_error_occurred.set(true);
        if let Some(world) = self.world.upgrade() {
            world.add_frame_end_task(Box::new(|w: &Rc<World>| w.end_game()));
        }
    }

    /// `FatalError(Exception)`.
    pub fn fatal_error(&self, e: &mlua::Error) {
        let message = e.to_string();
        println!("Fatal Lua Error: {message}");
        log::error!(target: "lua", "Fatal Lua Error: {message}");
        *self.error_message.borrow_mut() = Some(message);
        self.end_game();
    }

    /// `FatalError(string)`, the Lua global.
    fn fatal_error_message(&self, message: &str) {
        println!("Fatal Lua Error: {message}");
        log::error!(target: "lua", "{message}");
        self.end_game();
    }

    /// Calls `f`, reporting an error as a fatal error. Returns whether the call succeeded.
    pub fn call(&self, f: &Function, args: impl IntoLuaMulti) -> bool {
        match f.call::<()>(args) {
            Ok(()) => true,
            Err(e) => {
                self.fatal_error(&e);
                false
            }
        }
    }

    /// `WorldLoaded`.
    pub fn world_loaded(&self) {
        if self.fatal_error_occurred.get() {
            return;
        }
        if let Ok(Some(f)) = self.lua.globals().get::<Option<Function>>("WorldLoaded") {
            self.call(&f, ());
        }
    }

    /// `Tick`.
    pub fn tick(&self) {
        if self.fatal_error_occurred.get() || self.disposed.get() {
            return;
        }
        let tick = self.tick.borrow().clone();
        if let Some(f) = tick {
            self.call(&f, ());
        }
    }

    pub fn dispose(&self) {
        self.disposed.set(true);
    }
}
