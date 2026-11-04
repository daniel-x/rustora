// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MainMenuLogic.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::loader::cell;
use crate::logic::NoLogic;
use crate::widget::{AppRequest, ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::ButtonWidget;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuType {
    Main,
    Singleplayer,
    Extras,
    MapEditor,
}

fn on_click(ui: &Ui, root: WidgetId, button: &str, action: impl Fn(&mut Ui) + 'static) {
    let id = ui.child(root, button);
    ui.get::<ButtonWidget>(id).on_click = Some(Rc::new(action));
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

    cell(NoLogic)
}
