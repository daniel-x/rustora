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

/// `MainMenuLogic.MenuPanel`: the panel a game was started from, reopened when the menu returns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuPanel {
    None,
    Missions,
    Skirmish,
    Multiplayer,
    Replays,
    GameSaves,
}

thread_local! {
    static LAST_GAME_STATE: Cell<MenuPanel> = const { Cell::new(MenuPanel::None) };
}

fn remember_panel(panel: MenuPanel) -> Action {
    Rc::new(move |_| LAST_GAME_STATE.with(|s| s.set(panel)))
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
    let on_connect: Action = Rc::new(move |ui| lobby::open_lobby(ui, on_exit.clone(), remember_panel(MenuPanel::Skirmish), true));
    let to_main = menu.clone();
    let on_abort: Action = Rc::new(move |ui| {
        ui.requests.push(AppRequest::Disconnect);
        to_main.set(MenuType::Main);
    });
    connection::connect(ui, LOCAL_SERVER_TARGET, "", on_connect, on_abort);
}

/// Port of `OpenMissionBrowserPanel`.
fn open_mission_browser_panel(ui: &mut Ui, menu: &Rc<Cell<MenuType>>, map: Option<String>) {
    menu.set(MenuType::None);
    let to_singleplayer = menu.clone();
    let args = WidgetArgs::default()
        .with_action(
            "onExit",
            Rc::new(move |ui| {
                ui.requests.push(AppRequest::Disconnect);
                to_singleplayer.set(MenuType::Singleplayer);
            }),
        )
        .with_action("onStart", remember_panel(MenuPanel::Missions))
        .with_value("initialMap", map);
    ui.open_window("MISSIONBROWSER_PANEL", args);
}

/// Port of `OpenGameSaveBrowserPanel`.
fn open_game_save_browser_panel(ui: &mut Ui, menu: &Rc<Cell<MenuType>>) {
    menu.set(MenuType::None);
    let to_singleplayer = menu.clone();
    let args = WidgetArgs::default()
        .with_action("onExit", Rc::new(move |_| to_singleplayer.set(MenuType::Singleplayer)))
        .with_action("onStart", remember_panel(MenuPanel::GameSaves));
    ui.open_window("GAMESAVE_BROWSER_PANEL", args);
}

/// Port of `OpenMultiplayerPanel`.
fn open_multiplayer_panel(ui: &mut Ui, menu: &Rc<Cell<MenuType>>) {
    menu.set(MenuType::None);
    let to_main = menu.clone();
    crate::logic::multiplayer::open_multiplayer_panel(
        ui,
        remember_panel(MenuPanel::Multiplayer),
        Rc::new(move |_| to_main.set(MenuType::Main)),
    );
}

/// Port of `OpenReplayBrowserPanel`.
fn open_replay_browser_panel(ui: &mut Ui, menu: &Rc<Cell<MenuType>>) {
    menu.set(MenuType::None);
    let to_extras = menu.clone();
    let args = WidgetArgs::default()
        .with_action("onExit", Rc::new(move |_| to_extras.set(MenuType::Extras)))
        .with_action("onStart", remember_panel(MenuPanel::Replays));
    ui.open_window("REPLAYBROWSER_PANEL", args);
}

/// Port of `OpenMenuBasedOnLastGame`.
fn open_menu_based_on_last_game(ui: &mut Ui, menu: &Rc<Cell<MenuType>>) {
    match LAST_GAME_STATE.with(|s| s.replace(MenuPanel::None)) {
        MenuPanel::Replays => open_replay_browser_panel(ui, menu),
        MenuPanel::GameSaves => open_game_save_browser_panel(ui, menu),
        MenuPanel::Skirmish => start_skirmish_game(ui, menu),
        MenuPanel::Missions => {
            let map = super::mission_browser::pick_last_modified_map(ui, MapVisibility::MISSION_SELECTOR);
            open_mission_browser_panel(ui, menu, map);
        }
        MenuPanel::Multiplayer => open_multiplayer_panel(ui, menu),
        MenuPanel::None => {}
    }
}

