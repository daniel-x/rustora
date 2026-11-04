// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The in-game state of the user interface and a port of `LoadIngamePlayerOrObserverUILogic.cs`.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;

use super::{NoLogic, services};
use crate::loader::cell;
use crate::viewport::Viewport;
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};

/// What in-game widgets read in place of the `World` and `WorldRenderer` arguments of C#.
pub struct Ingame {
    pub world: Rc<World>,
    pub viewport: Rc<RefCell<Viewport>>,
    /// `World.LocalPlayer`.
    pub local_player: Option<PlayerRef>,
}

/// Port of the constructor, without the game over handling.
pub fn load_player_or_observer_ui(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let world_root = ui.child(widget, "WORLD_ROOT");
    let player_root = ui.child(world_root, "PLAYER_ROOT");
    let has_local_player = services(ui)
        .ingame
        .borrow()
        .as_ref()
        .is_some_and(|ingame| ingame.local_player.is_some());
    let widgets = if has_local_player { "PLAYER_WIDGETS" } else { "OBSERVER_WIDGETS" };
    let args = WidgetArgs::default();
    ui.load_widget(widgets, Some(player_root), &args);
    ui.load_widget("DEBUG_WIDGETS", Some(world_root), &args);
    ui.load_widget("TRANSIENTS_PANEL", Some(world_root), &args);
    cell(NoLogic)
}
