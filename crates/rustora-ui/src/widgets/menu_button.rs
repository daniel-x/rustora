// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MenuButtonWidget.cs`: a button that opens an in-game menu container.

use super::{ButtonWidget, impl_as_any, parse_bool};
use crate::assets::Assets;
use crate::draw::DrawContext;
use crate::input::{KeyInput, MouseInput};
use crate::widget::{Ui, WidgetId, WidgetKind};

#[derive(Clone)]
pub struct MenuButtonWidget {
    pub button: ButtonWidget,
    pub menu_container: String,
    pub pause: bool,
    pub hide_ingame_ui: bool,
    pub disable_world_sounds: bool,
}

impl MenuButtonWidget {
    pub fn new(assets: &Assets) -> MenuButtonWidget {
        MenuButtonWidget {
            button: ButtonWidget::new(assets),
            menu_container: "INGAME_MENU".into(),
            pause: true,
            hide_ingame_ui: true,
            disable_world_sounds: false,
        }
    }
}

impl WidgetKind for MenuButtonWidget {
    impl_as_any!();

    fn as_button(&mut self) -> Option<&mut ButtonWidget> {
        Some(&mut self.button)
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "MenuContainer" => self.menu_container = value.to_string(),
            "Pause" => self.pause = parse_bool(value),
            "HideIngameUI" => self.hide_ingame_ui = parse_bool(value),
            "DisableWorldSounds" => self.disable_world_sounds = parse_bool(value),
            _ => return self.button.set_field_base(key, value),
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        self.button.draw(ui, id, dc);
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        self.button.handle_mouse(ui, id, mi)
    }

    fn handle_key(&mut self, ui: &mut Ui, id: WidgetId, e: &KeyInput) -> bool {
        self.button.handle_key(ui, id, e)
    }

    fn yield_mouse_focus(&mut self, ui: &mut Ui, id: WidgetId, mi: Option<&MouseInput>) -> bool {
        self.button.yield_mouse_focus(ui, id, mi)
    }

    fn mouse_entered(&mut self, ui: &mut Ui, id: WidgetId) {
        self.button.show_tooltip(ui, id);
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.button.hide_tooltip(ui);
    }

    fn cursor(&self, default: &str) -> Option<String> {
        self.button.cursor(default)
    }
}
