// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MainMenuLogic.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_game::map_cache::{MapClassification, MapVisibility};

use crate::loader::cell;
use crate::logic::{NoLogic, connection, cosmetic_random, lobby, services};
use crate::net::LOCAL_SERVER_TARGET;
use crate::widget::{Action, AppRequest, ChromeLogic, Ui, WidgetArgs, WidgetId};

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

/// Port of `StartSkirmishGame` and `OpenSkirmishLobbyPanel`.
fn start_skirmish_game(ui: &mut Ui, menu: &Rc<Cell<MenuType>>) {
    menu.set(MenuType::None);
    let service = services(ui);
    let map = {
        let settings = service.settings.borrow();
        service
            .maps
            .borrow()
            .choose_initial_map(settings.server.map.as_deref(), &mut cosmetic_random)
    };
    {
        let mut settings = service.settings.borrow_mut();
        settings.server.map = Some(map.clone());
        if let Err(e) = settings.save() {
            log::warn!("Cannot save the settings: {e}");
        }
    }
    ui.requests.push(AppRequest::CreateLocalServer { map, skirmish: true });

    let to_singleplayer = menu.clone();
    let on_exit: Action = Rc::new(move |ui| {
        ui.requests.push(AppRequest::Disconnect);
        to_singleplayer.set(MenuType::Singleplayer);
    });
    let on_connect: Action = Rc::new(move |ui| lobby::open_lobby(ui, on_exit.clone(), Rc::new(|_| {}), true));
    let to_main = menu.clone();
    let on_abort: Action = Rc::new(move |ui| {
        ui.requests.push(AppRequest::Disconnect);
        to_main.set(MenuType::Main);
    });
    connection::connect(ui, LOCAL_SERVER_TARGET, "", on_connect, on_abort);
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
    let skirmish_menu = menu.clone();
    on_click(ui, singleplayer, "SKIRMISH_BUTTON", move |ui| {
        start_skirmish_game(ui, &skirmish_menu)
    });
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
