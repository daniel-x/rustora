// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::rc::Rc;

use rustora_core::fieldloader::Color;

use super::label::parse_align;
use super::{TextAlign, impl_as_any, parse_bool};
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::input::{KeyEvent, KeyInput, MouseButton, MouseEvent, MouseInput};
use crate::widget::{Action, Getter, Ui, WidgetId, WidgetKind};

/// Port of `WidgetUtils.GetStatefulImageName`.
pub fn stateful_image_name(base: &str, disabled: bool, pressed: bool, hover: bool, focused: bool) -> String {
    let suffix = if disabled {
        "-disabled"
    } else if focused {
        "-focused"
    } else if pressed {
        "-pressed"
    } else if hover {
        "-hover"
    } else {
        ""
    };
    format!("{base}{suffix}")
}

/// Port of `ButtonWidget`.
pub struct ButtonWidget {
    pub key: Option<String>,
    pub text: String,
    resolved: Option<String>,
    pub align: TextAlign,
    pub left_margin: i32,
    pub right_margin: i32,
    pub background: String,
    pub depressed: bool,
    pub visual_height: i32,
    pub font: String,
    pub text_color: Color,
    pub text_color_disabled: Color,
    pub contrast: bool,
    pub shadow: bool,
    pub contrast_dark: Color,
    pub contrast_light: Color,
    pub contrast_radius: i32,
    pub highlighted: bool,
    pub disabled: bool,
    pub cursor: String,
    pub get_text: Option<Getter<String>>,
    pub get_color: Option<Getter<Color>>,
    pub is_disabled: Option<Getter<bool>>,
    pub is_highlighted: Option<Getter<bool>>,
    pub on_click: Option<Action>,
    pub on_double_click: Option<Action>,
    pub on_mouse_down: Option<Rc<dyn Fn(&mut Ui, &MouseInput)>>,
}

impl ButtonWidget {
    pub fn new(assets: &Assets) -> ButtonWidget {
        let m = &assets.metrics;
        ButtonWidget {
            key: None,
            text: String::new(),
            resolved: None,
            align: TextAlign::Center,
            left_margin: 5,
            right_margin: 5,
            background: "button".into(),
            depressed: false,
            visual_height: m.int("ButtonDepth"),
            font: m.string("ButtonFont"),
            text_color: m.color("ButtonTextColor"),
            text_color_disabled: m.color("ButtonTextColorDisabled"),
            contrast: m.bool("ButtonTextContrast"),
            shadow: m.bool("ButtonTextShadow"),
            contrast_dark: m.color("ButtonTextContrastColorDark"),
            contrast_light: m.color("ButtonTextContrastColorLight"),
            contrast_radius: m.int("ButtonTextContrastRadius"),
            highlighted: false,
            disabled: false,
            cursor: m.string("ButtonCursor"),
            get_text: None,
            get_color: None,
            is_disabled: None,
            is_highlighted: None,
            on_click: None,
            on_double_click: None,
            on_mouse_down: None,
        }
    }

    pub fn disabled_now(&self) -> bool {
        self.is_disabled.as_ref().map_or(self.disabled, |f| f())
    }

    pub fn set_field_base(&mut self, key: &str, value: &str) -> bool {
        match key {
            "Key" => self.key = Some(value.to_string()),
            "Text" => self.text = value.to_string(),
            "Align" => self.align = parse_align(value),
            "LeftMargin" => self.left_margin = value.parse().unwrap_or(self.left_margin),
            "RightMargin" => self.right_margin = value.parse().unwrap_or(self.right_margin),
            "Background" => self.background = value.to_string(),
            "VisualHeight" => self.visual_height = value.parse().unwrap_or(self.visual_height),
            "Font" => self.font = value.to_string(),
            "TextColor" => self.text_color = Color::parse(value).unwrap_or(self.text_color),
            "TextColorDisabled" => self.text_color_disabled = Color::parse(value).unwrap_or(self.text_color_disabled),
            "Contrast" => self.contrast = parse_bool(value),
            "Shadow" => self.shadow = parse_bool(value),
            "ContrastColorDark" => self.contrast_dark = Color::parse(value).unwrap_or(self.contrast_dark),
            "ContrastColorLight" => self.contrast_light = Color::parse(value).unwrap_or(self.contrast_light),
            "ContrastRadius" => self.contrast_radius = value.parse().unwrap_or(self.contrast_radius),
            "Disabled" => self.disabled = parse_bool(value),
            "Highlighted" => self.highlighted = parse_bool(value),
            "Cursor" => self.cursor = value.to_string(),
            "TooltipContainer" | "TooltipTemplate" | "TooltipText" | "TooltipDesc" | "DisableKeyRepeat" | "DisableKeySound"
            | "ClickSound" | "ClickDisabledSound" => {}
            _ => return false,
        }
        true
    }

