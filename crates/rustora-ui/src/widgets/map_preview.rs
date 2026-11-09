// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MapPreviewWidget.cs`.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_client::map_cache::{MapGridType, MapPreview};
use rustora_core::fieldloader::Color;
use rustora_core::world_types::CPos;

use super::{TooltipContainerWidget, impl_as_any, parse_bool};
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::input::{MouseEvent, MouseInput};
use crate::widget::{Getter, Ui, WidgetArgs, WidgetId, WidgetKind};

/// Port of `SpawnOccupant`.
#[derive(Clone, Debug)]
pub struct SpawnOccupant {
    pub color: Color,
    pub player_name: String,
    pub team: i32,
    pub faction: Option<String>,
    pub spawn_point: i32,
}

/// Port of `MapPreviewWidget`.
#[derive(Clone)]
pub struct MapPreviewWidget {
    pub ignore_mouse_input: bool,
    pub show_spawn_points: bool,
    pub tooltip_container: Option<String>,
    pub tooltip_template: String,
    pub preview: Option<Getter<Option<Rc<MapPreview>>>>,
    pub spawn_occupants: Option<Getter<HashMap<i32, SpawnOccupant>>>,
    pub disabled_spawn_points: Option<Getter<Vec<i32>>>,
    pub on_mouse_down: Option<Rc<dyn Fn(&mut Ui, WidgetId, &MouseInput)>>,
    pub tooltip_spawn_index: i32,
    pub show_unoccupied_spawnpoints: bool,
    pub map_rect: Rect,
    pub preview_scale: f32,
    spawn_font: String,
    spawn_color: Color,
    spawn_contrast_color: Color,
    spawn_label_offset: (i32, i32),
}

impl MapPreviewWidget {
    pub fn new(assets: &Assets) -> MapPreviewWidget {
        let m = &assets.metrics;
        MapPreviewWidget {
            ignore_mouse_input: false,
            show_spawn_points: true,
            tooltip_container: None,
            tooltip_template: "SPAWN_TOOLTIP".into(),
            preview: None,
            spawn_occupants: None,
            disabled_spawn_points: None,
            on_mouse_down: None,
            tooltip_spawn_index: -1,
            show_unoccupied_spawnpoints: true,
            map_rect: Rect::default(),
            preview_scale: 0.0,
            spawn_font: m.string("SpawnFont"),
            spawn_color: m.color("SpawnColor"),
            spawn_contrast_color: m.color("SpawnContrastColor"),
            spawn_label_offset: m.int2("SpawnLabelOffset"),
        }
    }

    /// Port of `ConvertToPreview`.
    pub fn convert_to_preview(&self, preview: &MapPreview, cell: CPos) -> (i32, i32) {
        let (cell_width, u, v) = match preview.grid_type {
            MapGridType::Rectangular => (1, cell.x(), cell.y()),
            MapGridType::RectangularIsometric => {
                let v = cell.x() + cell.y();
                (2, (v - (v & 1)) / 2 - cell.y(), v)
            }
        };
        let mut dx = (self.preview_scale * (cell_width * (u - preview.bounds.left())) as f32) as i32;
        let dy = (self.preview_scale * (v - preview.bounds.top()) as f32) as i32;
        if v & 1 == 1 {
            dx += 1;
        }
        (self.map_rect.x + dx, self.map_rect.y + dy)
    }
}

