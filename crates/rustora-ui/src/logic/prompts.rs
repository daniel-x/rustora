// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `CreditsLogic.cs`, `DebugLogic.cs`, `IntroductionPromptLogic.cs` and `ScriptErrorLogic.cs`. `SystemInfoPromptLogic` has
//! no port, since rustora sends no system information.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_scripting::lua_script::LuaScript;

use super::settings::{
    adjust_scroll_panel_layout, bind_battlefield_camera, bind_checkbox, bind_mouse_control, bind_mouse_control_descs, bind_player_color,
    bind_player_name, bind_ui_scale, save,
};
use super::{NoLogic, services};
use crate::loader::cell;
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::LabelWidget;

/// The engine credits: the `AUTHORS` file of rustora, which includes the list of the OpenRA authors.
const ENGINE_CREDITS: &str = include_str!("../../../../AUTHORS");

/// `IntroductionPromptLogic.IntroductionVersion`.
const INTRODUCTION_VERSION: i32 = 1;

/// `CreditsLogic`. Only the engine tab exists, as `ModCredits.ModCreditsFile` is not set in the RA mod, so the tabs
/// stay hidden.
pub fn credits(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let panel = ui.child(widget, "CREDITS_PANEL");
    let on_exit = args.action("onExit");
    let back = ui.child(panel, "BACK_BUTTON");
    ui.button(back).on_click = Some(Rc::new(move |ui| {
        ui.close_window();
        on_exit(ui);
    }));
    let tabs = ui.child(panel, "TAB_CONTAINER");
    ui.set_visible_fn(tabs, || false);
    let scroll_panel = ui.child(panel, "CREDITS_DISPLAY");
    let template = ui.child(scroll_panel, "CREDITS_TEMPLATE");
    let lines = ENGINE_CREDITS.replace("\r\n", "\n").replace('\t', "    ").replace('*', "\u{2022}");
    let label = ui.clone_widget(template);
    ui.remove_children(scroll_panel);
    ui.label(label).get_text = Some(Rc::new(move || lines.clone()));
    LabelWidget::increase_height_to_fit_current_text(ui, label);
    ui.add_child(scroll_panel, label);
    cell(NoLogic)
}

/// `IntroductionPromptLogic.ShouldShowPrompt`.
pub fn should_show_introduction(ui: &Ui) -> bool {
    services(ui).settings.borrow().game.introduction_prompt_version < INTRODUCTION_VERSION
}

/// `IntroductionPromptLogic`, shown in the main menu, where the shell map runs.
pub fn introduction(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let on_complete = args.action("onComplete");
    bind_player_name(ui, widget, false);
    bind_mouse_control_descs(ui, widget);
    bind_mouse_control(ui, widget);
    bind_checkbox(
        ui,
        widget,
        "EDGESCROLL_CHECKBOX",
        |s| s.game.viewport_edge_scroll,
        |s, v| s.game.viewport_edge_scroll = v,
    );
    bind_player_color(ui, widget, false);
    bind_battlefield_camera(ui, widget);
    bind_ui_scale(ui, widget, false);
    bind_checkbox(
        ui,
        widget,
        "CURSORDOUBLE_CHECKBOX",
        |s| s.graphics.cursor_double,
        |s, v| s.graphics.cursor_double = v,
    );
    let continue_button = ui.child(widget, "CONTINUE_BUTTON");
    ui.button(continue_button).on_click = Some(Rc::new(move |ui| {
        services(ui).settings.borrow_mut().game.introduction_prompt_version = INTRODUCTION_VERSION;
        save(ui);
        ui.close_window();
        on_complete(ui);
    }));
    let scroll_panel = ui.child(widget, "SETTINGS_SCROLLPANEL");
    adjust_scroll_panel_layout(ui, scroll_panel);
    cell(NoLogic)
}

/// `ScriptErrorLogic`.
pub fn script_error(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let panel = ui.child(widget, "SCRIPT_ERROR_MESSAGE_PANEL");
    let label = ui.child(widget, "SCRIPT_ERROR_MESSAGE");
    let Some(context) = ingame.world.world_actor().trait_or_default::<LuaScript>().and_then(|s| s.context()) else {
        return cell(NoLogic);
    };
    let message = context.error_message.borrow().clone().unwrap_or_default().replace("\r\n", "\n");
    let font = ui.label(label).font.clone();
    let width = ui.node(label).bounds.w;
    let text = ui.assets.borrow_mut().wrap_text(&font, &message, width);
    ui.node_mut(label).bounds.h = ui.measure(&font, &text).1;
    ui.label(label).get_text = Some(Rc::new(move || text.clone()));
    ui.scroll_to_top(panel, false);
    ui.adjust_children(panel);
    cell(NoLogic)
}

/// `DebugLogic`: the cell and world position under the mouse while the terrain geometry overlay is on.
pub fn debug(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    if !ingame.world.world_actor().info.has_trait_info_named("TerrainGeometryOverlay") {
        return cell(NoLogic);
    }
    let label = ui.child(widget, "DEBUG_TEXT");
    let i = ingame.clone();
    ui.set_visible_fn(label, move || i.terrain_geometry.get());
    let mouse = Rc::new(std::cell::Cell::new((0, 0)));
    let m = mouse.clone();
    ui.label(label).get_text = Some(Rc::new(move || {
        let world_px = ingame.viewport.borrow().view_to_world_px(m.get());
        let cell = crate::widgets::world_interaction::view_to_world(world_px);
        let map = ingame.world.map();
        if !map.height.contains_cell(cell) {
            return String::new();
        }
        let height = *map.height.get(rustora_core::world_types::MPos::new(cell.x(), cell.y()));
        let pos = map.center_of_cell(cell);
        format!("({},{},{height}) ({},{},{})", cell.x(), cell.y(), pos.x, pos.y, pos.z)
    }));
    cell(DebugText { mouse })
}

struct DebugText {
    mouse: Rc<std::cell::Cell<(i32, i32)>>,
}

impl ChromeLogic for DebugText {
    fn tick(&mut self, ui: &mut Ui) {
        self.mouse.set(ui.last_mouse);
    }
}
