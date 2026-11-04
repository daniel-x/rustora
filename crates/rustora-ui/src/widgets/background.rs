// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use super::{impl_as_any, parse_bool};
use crate::draw::DrawContext;
use crate::input::MouseInput;
use crate::widget::{Ui, WidgetId, WidgetKind};

/// Port of `BackgroundWidget`.
#[derive(Clone)]
pub struct BackgroundWidget {
    pub click_through: bool,
    pub background: String,
}

impl Default for BackgroundWidget {
    fn default() -> Self {
        BackgroundWidget {
            click_through: false,
            background: "dialog".into(),
        }
    }
}

impl WidgetKind for BackgroundWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "ClickThrough" => self.click_through = parse_bool(value),
            "Background" => self.background = value.to_string(),
            _ => return false,
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        dc.draw_panel(&self.background, ui.render_bounds(id));
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        !self.click_through && ui.render_bounds(id).contains(mi.x, mi.y)
    }
}