/// Port of `OnRemoteDirectConnect`, without the server browser behind the lobby until that is ported.
fn direct_connect(ui: &mut Ui, target: &str, menu: &Rc<Cell<MenuType>>) {
    menu.set(MenuType::None);
    let to_main = menu.clone();
    let on_exit: Action = Rc::new(move |ui| {
        ui.requests.push(AppRequest::Disconnect);
        to_main.set(MenuType::Main);
    });
    let on_abort = on_exit.clone();
    let on_connect: Action = Rc::new(move |ui| lobby::open_lobby(ui, on_exit.clone(), Rc::new(|_| {}), false));
    connection::connect(ui, target, "", on_connect, on_abort);
}

pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
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
    on_click(ui, main_menu, "EXTRAS_BUTTON", switch(MenuType::Extras));
    let settings_menu = menu.clone();
    on_click(ui, main_menu, "SETTINGS_BUTTON", move |ui| {
        settings_menu.set(MenuType::None);
        let to_main = settings_menu.clone();
        let args = WidgetArgs::default().with_action("onExit", Rc::new(move |_| to_main.set(MenuType::Main)));
        ui.open_window("SETTINGS_PANEL", args);
    });
    let multiplayer_menu = menu.clone();
    on_click(ui, main_menu, "MULTIPLAYER_BUTTON", move |ui| {
        open_multiplayer_panel(ui, &multiplayer_menu)
    });
    if let Some(content_button) = ui.find(main_menu, "CONTENT_BUTTON") {
        let installer_mod = services(ui).content_installer_mod.clone();
        ui.button(content_button).disabled = installer_mod.is_none();
        ui.button(content_button).on_click = Some(Rc::new(move |ui| {
            if let Some(installer) = &installer_mod {
                ui.requests.push(AppRequest::InitializeMod(installer.clone()));
            }
        }));
    }
    on_click(ui, main_menu, "QUIT_BUTTON", |ui| ui.requests.push(AppRequest::Quit));

    let singleplayer = panel(ui, "SINGLEPLAYER_MENU", MenuType::Singleplayer);
    let missions_menu = menu.clone();
    let missions_button = ui.child(singleplayer, "MISSIONS_BUTTON");
    {
        let disabled = !super::mission_browser::has_missions(ui);
        let mut b = ui.button(missions_button);
        b.disabled = disabled;
        b.on_click = Some(Rc::new(move |ui| {
            let map = super::mission_browser::pick_last_modified_map(ui, MapVisibility::MISSION_SELECTOR);
            open_mission_browser_panel(ui, &missions_menu, map);
        }));
    }
    let skirmish_menu = menu.clone();
    on_click(ui, singleplayer, "SKIRMISH_BUTTON", move |ui| {
        start_skirmish_game(ui, &skirmish_menu)
    });
    let load_button = ui.child(singleplayer, "LOAD_BUTTON");
    let load_enabled = Rc::new(Cell::new(super::game_save_browser::is_load_panel_enabled(ui)));
    {
        let (e, m) = (load_enabled.clone(), menu.clone());
        let mut b = ui.button(load_button);
        b.is_disabled = Some(Rc::new(move || !e.get()));
        b.on_click = Some(Rc::new(move |ui| open_game_save_browser_panel(ui, &m)));
    }
    let refresh_menu = menu.clone();
    on_click(ui, main_menu, "SINGLEPLAYER_BUTTON", move |ui| {
        load_enabled.set(super::game_save_browser::is_load_panel_enabled(ui));
        refresh_menu.set(MenuType::Singleplayer);
    });
    on_click(ui, singleplayer, "BACK_BUTTON", switch(MenuType::Main));

    let extras = panel(ui, "EXTRAS_MENU", MenuType::Extras);
    on_click(ui, extras, "MAP_EDITOR_BUTTON", switch(MenuType::MapEditor));
    let replays_menu = menu.clone();
    on_click(ui, extras, "REPLAYS_BUTTON", move |ui| open_replay_browser_panel(ui, &replays_menu));
    let music_menu = menu.clone();
    on_click(ui, extras, "MUSIC_BUTTON", move |ui| {
        music_menu.set(MenuType::None);
        let to_extras = music_menu.clone();
        let args = WidgetArgs::default().with_action("onExit", Rc::new(move |_| to_extras.set(MenuType::Extras)));
        ui.open_window("MUSIC_PANEL", args);
    });
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
    if let Some(target) = args.value::<String>("directConnect") {
        direct_connect(ui, &target, &menu);
    } else {
        open_menu_based_on_last_game(ui, &menu);
    }
    cell(NoLogic)
}
