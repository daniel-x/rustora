// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `ScriptTags.cs` and `ScriptLobbyDropdown.cs`.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_mods_common::actor_reference::UnknownInit;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, register_traits, value_init};

value_init!(ScriptTagsInit, Vec<String>);

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ScriptTagsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct ScriptTags {
    tags: RefCell<IndexSet<String>>,
}

register_traits!(ScriptTags);
info_impl!(ScriptTagsInfo => |info, init| {
    let name = info.instance_name.as_deref();
    let mut tags = IndexSet::new();
    if let Some(i) = init.inits.get_for::<ScriptTagsInit>(name) {
        tags.extend(i.value.iter().cloned());
    } else if let Some(i) = init
        .inits
        .0
        .iter()
        .filter_map(|i| i.as_any().downcast_ref::<UnknownInit>())
        .filter(|i| i.name == "ScriptTags")
        .last()
    {
        tags.extend(i.yaml.value_str().split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from));
    }
    Some(Rc::new(ScriptTags { tags: RefCell::new(tags) }))
});

impl ScriptTags {
    pub fn add_tag(&self, tag: &str) -> bool {
        self.tags.borrow_mut().insert(tag.to_string())
    }

    pub fn remove_tag(&self, tag: &str) -> bool {
        self.tags.borrow_mut().shift_remove(tag)
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.borrow().contains(tag)
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ScriptLobbyDropdownInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(name = "ID")]
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub default: String,
    pub values: IndexMap<String, String>,
    pub locked: bool,
    pub visible: bool,
    pub display_order: i32,
}

impl Default for ScriptLobbyDropdownInfo {
    fn default() -> Self {
        ScriptLobbyDropdownInfo {
            instance_name: None,
            id: String::new(),
            label: String::new(),
            description: None,
            default: String::new(),
            values: IndexMap::new(),
            locked: false,
            visible: true,
            display_order: 0,
        }
    }
}

pub struct ScriptLobbyDropdown {
    pub info: Rc<ScriptLobbyDropdownInfo>,
    value: RefCell<String>,
}

register_traits!(ScriptLobbyDropdown: INotifyCreated);
info_impl!(ScriptLobbyDropdownInfo => |info, _| Some(Rc::new(ScriptLobbyDropdown {
    info,
    value: RefCell::new(String::new()),
})));

impl ScriptLobbyDropdown {
    pub fn value(&self) -> String {
        self.value.borrow().clone()
    }
}

impl INotifyCreated for ScriptLobbyDropdown {
    fn created(&self, self_: &ActorRef) {
        let world = self_.world();
        *self.value.borrow_mut() = world.lobby_info.global_settings.option_str(&self.info.id, &self.info.default).to_string();
    }
}
