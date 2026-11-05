// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::rc::Rc;

use rustora_core::fieldloader::Color;

use super::button::stateful_image_name;
use super::{impl_as_any, parse_bool};
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::hotkeys::Hotkey;
use crate::input::{KeyEvent, KeyInput, MouseEvent, MouseInput};
use crate::widget::{Getter, Ui, WidgetId, WidgetKind};

/// The callback of `OnLoseFocus`, given the entered key. It runs while the widget is borrowed.
pub type HotkeyCallback = Rc<dyn Fn(&mut Ui, Option<Hotkey>)>;

/// Port of `HotkeyEntryWidget`. An invalid key is `None`.
#[derive(Clone)]
pub struct HotkeyEntryWidget {
    pub key: Option<Hotkey>,
    pub visual_height: i32,
    pub left_margin: i32,
    pub right_margin: i32,
    /// `OnEscKey`, which returns the key to show.
    pub on_esc_key: Option<Rc<dyn Fn(&mut Ui) -> Option<Hotkey>>>,
    pub on_lose_focus: Option<HotkeyCallback>,
    pub is_valid: Option<Getter<bool>>,
    pub disabled: bool,
    pub is_disabled: Option<Getter<bool>>,
    pub font: String,
    pub text_color: Color,
    pub text_color_disabled: Color,
    pub text_color_invalid: Color,
    blink_cycle: i32,
    show_entry: bool,
}

const IGNORE_KEYS: [&str; 8] = ["RSHIFT", "LSHIFT", "RCTRL", "LCTRL", "RALT", "LALT", "RGUI", "LGUI"];

impl HotkeyEntryWidget {
    pub fn new(assets: &Assets) -> HotkeyEntryWidget {
        let m = &assets.metrics;
        HotkeyEntryWidget {
            key: None,
            visual_height: 1,
            left_margin: 5,
            right_margin: 5,
            on_esc_key: None,
            on_lose_focus: None,
            is_valid: None,
            disabled: false,
            is_disabled: None,
            font: m.string("HotkeyFont"),
            text_color: m.color("HotkeyColor"),
            text_color_disabled: m.color("HotkeyColorDisabled"),
            text_color_invalid: m.color("HotkeyColorInvalid"),
            blink_cycle: 15,
            show_entry: true,
        }
    }

    fn disabled_now(&self) -> bool {
        self.is_disabled.as_ref().map_or(self.disabled, |f| f())
    }

    fn valid_now(&self) -> bool {
        self.is_valid.as_ref().is_some_and(|f| f())
    }

    fn lose_focus(&self, ui: &mut Ui) {
        if let Some(f) = self.on_lose_focus.clone() {
            f(ui, self.key.clone());
        }
    }

    /// Port of `ForceYieldKeyboardFocus`.
    pub fn force_yield_keyboard_focus(ui: &mut Ui, id: WidgetId) {
        let (callback, key) = {
            let w = ui.get::<HotkeyEntryWidget>(id);
            (w.on_lose_focus.clone(), w.key.clone())
        };
        if let Some(f) = callback {
            f(ui, key);
        }
        if ui.keyboard_focus == Some(id) {
            ui.keyboard_focus = None;
        }
    }
}

impl WidgetKind for HotkeyEntryWidget {
    impl_as_any!();

    fn input_disabled(&mut self) -> Option<bool> {
        Some(self.disabled_now())
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        let color = |current: Color| Color::parse(value).unwrap_or(current);
        match key {
            "VisualHeight" => self.visual_height = value.parse().unwrap_or(self.visual_height),
            "LeftMargin" => self.left_margin = value.parse().unwrap_or(self.left_margin),
            "RightMargin" => self.right_margin = value.parse().unwrap_or(self.right_margin),
            "Font" => self.font = value.to_string(),
            "TextColor" => self.text_color = color(self.text_color),
            "TextColorDisabled" => self.text_color_disabled = color(self.text_color_disabled),
            "TextColorInvalid" => self.text_color_invalid = color(self.text_color_invalid),
            "Disabled" => self.disabled = parse_bool(value),
            _ => return false,
        }
        true
    }

    fn yield_keyboard_focus(&mut self, ui: &mut Ui, id: WidgetId) -> bool {
        self.lose_focus(ui);
        if !self.valid_now() {
            return false;
        }
        if ui.keyboard_focus == Some(id) {
            ui.keyboard_focus = None;
        }
        true
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        if self.disabled_now() || mi.event != MouseEvent::Down {
            return false;
        }
        if !ui.render_bounds(id).contains(mi.x, mi.y) || !ui.take_keyboard_focus(id) {
            return false;
        }
        self.blink_cycle = 15;
        true
    }

    fn handle_key(&mut self, ui: &mut Ui, id: WidgetId, e: &KeyInput) -> bool {
        if self.disabled_now() || e.event == KeyEvent::Up {
            return false;
        }
        if !ui.has_keyboard_focus(id) || IGNORE_KEYS.contains(&e.key.as_str()) {
            return false;
        }
        if e.key == "ESCAPE" {
            if let Some(f) = self.on_esc_key.clone() {
                self.key = f(ui);
            }
        } else {
            self.key = Some(Hotkey::from_key_input(e));
        }
        self.yield_keyboard_focus(ui, id);
        true
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        if ui.has_keyboard_focus(id) {
            self.blink_cycle -= 1;
            if self.blink_cycle <= 0 {
                self.blink_cycle = 15;
                self.show_entry = !self.show_entry;
            }
        }
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let unknown = Hotkey {
            key: "UNKNOWN".into(),
            modifiers: Default::default(),
        };
        let text = self.key.as_ref().unwrap_or(&unknown).display_string(&dc.assets.messages);
        let (x, y) = ui.render_origin(id);
        let bounds = ui.node(id).bounds;
        let (text_w, text_h) = dc.measure(&self.font, &text);
        let disabled = self.disabled_now();
        let focus = ui.has_keyboard_focus(id);
        let state = stateful_image_name("textfield", disabled, false, ui.mouse_over == Some(id), focus);
        dc.draw_panel(&state, Rect::new(x, y, bounds.w, bounds.h));
        if focus && !self.show_entry {
            return;
        }
        let text_x = x + self.left_margin;
        let text_y = y + (bounds.h - text_h) / 2 - self.visual_height;
        let overflowing = text_w > bounds.w - self.left_margin - self.right_margin;
        if overflowing {
            dc.push_clip(Rect::new(
                x + self.left_margin,
                y,
                bounds.w - self.left_margin - self.right_margin,
                bounds.bottom(),
            ));
        }
        let color = if disabled {
            self.text_color_disabled
        } else if !self.valid_now() {
            self.text_color_invalid
        } else {
            self.text_color
        };
        dc.draw_text(&self.font, &text, text_x, text_y, color, TextStyle::Plain);
        if overflowing {
            dc.pop_clip();
        }
    }
}
