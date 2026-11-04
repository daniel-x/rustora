// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::rc::Rc;

use super::button::draw_background;
use super::{impl_as_any, parse_bool};
use crate::draw::{DrawContext, Rect};
use crate::input::{MouseButton, MouseEvent, MouseInput};
use crate::widget::{Getter, Ui, WidgetId, WidgetKind};

/// Port of `SliderWidget`.
#[derive(Clone)]
pub struct SliderWidget {
    pub on_change: Option<Rc<dyn Fn(&mut Ui, f32)>>,
    pub ticks: i32,
    pub track_height: i32,
    pub thumb: String,
    pub track: String,
    pub minimum_value: f32,
    pub maximum_value: f32,
    pub value: f32,
    pub get_value: Option<Getter<f32>>,
    pub disabled: bool,
    pub is_disabled: Option<Getter<bool>>,
    is_moving: bool,
}

impl Default for SliderWidget {
    fn default() -> Self {
        SliderWidget {
            on_change: None,
            ticks: 0,
            track_height: 5,
            thumb: "slider-thumb".into(),
            track: "slider-track".into(),
            minimum_value: 0.0,
            maximum_value: 1.0,
            value: 0.0,
            get_value: None,
            disabled: false,
            is_disabled: None,
            is_moving: false,
        }
    }
}

impl SliderWidget {
    /// Sets the value without calling `on_change`. Returns true if it changed.
    pub fn set_value(&mut self, new_value: f32) -> bool {
        let old = self.value;
        self.value = new_value.clamp(self.minimum_value, self.maximum_value);
        old != self.value
    }

    /// Port of `UpdateValue`.
    pub fn update_value(&mut self, ui: &mut Ui, new_value: f32) {
        if self.set_value(new_value)
            && let Some(on_change) = self.on_change.clone()
        {
            let value = self.value;
            ui.defer(move |ui| on_change(ui, value));
        }
    }

    /// Port of `ValueFromPx`.
    pub fn value_from_px(&self, x: i32, rb: Rect) -> f32 {
        self.minimum_value + (self.maximum_value - self.minimum_value) * (x as f32 - 0.5 * rb.h as f32) / (rb.w - rb.h) as f32
    }

    /// Port of `PxFromValue`.
    pub fn px_from_value(&self, value: f32, rb: Rect) -> i32 {
        (0.5 * rb.h as f32 + (rb.w - rb.h) as f32 * (value - self.minimum_value) / (self.maximum_value - self.minimum_value)) as i32
    }

    fn thumb_rect(&self, rb: Rect) -> Rect {
        let origin = (rb.x as f32 + self.px_from_value(self.value, rb) as f32 - rb.h as f32 / 2.0) as i32;
        Rect::new(origin, rb.y, rb.h, rb.h)
    }

    fn disabled_now(&self) -> bool {
        self.is_disabled.as_ref().map_or(self.disabled, |f| f())
    }
}

impl WidgetKind for SliderWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        let float = |current: f32| value.parse().unwrap_or(current);
        match key {
            "Ticks" => self.ticks = value.parse().unwrap_or(self.ticks),
            "TrackHeight" => self.track_height = value.parse().unwrap_or(self.track_height),
            "Thumb" => self.thumb = value.to_string(),
            "Track" => self.track = value.to_string(),
            "MinimumValue" => self.minimum_value = float(self.minimum_value),
            "MaximumValue" => self.maximum_value = float(self.maximum_value),
            "Value" => self.value = float(self.value),
            "Disabled" => self.disabled = parse_bool(value),
            _ => return false,
        }
        true
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        if mi.button != MouseButton::Left || self.disabled_now() {
            return false;
        }
        if mi.event == MouseEvent::Down && !ui.take_mouse_focus(id, mi) {
            return false;
        }
        if !ui.has_mouse_focus(id) {
            return false;
        }
        let rb = ui.render_bounds(id);
        match mi.event {
            MouseEvent::Up => {
                self.is_moving = false;
                self.yield_mouse_focus(ui, id, Some(mi));
            }
            MouseEvent::Down => {
                self.is_moving = true;
                self.update_value(ui, self.value_from_px(mi.x - rb.x, rb));
            }
            MouseEvent::Move if self.is_moving => self.update_value(ui, self.value_from_px(mi.x - rb.x, rb)),
            _ => {}
        }
        self.thumb_rect(rb).contains(mi.x, mi.y)
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let next = self.get_value.as_ref().map_or(self.value, |f| f());
        self.update_value(ui, next);
        let rb = ui.render_bounds(id);
        let thumb_rect = self.thumb_rect(rb);
        let track_width = rb.w - rb.h;
        let track_origin = rb.x + rb.h / 2;
        let track_rect = Rect::new(
            track_origin - 1,
            rb.y + (rb.h - self.track_height) / 2,
            track_width + 2,
            self.track_height,
        );
        if let Some(tick) = dc.image("slider", "tick") {
            for i in 0..self.ticks {
                let x = (track_origin + i * (track_rect.w - tick.w) / (self.ticks - 1)) as f32 - tick.w as f32 / 2.0;
                dc.draw_sprite(tick, x, track_rect.bottom() as f32);
            }
        }
        dc.draw_panel(&self.track, track_rect);
        let (mx, my) = ui.last_mouse;
        let hover = ui.mouse_over == Some(id) && thumb_rect.contains(mx, my);
        draw_background(dc, &self.thumb, thumb_rect, self.disabled_now(), self.is_moving, hover, false);
    }
}
