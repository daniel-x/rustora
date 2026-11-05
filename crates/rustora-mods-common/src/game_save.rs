// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The client side `IGameSaveTraitData` traits. `World.RequestGameSave` numbers traits by their position in the list
//! of all implementers, so `Selection` (in `selection.rs`), `ControlGroups` (in `selection.rs`) and `GameSaveViewportManager` must exist for the bot modules to
//! get the same numbers as in C# saves. Without a user interface yet they save empty state and ignore loaded state.

use std::rc::Rc;

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::IGameSaveTraitData;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("GameSaveViewportManager", |n| load_info!(GameSaveViewportManagerInfo, n));
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GameSaveViewportManagerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct GameSaveViewportManager;
register_traits!(GameSaveViewportManager: IGameSaveTraitData);
info_impl!(GameSaveViewportManagerInfo => |_info, _init| Some(Rc::new(GameSaveViewportManager)));

impl IGameSaveTraitData for GameSaveViewportManager {
    /// The viewport of a spectating host is stored on the first bot, like in C#. There is no viewport yet, so the
    /// position is the origin, which C# clamps to the map when it loads the save.
    fn issue_trait_data(&self, self_: &ActorRef) -> Option<Vec<MiniYamlNode>> {
        let owner = self_.owner();
        let first_bot = self_.world().players().into_iter().find(|p| p.is_bot)?;
        Rc::ptr_eq(&first_bot, &owner).then(|| vec![MiniYamlNode::with_value_str("Viewport", "0,0,0")])
    }

    fn resolve_trait_data(&self, _actor: &ActorRef, _data: &MiniYaml) {}
}
