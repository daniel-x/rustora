// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `MenuButtonsChromeLogic.cs` and `IngameMenuLogic.cs`. The menu has the abort, surrender and resume
//! buttons; restart, load, save, music, settings and the editor buttons, the menu fade effect
//! and the leave notification are not ported.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::miniyaml::MiniYamlNode;
use rustora_engine::order::Order;
use rustora_engine::player::WinState;
use rustora_engine::target::Target;
use rustora_engine::world::WorldType;

use super::game_info::{HideMenu, IngameInfoPanel, cheats_enabled};
use super::{NoLogic, button_prompt_with_cancel, message, services};
use crate::loader::cell;
use crate::widget::{Action, AppRequest, ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::menu_button::MenuButtonWidget;

/// `World.SetPauseState`.
fn set_pause_state(world: &rustora_engine::world::World, paused: bool) {
    let mut o = Order::new("PauseGame", None, Target::Invalid, false);
    o.target_string = Some(if paused { "Pause" } else { "UnPause" }.to_string());
    world.issue_order(o);
}

fn single_non_bot_client(world: &rustora_engine::world::World) -> bool {
    world.lobby_info.clients.iter().filter(|c| c.bot.is_none()).count() == 1
}

/// `MenuButtonsChromeLogic`.
pub fn menu_buttons(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let disable_system_buttons = Rc::new(Cell::new(false));
    if let Some(options) = ui.find(id, "OPTIONS_BUTTON") {
        let d = disable_system_buttons.clone();
        let mut b = ui.button(options);
        b.is_disabled = Some(Rc::new(move || d.get()));
        let d = disable_system_buttons.clone();
        let ingame = ingame.clone();
        b.on_click = Some(Rc::new(move |ui: &mut Ui| {
            open_menu_panel(ui, options, &ingame, d.clone(), IngameInfoPanel::AutoSelect)
        }));
    }
    if let Some(debug) = ui.find(id, "DEBUG_BUTTON") {
        let enabled = cheats_enabled(&ingame.world);
        ui.set_visible_fn(debug, move || enabled);
        let d = disable_system_buttons.clone();
        let mut b = ui.button(debug);
        b.is_disabled = Some(Rc::new(move || d.get()));
        let d = disable_system_buttons.clone();
        let ingame = ingame.clone();
        b.on_click = Some(Rc::new(move |ui: &mut Ui| {
            open_menu_panel(ui, debug, &ingame, d.clone(), IngameInfoPanel::Debug)
        }));
    }
    cell(NoLogic)
}

/// `OpenMenuPanel`.
fn open_menu_panel(
    ui: &mut Ui,
    button: WidgetId,
    ingame: &Rc<crate::logic::ingame::Ingame>,
    disable: Rc<Cell<bool>>,
    initial_panel: IngameInfoPanel,
) {
    disable.set(true);
    let (container, pause, hide_ingame_ui) = {
        let b = ui.get::<MenuButtonWidget>(button);
        (b.menu_container.clone(), b.pause, b.hide_ingame_ui)
    };
    let world = ingame.world.clone();
    let cached_pause = world.paused();
    let world_root = ui.find(ui.root, "WORLD_ROOT");
    if hide_ingame_ui {
        ingame.cancel_input_mode();
        if let Some(r) = world_root {
            ui.set_visible_fn(r, || false);
        }
    }
    let pauses = pause && single_non_bot_client(&world);
    if pauses {
        set_pause_state(&world, true);
    }
    let menu_root = ui.find(ui.root, "MENU_ROOT").unwrap_or(ui.root);
    let current: Rc<Cell<Option<WidgetId>>> = Rc::default();
    let c = current.clone();
    let on_exit: Action = Rc::new(move |ui: &mut Ui| {
        if hide_ingame_ui && let Some(r) = world_root {
            ui.set_visible_fn(r, || true);
        }
        if pauses {
            set_pause_state(&world, cached_pause);
        }
        if let Some(w) = c.get() {
            ui.remove_child(menu_root, w);
        }
        disable.set(false);
    });
    let args = WidgetArgs::default()
        .with_action("onExit", on_exit)
        .with_value("initialPanel", initial_panel);
    let widget = ui.load_widget(&container, Some(menu_root), &args);
    current.set(Some(widget));
    ui.run_after_tick(|ui: &mut Ui| ui.reset_tooltips());
}

/// `IngameMenuLogic`.
pub fn ingame_menu(ui: &mut Ui, id: WidgetId, args: &WidgetArgs, logic_args: &[MiniYamlNode]) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let on_exit = args.action("onExit");
    let world = ingame.world.clone();
    let menu = if ui.node(id).id.as_deref() == Some("INGAME_MENU") {
        id
    } else {
        ui.child(id, "INGAME_MENU")
    };
    let container = ui.child(menu, "MENU_BUTTONS");
    let template = ui.child(container, "BUTTON_TEMPLATE");
    ui.hide_child(container, template);
    let hide_menu = Rc::new(Cell::new(false));
    let leaving = Rc::new(Cell::new(false));
    {
        let h = hide_menu.clone();
        ui.set_visible_fn(container, move || !h.get());
    }
    let arg = |key: &str| {
        logic_args
            .iter()
            .find(|n| n.key == key)
            .and_then(|n| n.value.value.clone())
            .unwrap_or_default()
    };
    let stride = {
        let v: Vec<i32> = arg("ButtonStride").split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0))
    };
    let is_single_player = !world.lobby_info.global_settings.dedicated && single_non_bot_client(&world);

    let close_menu: Action = {
        let on_exit = on_exit.clone();
        Rc::new(move |ui: &mut Ui| {
            ui.close_window();
            on_exit(ui);
            ui.reset_tooltips();
        })
    };
    let show_menu: Action = {
        let h = hide_menu.clone();
        Rc::new(move |_ui: &mut Ui| h.set(false))
    };
    let mut buttons: Vec<WidgetId> = Vec::new();
    let mut add_button = |ui: &mut Ui, button_id: &str, label: &str| -> WidgetId {
        let button = ui.clone_widget(template);
        if let Some(&last) = buttons.last() {
            let lb = ui.node(last).bounds;
            ui.node_mut(button).bounds.x = lb.x + stride.0;
            ui.node_mut(button).bounds.y = lb.y + stride.1;
        }
        ui.node_mut(button).id = Some(button_id.to_string());
        let text = message(ui, label);
        let l = leaving.clone();
        {
            let mut b = ui.button(button);
            b.is_disabled = Some(Rc::new(move || l.get()));
            b.get_text = Some(Rc::new(move || text.clone()));
        }
        ui.add_child(container, button);
        buttons.push(button);
        button
    };

    for name in arg("Buttons").split(',').map(str::trim) {
        match name {
            "ABORT_MISSION" if world.world_type == WorldType::Regular => {
                let label = if world.is_game_over() {
                    "menu-ingame.leave"
                } else {
                    "menu-ingame.abort"
                };
                let button = add_button(ui, name, label);
                let (h, l, show) = (hide_menu.clone(), leaving.clone(), show_menu.clone());
                ui.button(button).on_click = Some(Rc::new(move |ui: &mut Ui| {
                    h.set(true);
                    let l = l.clone();
                    let on_confirm: Action = Rc::new(move |ui: &mut Ui| {
                        ui.requests.push(AppRequest::LeaveGame);
                        l.set(true);
                    });
                    button_prompt_with_cancel(
                        ui,
                        "dialog-leave-mission.title",
                        &message(ui, "dialog-leave-mission.prompt"),
                        on_confirm,
                        "dialog-leave-mission.confirm",
                        Some(show.clone()),
                        Some("dialog-leave-mission.cancel"),
                    );
                }));
            }
            "SURRENDER" if world.world_type == WorldType::Regular && !is_single_player => {
                let Some(player) = ingame.local_player.clone() else { continue };
                let button = add_button(ui, name, "menu-ingame.surrender");
                let (l, p) = (leaving.clone(), player.clone());
                ui.button(button).is_disabled = Some(Rc::new(move || p.win_state.get() != WinState::Undefined || l.get()));
                let (h, show, close, world) = (hide_menu.clone(), show_menu.clone(), close_menu.clone(), world.clone());
                ui.button(button).on_click = Some(Rc::new(move |ui: &mut Ui| {
                    h.set(true);
                    let (close, world, player) = (close.clone(), world.clone(), player.clone());
                    let on_confirm: Action = Rc::new(move |ui: &mut Ui| {
                        world.issue_order(Order::new("Surrender", Some(player.player_actor().clone()), Target::Invalid, false));
                        close(ui);
                    });
                    button_prompt_with_cancel(
                        ui,
                        "dialog-surrender.title",
                        &message(ui, "dialog-surrender.prompt"),
                        on_confirm,
                        "dialog-surrender.confirm",
                        Some(show.clone()),
                        Some("dialog-surrender.cancel"),
                    );
                }));
            }
            "SETTINGS" => {
                let button = add_button(ui, name, "menu-ingame.settings");
                let h = hide_menu.clone();
                ui.button(button).on_click = Some(Rc::new(move |ui: &mut Ui| {
                    h.set(true);
                    let h = h.clone();
                    let args = WidgetArgs::default().with_action("onExit", Rc::new(move |_| h.set(false)));
                    ui.open_window("SETTINGS_PANEL", args);
                }));
            }
            "RESUME" => {
                let label = if world.is_game_over() {
                    "menu-ingame.return-to-map"
                } else {
                    "menu-ingame.resume"
                };
                let button = add_button(ui, name, label);
                let mut b = ui.button(button);
                b.key = Some("escape".into());
                b.on_click = Some(close_menu.clone());
            }
            _ => {}
        }
    }
    if !buttons.is_empty() {
        let n = buttons.len() as i32 - 1;
        let b = &mut ui.node_mut(container).bounds;
        b.x -= n * stride.0 / 2;
        b.y -= n * stride.1 / 2;
        b.w += n * stride.0;
        b.h += n * stride.1;
    }
    if let Some(panel_root) = ui.find(id, "PANEL_ROOT")
        && world.world_type != WorldType::Editor
    {
        let h = hide_menu.clone();
        let request_hide: HideMenu = Rc::new(move |hide| h.set(hide));
        let initial_panel = args.value::<IngameInfoPanel>("initialPanel").unwrap_or(IngameInfoPanel::AutoSelect);
        let args = WidgetArgs::default()
            .with_value("initialPanel", initial_panel)
            .with_value("hideMenu", request_hide)
            .with_action("closeMenu", close_menu.clone());
        let panel = ui.load_widget("GAME_INFO_PANEL", Some(panel_root), &args);
        let h = hide_menu.clone();
        ui.set_visible_fn(panel, move || !h.get());
    }
    cell(NoLogic)
}
