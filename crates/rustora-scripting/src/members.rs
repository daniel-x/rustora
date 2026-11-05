// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ScriptObjectWrapper.cs`, `ScriptMemberWrapper.cs`, `ScriptActorInterface.cs`,
//! `ScriptPlayerInterface.cs` and `ScriptGlobal`: named groups of methods and properties bound to an actor, a
//! player or a global table.

use std::rc::Rc;

use indexmap::IndexMap;
use mlua::{IntoLuaMulti, Lua, MetaMethod, MultiValue, UserData, UserDataMethods, Value};
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::player::PlayerRef;

use crate::context::ctx;
use crate::values::{Args, FromScript, LuaResult, error, key_string, type_name};

pub type MethodFn<T> = Rc<dyn Fn(&Lua, &T, &Args) -> LuaResult<Value>>;
pub type GetFn<T> = Rc<dyn Fn(&Lua, &T) -> LuaResult<Value>>;
pub type SetFn<T> = Rc<dyn Fn(&Lua, &T, &Value) -> LuaResult<()>>;

pub enum Member<T> {
    Method(MethodFn<T>),
    Property(Option<GetFn<T>>, Option<SetFn<T>>),
}

/// One C# properties class (`ScriptActorProperties`, `ScriptPlayerProperties` or `ScriptGlobal`).
pub struct Group<T> {
    pub name: &'static str,
    /// The C# info type names of its `Requires<T>` interfaces.
    pub requires: Vec<&'static str>,
    /// `[ExposedForDestroyedActors]`.
    pub exposed_for_destroyed: bool,
    pub members: Vec<(&'static str, Member<T>)>,
}

/// The first value of `r`, as a C# method returns one value (`void` gives `nil`).
fn first<R: IntoLuaMulti>(lua: &Lua, r: R) -> LuaResult<Value> {
    Ok(r.into_lua_multi(lua)?.into_iter().next().unwrap_or(Value::Nil))
}

impl<T: 'static> Group<T> {
    pub fn new(name: &'static str, requires: &[&'static str]) -> Self {
        Group {
            name,
            requires: requires.to_vec(),
            exposed_for_destroyed: false,
            members: Vec::new(),
        }
    }

    pub fn exposed_for_destroyed(mut self) -> Self {
        self.exposed_for_destroyed = true;
        self
    }

    pub fn method<R: IntoLuaMulti>(mut self, name: &'static str, f: impl Fn(&Lua, &T, &Args) -> LuaResult<R> + 'static) -> Self {
        let f: MethodFn<T> = Rc::new(move |lua, t, args| first(lua, f(lua, t, args)?));
        self.members.push((name, Member::Method(f)));
        self
    }

    pub fn get<R: IntoLuaMulti>(mut self, name: &'static str, f: impl Fn(&Lua, &T) -> LuaResult<R> + 'static) -> Self {
        let f: GetFn<T> = Rc::new(move |lua, t| first(lua, f(lua, t)?));
        self.members.push((name, Member::Property(Some(f), None)));
        self
    }

    /// A read and write property; the setter receives the value converted to `V`.
    pub fn get_set<R: IntoLuaMulti, V: FromScript>(
        mut self,
        name: &'static str,
        get: impl Fn(&Lua, &T) -> LuaResult<R> + 'static,
        set: impl Fn(&Lua, &T, V) -> LuaResult<()> + 'static,
    ) -> Self {
        let get: GetFn<T> = Rc::new(move |lua, t| first(lua, get(lua, t)?));
        let set: SetFn<T> = Rc::new(move |lua, t, v| match V::from_script(v) {
            Some(v) => set(lua, t, v),
            None => error(format!("Unable to convert '{}' to CLR type '{}'", type_name(v), V::NAME)),
        });
        self.members.push((name, Member::Property(Some(get), Some(set))));
        self
    }
}

/// The members of the bound groups by name, or the duplicate key error of `ScriptObjectWrapper.Bind`.
pub type MemberTable = Result<IndexMap<&'static str, (usize, usize)>, String>;

pub fn bind<T>(groups: &[Group<T>], selected: impl Iterator<Item = usize>, duplicate: impl Fn(&str) -> String) -> MemberTable {
    let mut table = IndexMap::new();
    for g in selected {
        for (m, (name, _)) in groups[g].members.iter().enumerate() {
            if table.insert(*name, (g, m)).is_some() {
                return Err(duplicate(name));
            }
        }
    }
    Ok(table)
}

/// `ScriptContext.FilterCommands`: the groups whose required trait infos all exist on `info`.
pub fn filter<T>(groups: &[Group<T>], info: &ActorInfo) -> Vec<usize> {
    (0..groups.len())
        .filter(|&g| groups[g].requires.iter().all(|r| info.has_trait_info_assignable_to(r)))
        .collect()
}

