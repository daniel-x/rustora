// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ResourceBarWidget.cs`: the power and silo bars of the cnc sidebar.

use std::rc::Rc;

use rustora_core::fieldloader::Color;

use super::{TooltipContainerWidget, impl_as_any};
use crate::draw::{DrawContext, Rect};
use crate::widget::{Getter, Ui, WidgetArgs, WidgetId, WidgetKind};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResourceBarOrientation {
    Vertical,
    Horizontal,
}

/// `EWMA`: an exponentially weighted moving average that starts at the first value.
#[derive(Clone, Copy)]
struct Ewma {
    anim_rate: f32,
    value: Option<f32>,
}

impl Ewma {
    fn update(&mut self, new_value: f32) -> f32 {
        let old = self.value.unwrap_or(new_value);
        let v = old + (new_value - old) * self.anim_rate;
        self.value = Some(v);
        v
    }
}

#[derive(Clone)]
pub struct ResourceBarWidget {
    tooltip_template: String,
    tooltip_container: Option<String>,
    /// `TooltipTextCached`, given the used and the provided amount.
    pub tooltip_text: Option<Rc<dyn Fn(f32, f32) -> String>>,
    orientation: ResourceBarOrientation,
    indicator_collection: String,
    indicator_image: String,
    pub get_provided: Option<Getter<f32>>,
    pub get_used: Option<Getter<f32>>,
    pub get_bar_color: Option<Getter<Color>>,
    provided_lerp: Ewma,
    used_lerp: Ewma,
}

impl Default for ResourceBarWidget {
    fn default() -> Self {
        ResourceBarWidget {
            tooltip_template: String::new(),
            tooltip_container: None,
            tooltip_text: None,
            orientation: ResourceBarOrientation::Vertical,
            indicator_collection: "sidebar-bits".into(),
            indicator_image: "indicator".into(),
            get_provided: None,
            get_used: None,
            get_bar_color: None,
            provided_lerp: Ewma {
                anim_rate: 0.3,
                value: None,
            },
            used_lerp: Ewma {
                anim_rate: 0.3,
                value: None,
            },
        }
    }
}

impl WidgetKind for ResourceBarWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "TooltipTemplate" => self.tooltip_template = value.to_string(),
            "TooltipContainer" => self.tooltip_container = Some(value.to_string()),
            "Orientation" => {
                self.orientation = if value.trim() == "Horizontal" {
                    ResourceBarOrientation::Horizontal
                } else {
                    ResourceBarOrientation::Vertical
                }
            }
            "IndicatorCollection" => self.indicator_collection = value.to_string(),
            "IndicatorImage" => self.indicator_image = value.to_string(),
            _ => return false,
        }
        true
    }

    fn mouse_entered(&mut self, ui: &mut Ui, _id: WidgetId) {
        let Some(container) = self.tooltip_container.as_deref() else {
            return;
        };
        let (Some(text), Some(used), Some(provided)) = (&self.tooltip_text, &self.get_used, &self.get_provided) else {
            return;
        };
        let (text, used, provided) = (text.clone(), used.clone(), provided.clone());
        let get_text: Getter<String> = Rc::new(move || text(used(), provided()));
        let container = ui.child(ui.root, container);
        let args = WidgetArgs::default().with_value("getText", get_text);
        TooltipContainerWidget::set_tooltip(ui, container, &self.tooltip_template, args);
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        if let Some(container) = self.tooltip_container.as_deref().and_then(|c| ui.find(ui.root, c)) {
            TooltipContainerWidget::remove_tooltip(ui, container);
        }
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let provided = self.get_provided.as_ref().map_or(0.0, |f| f());
        let used = self.get_used.as_ref().map_or(0.0, |f| f());
        let mut scale_by = 100.0f32;
        let max = provided.max(used);
        while max >= scale_by {
            scale_by *= 2.0;
        }
        let provided_frac = self.provided_lerp.update(provided / scale_by);
        let used_frac = self.used_lerp.update(used / scale_by);
        let b = ui.render_bounds(id);
        let color = self.get_bar_color.as_ref().map_or(Color::from_argb(255, 255, 255, 255), |f| f());
        let indicator = dc.image(&self.indicator_collection, &self.indicator_image);
        let lerp = |a: i32, b: i32, t: f32| a as f32 + (b - a) as f32 * t;
        if self.orientation == ResourceBarOrientation::Vertical {
            let top = lerp(b.bottom(), b.y, provided_frac) as i32;
            let height = (provided_frac * b.h as f32) as i32;
            dc.fill_rect_with_color(Rect::new(b.x, top, b.w, height), color);
            if let Some(s) = indicator {
                let x = (b.x + b.right() - s.w) as f32 / 2.0;
                let y = lerp(b.bottom(), b.y, used_frac) - s.h as f32 / 2.0;
                dc.draw_sprite(s, x, y);
            }
        } else {
            let width = (provided_frac * b.w as f32) as i32;
            dc.fill_rect_with_color(Rect::new(b.x, b.y, width, b.h), color);
            if let Some(s) = indicator {
                let x = lerp(b.x, b.right(), used_frac) - s.w as f32 / 2.0;
                let y = (b.bottom() + b.y - s.h) as f32 / 2.0;
                dc.draw_sprite(s, x, y);
            }
        }
    }
}
