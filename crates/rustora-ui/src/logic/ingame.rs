// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The in-game state of the user interface and a port of `LoadIngamePlayerOrObserverUILogic.cs`.

use std::cell::{Cell, RefCell};
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
    render_player: RefCell<Option<PlayerRef>>,
    pub screen_map: RefCell<ScreenMap>,
    pub selection: Rc<Selection>,
    pub world_sprites: Rc<dyn WorldSprites>,
    order_generator: RefCell<Rc<dyn OrderGenerator>>,
    /// The selected actor ids of the previous frame, to notice newly selected actors for their target lines.
    pub target_line_selection: RefCell<std::collections::HashSet<u32>>,
    world_default_cursor: String,
    default_order_generator: Rc<dyn OrderGenerator>,
    unit_order_generator: UnitOrderGenerator,
    /// The state of a replay, `None` for a game.
    pub replay: Option<ReplayStatus>,
}

/// What `ReplayControlBarLogic` reads of a `ReplayConnection` and its `OrderManager`.
pub struct ReplayStatus {
    /// `ReplayConnection.TickCount`: the last net frame of the replay.
    pub tick_count: i32,
    /// `ReplayConnection.FinalGameTick`: the last world tick from the metadata, 0 if unknown.
    pub final_game_tick: i32,
    /// `OrderManager.IsOutOfSync`.
    pub out_of_sync: Cell<bool>,
}

impl Ingame {
    /// `World.RenderPlayer`.
    pub fn render_player(&self) -> Option<PlayerRef> {
        self.render_player.borrow().clone()
    }

    /// The `World.RenderPlayer` setter: only observers and players with an unlocked render player can change it.
    pub fn set_render_player(&self, player: Option<PlayerRef>) {
        if self.local_player.as_ref().is_none_or(|p| p.unlocked_render_player()) {
            *self.render_player.borrow_mut() = player;
        }
    }

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
        world.sound.set_local_player(local_player.clone());
        if let Some(groups) = rustora_mods_common::selection::ControlGroups::of(&world) {
            groups.set_local_player(local_player.clone());
        }
        let unit_order_generator = UnitOrderGenerator {
            world_select_cursor,
            world_default_cursor: world_default_cursor.clone(),
        };
        let default_order_generator: Rc<dyn OrderGenerator> = Rc::new(unit_order_generator.clone());
        Ingame {
            world,
            viewport,
            render_player: RefCell::new(local_player.clone()),
            local_player,
            screen_map: RefCell::default(),
            selection,
            world_sprites,
            order_generator: RefCell::new(default_order_generator.clone()),
            target_line_selection: RefCell::new(std::collections::HashSet::new()),
            world_default_cursor,
            default_order_generator,
            unit_order_generator,
            replay: None,
        }
    }

    /// A new `UnitOrderGenerator`, as the base of the generators that derive from it in C#.
    pub fn unit_order_generator(&self) -> UnitOrderGenerator {
        self.unit_order_generator.clone()
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

    /// `IOrderGenerator.RenderAboveShroud` of the active order generator.
    pub fn order_generator_renderables(&self) -> Vec<rustora_engine::render::Renderable> {
        let generator = self.order_generator();
        let mut out = Vec::new();
        self.order_context(false, 0, |ctx| generator.render_above_shroud(ctx, &mut out));
        out
    }

    /// Runs `f` with the state that order generators read.
    pub fn with_order_context<R>(&self, settings: &Settings, f: impl FnOnce(&OrderContext) -> R) -> R {
        self.order_context(settings.game.use_classic_mouse_style, settings.game.selection_deadzone, f)
    }

    fn order_context<R>(&self, use_classic_mouse_style: bool, selection_deadzone: i32, f: impl FnOnce(&OrderContext) -> R) -> R {
        let screen_map = self.screen_map.borrow();
        let render_player = self.render_player();
        let cancel = || self.cancel_input_mode();
        let ctx = OrderContext {
            world: &self.world,
            screen_map: &screen_map,
            selection: &self.selection,
            local_player: self.local_player.as_ref(),
            render_player: render_player.as_ref(),
            use_classic_mouse_style,
            selection_deadzone,
            world_default_cursor: self.world_default_cursor.clone(),
            cancel_input_mode: &cancel,
        };
        f(&ctx)
    }
}

/// Port of `LoadIngamePlayerOrObserverUILogic`. The `World.GameOver` event is polled in `tick`, and the observer widgets after a win or loss stay mostly plain containers until slice 6 ends.
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
            if let Some(player) = &ingame.local_player {
                play_game_over_video(ui, &ingame.world, player);
            }
            let options = ui.find(self.player_root, "OPTIONS_BUTTON");
            if let Some(click) = options.and_then(|b| ui.button(b).on_click.clone()) {
                click(ui);
            }
        }
    }
}

/// The `MissionData` win or loss video of the `World.GameOver` handler, unless a script failed.
fn play_game_over_video(ui: &mut Ui, world: &Rc<World>, player: &PlayerRef) {
    let Some(mission) = world.world_actor().info.trait_info::<rustora_mods_common::player::objectives::MissionDataInfo>() else {
        return;
    };
    let failed = world
        .world_actor()
        .trait_or_default::<rustora_scripting::lua_script::LuaScript>()
        .is_some_and(|s| s.fatal_error_occurred());
    if failed {
        return;
    }
    let video = if player.win_state.get() == WinState::Won { &mission.win_video } else { &mission.loss_video };
    if let Some(video) = video.as_deref().filter(|v| !v.is_empty()) {
        crate::logic::media::play_fmv_fullscreen(ui, world, video, Box::new(|_| {}));
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
