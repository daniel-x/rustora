// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The in-game state of the user interface and a port of `LoadIngamePlayerOrObserverUILogic.cs`.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;
use rustora_game::settings::Settings;
use rustora_mods_common::selection::Selection;

use super::{NoLogic, services};
use crate::loader::cell;
use crate::orders::{OrderContext, OrderGenerator, UnitOrderGenerator};
use crate::screen_map::ScreenMap;
use crate::viewport::Viewport;
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::world_sprites::WorldSprites;

/// What in-game widgets read in place of the `World` and `WorldRenderer` arguments of C#.
pub struct Ingame {
    pub world: Rc<World>,
    pub viewport: Rc<RefCell<Viewport>>,
    /// `World.LocalPlayer`.
    pub local_player: Option<PlayerRef>,
    /// `World.RenderPlayer`.
    pub render_player: Option<PlayerRef>,
    pub screen_map: RefCell<ScreenMap>,
    pub selection: Rc<Selection>,
    pub world_sprites: Rc<dyn WorldSprites>,
    order_generator: RefCell<Rc<dyn OrderGenerator>>,
    world_default_cursor: String,
    default_order_generator: Rc<dyn OrderGenerator>,
}

impl Ingame {
    /// `world_select_cursor` and `world_default_cursor` are the chrome metrics `WorldSelectCursor` and
    /// `WorldDefaultCursor`.
    pub fn new(
        world: Rc<World>,
        viewport: Rc<RefCell<Viewport>>,
        local_player: Option<PlayerRef>,
        world_sprites: Rc<dyn WorldSprites>,
        world_select_cursor: String,
        world_default_cursor: String,
    ) -> Ingame {
        let selection = Selection::of(&world).unwrap_or_default();
        let default_order_generator: Rc<dyn OrderGenerator> = Rc::new(UnitOrderGenerator {
            world_select_cursor,
            world_default_cursor: world_default_cursor.clone(),
        });
        Ingame {
            world,
            viewport,
            render_player: local_player.clone(),
            local_player,
            screen_map: RefCell::default(),
            selection,
            world_sprites,
            order_generator: RefCell::new(default_order_generator.clone()),
            world_default_cursor,
            default_order_generator,
        }
    }

    /// `World.OrderGenerator`.
    pub fn order_generator(&self) -> Rc<dyn OrderGenerator> {
        self.order_generator.borrow().clone()
    }

    /// `World.CancelInputMode`.
    pub fn cancel_input_mode(&self) {
        self.set_order_generator(self.default_order_generator.clone());
    }

    /// Setting `World.OrderGenerator`: the old one is deactivated.
    pub fn set_order_generator(&self, generator: Rc<dyn OrderGenerator>) {
        let old = std::mem::replace(&mut *self.order_generator.borrow_mut(), generator);
        old.deactivate();
    }

    /// Runs `f` with the state that order generators read.
    pub fn with_order_context<R>(&self, settings: &Settings, f: impl FnOnce(&OrderContext) -> R) -> R {
        let screen_map = self.screen_map.borrow();
        let cancel = || self.cancel_input_mode();
        let ctx = OrderContext {
            world: &self.world,
            screen_map: &screen_map,
            selection: &self.selection,
            local_player: self.local_player.as_ref(),
            render_player: self.render_player.as_ref(),
            use_classic_mouse_style: settings.game.use_classic_mouse_style,
            selection_deadzone: settings.game.selection_deadzone,
            world_default_cursor: self.world_default_cursor.clone(),
            cancel_input_mode: &cancel,
        };
        f(&ctx)
    }
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