    /// Port of `ButtonWidget.Draw` with the background drawn by the caller's choice of `rect`.
    pub fn draw_button(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext, usable_width: i32) {
        let rb = ui.render_bounds(id);
        let disabled = self.disabled_now();
        let highlighted = self.is_highlighted.as_ref().map_or(self.highlighted, |f| f());
        let hover = ui.mouse_over == Some(id) || ui.node(id).children.iter().any(|&c| ui.mouse_over == Some(c));
        let offset = if self.depressed { self.visual_height } else { 0 };
        draw_background(dc, &self.background, rb, disabled, self.depressed, hover, highlighted);

        let text = match &self.get_text {
            Some(f) => f(),
            None => {
                if self.resolved.is_none() {
                    self.resolved = Some(if self.text.is_empty() {
                        String::new()
                    } else {
                        dc.assets.messages.get(&self.text)
                    });
                }
                self.resolved.clone().unwrap_or_default()
            }
        };
        let (tw, th) = dc.measure(&self.font, &text);
        let y = rb.y + (rb.h - th - dc.font_top_offset(&self.font)) / 2;
        let x = match self.align {
            TextAlign::Left => rb.x + self.left_margin,
            TextAlign::Center => rb.x + (usable_width - tw) / 2,
            TextAlign::Right => rb.x + usable_width - tw - self.right_margin,
        };
        let color = self.get_color.as_ref().map_or(self.text_color, |f| f());
        let shown = if disabled { self.text_color_disabled } else { color };
        if self.contrast {
            let style = TextStyle::Contrast {
                dark: self.contrast_dark,
                light: self.contrast_light,
                radius: self.contrast_radius,
            };
            dc.draw_text(&self.font, &text, x + offset, y + offset, shown, style);
        } else if self.shadow {
            let style = TextStyle::Shadow {
                dark: self.contrast_dark,
                light: self.contrast_light,
                offset: 1,
            };
            dc.draw_text(&self.font, &text, x, y, color, style);
        } else {
            dc.draw_text(&self.font, &text, x + offset, y + offset, shown, TextStyle::Plain);
        }
    }

    /// Port of `ButtonWidget.HandleMouseInput`.
    pub fn handle_button_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        if mi.button != MouseButton::Left {
            return false;
        }
        if mi.event == MouseEvent::Down && !ui.take_mouse_focus(id, mi) {
            return false;
        }
        let disabled = self.disabled_now();
        if ui.has_mouse_focus(id) && mi.event == MouseEvent::Up && mi.multi_tap == 2 && self.on_double_click.is_some() {
            if !disabled {
                let action = self.on_double_click.clone().expect("checked");
                action(ui);
                return self.yield_mouse_focus(ui, id, Some(mi));
            }
        } else if ui.has_mouse_focus(id) && mi.event == MouseEvent::Up {
            let click = self.depressed && !disabled;
            let result = self.yield_mouse_focus(ui, id, Some(mi));
            if click && let Some(action) = self.on_click.clone() {
                action(ui);
            }
            return result;
        }
        if mi.event == MouseEvent::Down {
            if !disabled {
                if let Some(action) = self.on_mouse_down.clone() {
                    action(ui, mi);
                }
                self.depressed = true;
            } else {
                self.yield_mouse_focus(ui, id, Some(mi));
            }
        } else if mi.event == MouseEvent::Move && ui.has_mouse_focus(id) {
            self.depressed = ui.render_bounds(id).contains(mi.x, mi.y);
        }
        self.depressed
    }
}

/// Port of the static `ButtonWidget.DrawBackground`.
pub fn draw_background(dc: &mut DrawContext, base: &str, rect: Rect, disabled: bool, pressed: bool, hover: bool, highlighted: bool) {
    if base.is_empty() {
        return;
    }
    let variant = if highlighted {
        format!("{base}-highlighted")
    } else {
        base.to_string()
    };
    dc.draw_panel(&stateful_image_name(&variant, disabled, pressed, hover, false), rect);
}

impl WidgetKind for ButtonWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        self.set_field_base(key, value)
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let width = ui.node(id).bounds.w;
        self.draw_button(ui, id, dc, width);
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        let handled = self.handle_button_mouse(ui, id, mi);
        if ui.exists(id) {
            let offset = if self.depressed { self.visual_height } else { 0 };
            ui.node_mut(id).child_offset = (offset, offset);
        }
        handled
    }

    fn handle_key(&mut self, ui: &mut Ui, _id: WidgetId, e: &KeyInput) -> bool {
        if e.event != KeyEvent::Down || self.key.as_deref() != Some(e.key.as_str()) {
            return false;
        }
        if !self.disabled_now()
            && let Some(action) = self.on_click.clone()
        {
            action(ui);
        }
        true
    }

    fn yield_mouse_focus(&mut self, ui: &mut Ui, id: WidgetId, _mi: Option<&MouseInput>) -> bool {
        self.depressed = false;
        ui.node_mut(id).child_offset = (0, 0);
        if ui.mouse_focus == Some(id) {
            ui.mouse_focus = None;
        }
        true
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        Some(self.cursor.clone())
    }
}
