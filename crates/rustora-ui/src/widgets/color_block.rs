// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `ColorBlockWidget.cs`, `ClientTooltipRegionWidget.cs` and `BadgeWidget.cs`.

use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_net::session::Client;

use super::{TooltipContainerWidget, impl_as_any};
use crate::draw::DrawContext;
use crate::input::{MouseButton, MouseEvent, MouseInput};
use crate::widget::{Getter, Ui, WidgetArgs, WidgetId, WidgetKind};

/// Port of `ColorBlockWidget`.
#[derive(Clone, Default)]
pub struct ColorBlockWidget {
    pub color: Color,
    pub get_color: Option<Getter<Color>>,
    pub on_mouse_down: Option<Rc<dyn Fn(&mut Ui, &MouseInput)>>,
    pub on_mouse_up: Option<Rc<dyn Fn(&mut Ui, &MouseInput)>>,
}

impl WidgetKind for ColorBlockWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "Color" => self.color = Color::parse(value).unwrap_or(self.color),
            "ClickSound" => {}
            _ => return false,
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let color = self.get_color.as_ref().map_or(self.color, |f| f());
        dc.fill_rect_with_color(ui.render_bounds(id), color);
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        if mi.button != MouseButton::Left {
            return false;
        }
        if mi.event == MouseEvent::Down && !ui.take_mouse_focus(id, mi) {
            return false;
        }
        if ui.has_mouse_focus(id) && mi.event == MouseEvent::Up {
            if let Some(action) = self.on_mouse_up.clone() {
                let mi = *mi;
                ui.defer(move |ui| action(ui, &mi));
            }
            return self.yield_mouse_focus(ui, id, Some(mi));
        }
        if mi.event == MouseEvent::Down
            && let Some(action) = self.on_mouse_down.clone()
        {
            let mi = *mi;
            ui.defer(move |ui| action(ui, &mi));
        }
        false
    }
}

/// Port of `ClientTooltipRegionWidget`.
#[derive(Clone, Default)]
pub struct ClientTooltipRegionWidget {
    pub template: String,
    pub tooltip_container: Option<String>,
    pub client: Option<Client>,
}

impl WidgetKind for ClientTooltipRegionWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "Template" => self.template = value.to_string(),
            "TooltipContainer" => self.tooltip_container = Some(value.to_string()),
            _ => return false,
        }
        true
    }

    fn mouse_entered(&mut self, ui: &mut Ui, _id: WidgetId) {
        if let Some(container) = self.tooltip_container.as_deref() {
            let container = ui.child(ui.root, container);
            let args = WidgetArgs::default().with_value("client", self.client.clone());
            TooltipContainerWidget::set_tooltip(ui, container, &self.template, args);
        }
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        if let Some(container) = self.tooltip_container.as_deref().and_then(|c| ui.find(ui.root, c)) {
            TooltipContainerWidget::remove_tooltip(ui, container);
        }
    }
}

/// Port of `BadgeWidget`: the icon of a profile badge, drawn once it has been downloaded.
#[derive(Clone, Default)]
pub struct BadgeWidget {
    pub badge: Option<crate::player_database::PlayerBadge>,
}

impl WidgetKind for BadgeWidget {
    impl_as_any!();

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let Some(badge) = &self.badge else { return };
        let ui_scale = crate::logic::services(ui).settings.borrow().graphics.ui_scale;
        let Some((url, png)) = crate::player_database::icon(badge, ui_scale) else {
            return;
        };
        if let Some(sprite) = dc.minimap(&url, Some(&png)) {
            let size = crate::player_database::ICON_SIZE as f32;
            let origin = ui.render_bounds(id);
            dc.draw_sprite_sized(sprite, origin.x as f32, origin.y as f32, size, size);
        }
    }
}
