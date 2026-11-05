// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of the Lua scripting of `OpenRA.Game/Scripting` and `OpenRA.Mods.Common/Scripting`.

pub mod context;
pub mod globals;
pub mod host;
pub mod lua_script;
pub mod members;
pub mod properties;
pub mod script_traits;
pub mod triggers;
pub mod values;

use rustora_engine::load_info;
use rustora_engine::rules::TraitRegistry;

pub fn register(r: &mut TraitRegistry) {
    r.register("LuaScript", |n| load_info!(lua_script::LuaScriptInfo, n));
    r.register("ScriptTriggers", |n| load_info!(triggers::ScriptTriggersInfo, n));
    r.register("ScriptTags", |n| load_info!(script_traits::ScriptTagsInfo, n));
    r.register("ScriptLobbyDropdown", |n| load_info!(script_traits::ScriptLobbyDropdownInfo, n));
}

#[cfg(test)]
mod tests {
    #[test]
    fn lua_version() {
        let lua = mlua::Lua::new();
        let v: String = lua.load("return _VERSION").eval().unwrap();
        assert_eq!(v, "Lua 5.1");
    }
}
