// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The in-game state of the user interface and a port of `LoadIngamePlayerOrObserverUILogic.cs`.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_engine::player::{PlayerRef, WinState};
use rustora_engine::world::World;
use rustora_game::settings::Settings;
use rustora_mods_common::selection::Selection;

use super::services;
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
        if let Some(groups) = rustora_mods_common::selection::ControlGroups::of(&world) {
            groups.set_local_player(local_player.clone());
        }
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

/// Port of `LoadIngamePlayerOrObserverUILogic`. The `World.GameOver` event is polled in `tick`, the mission videos
/// are not played, and the observer widgets after a win or loss stay mostly plain containers until slice 6 ends.
struct PlayerOrObserverUi {
    ingame: Option<Rc<Ingame>>,
    player_root: WidgetId,
    menu_root: WidgetId,
    /// The run time at which the observer widgets replace the player widgets.
    observer_switch_at: Option<i64>,
    switched: bool,
    game_over: bool,
}

impl ChromeLogic for PlayerOrObserverUi {
    fn tick(&mut self, ui: &mut Ui) {
        let Some(ingame) = self.ingame.clone() else { return };
        if let Some(player) = &ingame.local_player {
            if player.win_state.get() != WinState::Undefined && self.observer_switch_at.is_none() {
                let delay = player
                    .player_actor()
                    .info
                    .trait_info::<rustora_mods_common::player::objectives::MissionObjectivesInfo>()
                    .map_or(0, |o| o.game_over_delay);
                self.observer_switch_at = Some(ui.run_time() + delay as i64);
            }
            if !self.switched && self.observer_switch_at.is_some_and(|t| ui.run_time() >= t) {
                self.switched = true;
                ui.remove_children(self.player_root);
                ui.load_widget("OBSERVER_WIDGETS", Some(self.player_root), &WidgetArgs::default());
            }
        }
        if !self.game_over && ingame.world.is_game_over() {
            self.game_over = true;
            ui.close_window();
            ui.remove_children(self.menu_root);
            let options = ui.find(self.player_root, "OPTIONS_BUTTON");
            if let Some(click) = options.and_then(|b| ui.button(b).on_click.clone()) {
                click(ui);
            }
        }
    }
}

pub fn load_player_or_observer_ui(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let world_root = ui.child(widget, "WORLD_ROOT");
    let menu_root = ui.child(widget, "MENU_ROOT");
    let player_root = ui.child(world_root, "PLAYER_ROOT");
    let ingame = services(ui).ingame.borrow().clone();
    let has_local_player = ingame.as_ref().is_some_and(|i| i.local_player.is_some());
    let widgets = if has_local_player { "PLAYER_WIDGETS" } else { "OBSERVER_WIDGETS" };
    let args = WidgetArgs::default();
    ui.load_widget(widgets, Some(player_root), &args);
    ui.load_widget("DEBUG_WIDGETS", Some(world_root), &args);
    ui.load_widget("TRANSIENTS_PANEL", Some(world_root), &args);
    cell(PlayerOrObserverUi {
        ingame,
        player_root,
        menu_root,
        observer_switch_at: None,
        switched: false,
        game_over: false,
    })
}
