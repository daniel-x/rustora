// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use super::{impl_as_any, parse_bool};
use crate::draw::DrawContext;
use crate::input::MouseInput;
use crate::widget::{Getter, Ui, WidgetId, WidgetKind};

/// Port of `ImageWidget`.
pub struct ImageWidget {
    pub image_collection: String,
    pub image_name: String,
    pub click_through: bool,
    pub get_image_name: Option<Getter<String>>,
    pub get_image_collection: Option<Getter<String>>,
}

impl Default for ImageWidget {
    fn default() -> Self {
        ImageWidget {
            image_collection: String::new(),
            image_name: String::new(),
            click_through: true,
            get_image_name: None,
            get_image_collection: None,
        }
    }
}

impl WidgetKind for ImageWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "ImageCollection" => self.image_collection = value.to_string(),
            "ImageName" => self.image_name = value.to_string(),
            "ClickThrough" => self.click_through = parse_bool(value),
            "TooltipTemplate" | "TooltipContainer" | "TooltipText" => {}
            _ => return false,
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let collection = self
            .get_image_collection
            .as_ref()
            .map_or_else(|| self.image_collection.clone(), |f| f());
        let name = self.get_image_name.as_ref().map_or_else(|| self.image_name.clone(), |f| f());
        if let Some(sprite) = dc.image(&collection, &name) {
            let (x, y) = ui.render_origin(id);
            dc.draw_sprite(sprite, x as f32, y as f32);
        }
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        !self.click_through && ui.render_bounds(id).contains(mi.x, mi.y)
    }
}
