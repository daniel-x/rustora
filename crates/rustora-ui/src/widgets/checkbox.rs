// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use rustora_core::fieldloader::Color;

use super::button::{draw_background, stateful_image};
use super::{ButtonWidget, impl_as_any};
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::input::{KeyInput, MouseInput};
use crate::widget::{Getter, Ui, WidgetId, WidgetKind};

/// Port of `CheckboxWidget`.
#[derive(Clone)]
pub struct CheckboxWidget {
    pub button: ButtonWidget,
    pub background: String,
    pub checkmark: String,
    pub get_checkmark: Option<Getter<String>>,
    pub is_checked: Option<Getter<bool>>,
    pub text_color: Color,
    pub text_color_disabled: Color,
}

impl CheckboxWidget {
    pub fn new(assets: &Assets) -> CheckboxWidget {
        CheckboxWidget {
            button: ButtonWidget::new(assets),
            background: "checkbox".into(),
            checkmark: "tick".into(),
            get_checkmark: None,
            is_checked: None,
            text_color: assets.metrics.color("TextColor"),
            text_color_disabled: assets.metrics.color("TextDisabledColor"),
        }
    }
}

impl WidgetKind for CheckboxWidget {
    impl_as_any!();

    fn as_button(&mut self) -> Option<&mut ButtonWidget> {
        Some(&mut self.button)
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "Background" => self.background = value.to_string(),
            "Checkmark" => self.checkmark = value.to_string(),
            "TextColor" => self.text_color = Color::parse(value).unwrap_or(self.text_color),
            "TextColorDisabled" => self.text_color_disabled = Color::parse(value).unwrap_or(self.text_color_disabled),
            _ => return self.button.set_field_base(key, value),
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let b = &self.button;
        let disabled = b.disabled_now();
        let hover = ui.mouse_over == Some(id);
        let highlighted = b.is_highlighted.as_ref().map_or(b.highlighted, |f| f());
        let rb = ui.render_bounds(id);
        let bounds = ui.node(id).bounds;
        let rect = Rect::new(rb.x, rb.y, bounds.h, bounds.h);
        draw_background(dc, &self.background, rect, disabled, b.depressed, hover, highlighted);

        let text = self.button.resolved_text(dc);
        let b = &self.button;
        let color = b.get_color.as_ref().map_or(self.text_color, |f| f());
        let shown = if disabled { self.text_color_disabled } else { color };
        let (_, text_height) = dc.measure(&b.font, &text);
        let x = (rb.x as f32 + rb.h as f32 * 1.5) as i32;
        let y = rb.y + (bounds.h - text_height - dc.font_top_offset(&b.font)) / 2;
        let style = if b.contrast {
            TextStyle::Contrast {
                dark: b.contrast_dark,
                light: b.contrast_light,
                radius: 2,
            }
        } else {
            TextStyle::Plain
        };
        dc.draw_text(&b.font, &text, x, y, shown, style);

        let check_type = self.get_checkmark.as_ref().map_or_else(|| self.checkmark.clone(), |f| f());
        let checked = self.is_checked.as_ref().is_some_and(|f| f());
        let image_name = if checked { "checked" } else { "unchecked" };
        let state = (disabled, b.depressed, hover, false, highlighted);
        if let Some(mark) = stateful_image(dc, &format!("checkmark-{check_type}"), image_name, state) {
            let mx = rect.right() - (rect.h + mark.w) / 2;
            let my = rect.y + (rect.h - mark.h) / 2;
            dc.draw_sprite(mark, mx as f32, my as f32);
        }
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        self.button.handle_mouse_with_offset(ui, id, mi)
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
