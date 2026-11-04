// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MainMenuLogic.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_game::map_cache::{MapClassification, MapVisibility};

use crate::loader::cell;
use crate::logic::NoLogic;
use crate::widget::{AppRequest, ChromeLogic, Ui, WidgetArgs, WidgetId};

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuType {
    Main,
    Singleplayer,
    Extras,
    MapEditor,
    None,
}

fn on_click(ui: &Ui, root: WidgetId, button: &str, action: impl Fn(&mut Ui) + 'static) {
    let id = ui.child(root, button);
    ui.button(id).on_click = Some(Rc::new(action));
}

/// The news and the version check are not ported, so these stay hidden as with `Game.FetchNews=False` in C#.
fn hide_unported_notices(ui: &mut Ui, widget: WidgetId) {
    for name in ["NEWS_BG", "UPDATE_NOTICE"] {
        if let Some(id) = ui.find(widget, name) {
            ui.set_visible_fn(id, || false);
        }
    }
}

pub fn create(ui: &mut Ui, widget: WidgetId, _args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let menu = Rc::new(Cell::new(MenuType::Main));
    let switch = |target: MenuType| {
        let menu = menu.clone();
        move |_: &mut Ui| menu.set(target)
    };
    let panel = |ui: &mut Ui, name: &str, kind: MenuType| {
        let id = ui.child(widget, name);
        let menu = menu.clone();
        ui.set_visible_fn(id, move || menu.get() == kind);
        id
    };

    let main_menu = panel(ui, "MAIN_MENU", MenuType::Main);
    on_click(ui, main_menu, "SINGLEPLAYER_BUTTON", switch(MenuType::Singleplayer));
    on_click(ui, main_menu, "EXTRAS_BUTTON", switch(MenuType::Extras));
    on_click(ui, main_menu, "QUIT_BUTTON", |ui| ui.requests.push(AppRequest::Quit));

    let singleplayer = panel(ui, "SINGLEPLAYER_MENU", MenuType::Singleplayer);
    on_click(ui, singleplayer, "BACK_BUTTON", switch(MenuType::Main));

    let extras = panel(ui, "EXTRAS_MENU", MenuType::Extras);
    on_click(ui, extras, "MAP_EDITOR_BUTTON", switch(MenuType::MapEditor));
    on_click(ui, extras, "BACK_BUTTON", switch(MenuType::Main));

    let editor = panel(ui, "MAP_EDITOR_MENU", MenuType::MapEditor);
    on_click(ui, editor, "BACK_BUTTON", switch(MenuType::Extras));
    let chooser_menu = menu.clone();
    on_click(ui, editor, "LOAD_MAP_BUTTON", move |ui| {
        chooser_menu.set(MenuType::None);
        let exit_menu = chooser_menu.clone();
        let args = WidgetArgs::default()
            .with_value("initialMap", None::<String>)
            .with_value("remoteMapPool", None::<Vec<String>>)
            .with_value("initialTab", MapClassification::User)
            .with_value(
                "filter",
                MapVisibility(MapVisibility::LOBBY.0 | MapVisibility::SHELLMAP.0 | MapVisibility::MISSION_SELECTOR.0),
            )
            .with_action("onExit", Rc::new(move |_| exit_menu.set(MenuType::MapEditor)));
        ui.open_window("MAPCHOOSER_PANEL", args);
    });

    hide_unported_notices(ui, widget);
    cell(NoLogic)
}
