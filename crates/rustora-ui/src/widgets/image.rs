// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use super::TooltipContainerWidget;
use super::{impl_as_any, parse_bool};
use crate::draw::DrawContext;
use crate::input::MouseInput;
use crate::widget::{Getter, Ui, WidgetArgs, WidgetId, WidgetKind};

/// Port of `ImageWidget`.
#[derive(Clone)]
pub struct ImageWidget {
    pub image_collection: String,
    pub image_name: String,
    pub click_through: bool,
    pub get_image_name: Option<Getter<String>>,
    pub get_image_collection: Option<Getter<String>>,
    /// `WidgetUtils.BindButtonIcon`: the button whose state selects the image variant.
    pub state_button: Option<WidgetId>,
    pub tooltip_template: String,
    pub tooltip_container: Option<String>,
    pub get_tooltip_text: Option<Getter<String>>,
}

impl Default for ImageWidget {
    fn default() -> Self {
        ImageWidget {
            image_collection: String::new(),
            image_name: String::new(),
            click_through: true,
            get_image_name: None,
            get_image_collection: None,
            state_button: None,
            tooltip_template: String::new(),
            tooltip_container: None,
            get_tooltip_text: None,
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
            "TooltipTemplate" => self.tooltip_template = value.to_string(),
            "TooltipContainer" => self.tooltip_container = Some(value.to_string()),
            "TooltipText" => {
                let text = value.to_string();
                self.get_tooltip_text = Some(std::rc::Rc::new(move || text.clone()));
            }
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
        let sprite = match self.state_button {
            Some(button) => {
                let (disabled, pressed, highlighted) = {
                    let b = ui.button(button);
                    (
                        b.disabled_now(),
                        b.depressed,
                        b.is_highlighted.as_ref().map_or(b.highlighted, |f| f()),
                    )
                };
                let suffix = match (disabled, pressed, ui.mouse_over == Some(button)) {
                    (true, _, _) => "-disabled",
                    (_, true, _) => "-pressed",
                    (_, _, true) => "-hover",
                    _ => "",
                };
                let collection = if highlighted {
                    format!("{collection}-highlighted")
                } else {
                    collection
                };
                dc.image(&collection, &format!("{name}{suffix}"))
                    .or_else(|| dc.image(&collection, &name))
            }
            None => dc.image(&collection, &name),
        };
        if let Some(sprite) = sprite {
            let (x, y) = ui.render_origin(id);
            dc.draw_sprite(sprite, x as f32, y as f32);
        }
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        !self.click_through && ui.render_bounds(id).contains(mi.x, mi.y)
    }

    fn mouse_entered(&mut self, ui: &mut Ui, _id: WidgetId) {
        let (Some(container), Some(text)) = (self.tooltip_container.as_deref(), &self.get_tooltip_text) else {
            return;
        };
        let container = ui.child(ui.root, container);
        let args = WidgetArgs::default().with_value("getText", text.clone());
        TooltipContainerWidget::set_tooltip(ui, container, &self.tooltip_template, args);
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        if let Some(container) = self.tooltip_container.as_deref().and_then(|c| ui.find(ui.root, c)) {
            TooltipContainerWidget::remove_tooltip(ui, container);
        }
    }
}