impl WidgetKind for MapPreviewWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "IgnoreMouseInput" => self.ignore_mouse_input = parse_bool(value),
            "ShowSpawnPoints" => self.show_spawn_points = parse_bool(value),
            "TooltipContainer" => self.tooltip_container = Some(value.to_string()),
            "TooltipTemplate" => self.tooltip_template = value.to_string(),
            _ => return false,
        }
        true
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        if self.ignore_mouse_input || mi.event != MouseEvent::Down {
            return false;
        }
        if let Some(on_mouse_down) = self.on_mouse_down.clone() {
            let mi = *mi;
            ui.defer(move |ui| on_mouse_down(ui, id, &mi));
        }
        true
    }

    fn mouse_entered(&mut self, ui: &mut Ui, id: WidgetId) {
        if let Some(container) = self.tooltip_container.as_deref() {
            let container = ui.child(ui.root, container);
            let args = WidgetArgs::default()
                .with_value("preview", id)
                .with_value("showUnoccupiedSpawnpoints", self.show_unoccupied_spawnpoints);
            TooltipContainerWidget::set_tooltip(ui, container, &self.tooltip_template, args);
        }
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        if let Some(container) = self.tooltip_container.as_deref().and_then(|c| ui.find(ui.root, c)) {
            TooltipContainerWidget::remove_tooltip(ui, container);
        }
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let Some(preview) = self.preview.as_ref().and_then(|f| f()) else {
            return;
        };
        let Some(minimap) = dc.minimap(&preview.uid, preview.minimap_png.as_deref()) else {
            return;
        };
        let bounds = ui.render_bounds(id);
        self.preview_scale = (bounds.w as f32 / minimap.w as f32).min(bounds.h as f32 / minimap.h as f32);
        let w = (self.preview_scale * minimap.w as f32) as i32;
        let h = (self.preview_scale * minimap.h as f32) as i32;
        self.map_rect = Rect::new(bounds.x + (bounds.w - w) / 2, bounds.y + (bounds.h - h) / 2, w, h);
        dc.draw_sprite_sized(minimap, self.map_rect.x as f32, self.map_rect.y as f32, w as f32, h as f32);

        self.tooltip_spawn_index = -1;
        if !self.show_spawn_points {
            return;
        }
        let occupants = self.spawn_occupants.as_ref().map_or_else(HashMap::new, |f| f());
        let disabled = self.disabled_spawn_points.as_ref().map_or_else(Vec::new, |f| f());
        let unclaimed = dc.image("lobby-bits", "spawn-unclaimed");
        let claimed = dc.image("lobby-bits", "spawn-claimed");
        let disabled_image = dc.image("lobby-bits", "spawn-disabled").or(unclaimed);
        for (i, &spawn) in preview.spawn_points.iter().enumerate() {
            let number = i as i32 + 1;
            let occupant = occupants.get(&number);
            let is_disabled = disabled.contains(&number);
            let pos = self.convert_to_preview(&preview, spawn);
            let sprite = if is_disabled {
                disabled_image
            } else if occupant.is_some() {
                claimed
            } else {
                unclaimed
            };
            let Some(sprite) = sprite else {
                continue;
            };
            let offset = (sprite.w / 2, sprite.h / 2);
            let (mx, my) = ui.last_mouse;
            let along_x = (pos.0 - mx) as f32 / offset.0 as f32;
            let along_y = (pos.1 - my) as f32 / offset.1 as f32;
            if along_x * along_x + along_y * along_y <= 1.0 {
                self.tooltip_spawn_index = number;
            }
            if is_disabled {
                dc.draw_sprite(sprite, (pos.0 - offset.0) as f32, (pos.1 - offset.1) as f32);
                continue;
            }
            if let Some(occupant) = occupant {
                let ellipse = Rect::new(pos.0 - offset.0 + 1, pos.1 - offset.1 + 1, sprite.w - 2, sprite.h - 2);
                dc.fill_ellipse(ellipse, occupant.color);
            }
            dc.draw_sprite(sprite, (pos.0 - offset.0) as f32, (pos.1 - offset.1) as f32);

            let letter = char::from(b'A' + i as u8).to_string();
            let size = dc.measure(&self.spawn_font, &letter);
            let text = (
                pos.0 - (size.0 / 2 + self.spawn_label_offset.0),
                pos.1 - (size.1 / 2 + self.spawn_label_offset.1),
            );
            let style = TextStyle::Contrast {
                dark: self.spawn_contrast_color,
                light: self.spawn_contrast_color,
                radius: 1,
            };
            dc.draw_text(&self.spawn_font, &letter, text.0, text.1, self.spawn_color, style);
        }
    }
}
