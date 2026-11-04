// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use super::{impl_as_any, parse_bool};
use crate::input::MouseInput;
use crate::widget::{Ui, WidgetId, WidgetKind};

/// Port of `ContainerWidget`.
#[derive(Clone)]
pub struct ContainerWidget {
    pub click_through: bool,
}

impl Default for ContainerWidget {
    fn default() -> Self {
        ContainerWidget { click_through: true }
    }
}

impl WidgetKind for ContainerWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "ClickThrough" => self.click_through = parse_bool(value),
            _ => return false,
        }
        true
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        !self.click_through && ui.render_bounds(id).contains(mi.x, mi.y)
    }

    fn ignores_mouse_over(&self) -> bool {
        true
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        None
    }
}