fn get_member<T: Clone + 'static>(
    lua: &Lua,
    groups: &Rc<Vec<Group<T>>>,
    (g, m): (usize, usize),
    target: &T,
    name: &'static str,
) -> LuaResult<Value> {
    match &groups[g].members[m].1 {
        Member::Method(f) => {
            let f = f.clone();
            let target = target.clone();
            Ok(Value::Function(lua.create_function(move |lua, args: MultiValue| {
                f(lua, &target, &Args::new(args, name))
            })?))
        }
        Member::Property(Some(get), _) => get(lua, target),
        Member::Property(None, _) => error(format!("The property '{name}' is write-only")),
    }
}

fn set_member<T>(lua: &Lua, groups: &[Group<T>], (g, m): (usize, usize), target: &T, name: &str, v: Value) -> LuaResult<()> {
    match &groups[g].members[m].1 {
        Member::Property(_, Some(set)) => set(lua, target, &v),
        _ => error(format!("The property '{name}' is read-only")),
    }
}

/// `Actor.this[]` get.
pub fn actor_index(lua: &Lua, actor: &ActorRef, name: &str) -> LuaResult<Value> {
    let ctx = ctx(lua)?;
    let table = ctx.actor_members(actor);
    let table = table.as_ref().as_ref().map_err(|e| mlua::Error::RuntimeError(e.clone()))?;
    match table.get_full(name) {
        Some((_, key, &index)) => get_member(lua, &ctx.actor_groups, index, actor, key),
        None => error(actor_not_found(actor, name)),
    }
}

/// `Actor.this[]` set.
pub fn actor_new_index(lua: &Lua, actor: &ActorRef, name: &str, v: Value) -> LuaResult<()> {
    let ctx = ctx(lua)?;
    let table = ctx.actor_members(actor);
    let table = table.as_ref().as_ref().map_err(|e| mlua::Error::RuntimeError(e.clone()))?;
    match table.get(name) {
        Some(&index) => set_member(lua, &ctx.actor_groups, index, actor, name, v),
        None => error(actor_not_found(actor, name)),
    }
}

fn actor_not_found(actor: &ActorRef, name: &str) -> String {
    let mut actor_name = actor.info.name.clone();
    if actor.is_dead() {
        actor_name += " (dead)";
    }
    format!("Actor '{actor_name}' does not define a property '{name}'")
}

/// `Actor.HasScriptProperty`.
pub fn actor_has_property(lua: &Lua, actor: &ActorRef, name: &str) -> LuaResult<bool> {
    let ctx = ctx(lua)?;
    let table = ctx.actor_members(actor);
    let table = table.as_ref().as_ref().map_err(|e| mlua::Error::RuntimeError(e.clone()))?;
    Ok(table.contains_key(name))
}

/// `Player.this[]` get.
pub fn player_index(lua: &Lua, player: &PlayerRef, name: &str) -> LuaResult<Value> {
    let ctx = ctx(lua)?;
    let table = ctx.player_members.as_ref().map_err(|e| mlua::Error::RuntimeError(e.clone()))?;
    match table.get_full(name) {
        Some((_, key, &index)) => get_member(lua, &ctx.player_groups, index, player, key),
        None => error(player_not_found(lua, player, name)),
    }
}

/// `Player.this[]` set.
pub fn player_new_index(lua: &Lua, player: &PlayerRef, name: &str, v: Value) -> LuaResult<()> {
    let ctx = ctx(lua)?;
    let table = ctx.player_members.as_ref().map_err(|e| mlua::Error::RuntimeError(e.clone()))?;
    match table.get(name) {
        Some(&index) => set_member(lua, &ctx.player_groups, index, player, name, v),
        None => error(player_not_found(lua, player, name)),
    }
}

fn player_not_found(lua: &Lua, player: &PlayerRef, name: &str) -> String {
    format!(
        "Player '{}' does not define a property '{name}'",
        crate::host::resolved_player_name(lua, player)
    )
}

/// A `ScriptGlobal` table.
pub struct GlobalTable {
    pub group: Rc<Vec<Group<()>>>,
    pub table: IndexMap<&'static str, (usize, usize)>,
}

impl GlobalTable {
    pub fn new(group: Group<()>) -> LuaResult<GlobalTable> {
        let name = group.name;
        let groups = Rc::new(vec![group]);
        let table = bind(&groups, 0..1, |m| format!("Table '{name}' defines multiple members '{m}'"))
            .map_err(mlua::Error::RuntimeError)?;
        Ok(GlobalTable { group: groups, table })
    }

    fn name(&self) -> &'static str {
        self.group[0].name
    }
}

impl UserData for GlobalTable {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| {
            let name = key_string(&key);
            match this.table.get_full(name.as_str()) {
                Some((_, key, &index)) => get_member(lua, &this.group, index, &(), key),
                None => error(format!("Table '{}' does not define a property '{name}'", this.name())),
            }
        });
        m.add_meta_method(MetaMethod::NewIndex, |lua, this, (key, v): (Value, Value)| {
            let name = key_string(&key);
            match this.table.get(name.as_str()) {
                Some(&index) => set_member(lua, &this.group, index, &(), &name, v),
                None => error(format!("Table '{}' does not define a property '{name}'", this.name())),
            }
        });
    }
}
