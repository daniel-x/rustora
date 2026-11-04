// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `ColorMixerWidget.cs` and `HueSliderWidget.cs`.

use rustora_core::fieldloader::Color;
use rustora_game::color_picker::{from_hsv, to_hsv};

use super::impl_as_any;
use super::slider::SliderWidget;
use crate::chrome::Sprite;
use crate::draw::{DrawContext, Rect};
use crate::input::{MouseButton, MouseEvent, MouseInput};
use crate::widget::{Action, Ui, WidgetId, WidgetKind};

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

/// Port of `HueSliderWidget`.
#[derive(Clone, Default)]
pub struct HueSliderWidget {
    pub slider: SliderWidget,
}

impl HueSliderWidget {
    fn sprite(dc: &mut DrawContext) -> Sprite {
        let texture = match dc.assets.hue_texture {
            Some(texture) => texture,
            None => {
                let rgba: Vec<u8> = (0..256)
                    .flat_map(|h| {
                        let c = from_hsv(h as f32 / 255.0, 1.0, 1.0);
                        [c.r(), c.g(), c.b(), 255]
                    })
                    .collect();
                let texture = dc.renderer.add_ui_texture(256, 1, &rgba, false);
                dc.assets.hue_texture = Some(texture);
                texture
            }
        };
        Sprite {
            texture,
            x: 0,
            y: 0,
            w: 256,
            h: 1,
        }
    }
}

impl WidgetKind for HueSliderWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        self.slider.set_field(key, value)
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        self.slider.handle_mouse(ui, id, mi)
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let rb = ui.render_bounds(id);
        let hue = Self::sprite(dc);
        dc.draw_sprite_sized(hue, rb.x as f32, rb.y as f32, rb.w as f32, rb.h as f32);
        if let Some(picker) = dc.image("lobby-bits", "huepicker") {
            let x = rb.x + self.slider.px_from_value(self.slider.value, rb).clamp(0, rb.w - 1) - picker.w / 2;
            let y = rb.y + (rb.h - picker.h) / 2;
            dc.draw_sprite(picker, x as f32, y as f32);
        }
    }
}

/// Port of `ColorMixerWidget`.
#[derive(Clone, Default)]
pub struct ColorMixerWidget {
    pub on_change: Option<Action>,
    pub h: f32,
    pub s: f32,
    pub v: f32,
    shown_hue: f32,
    min_sat: f32,
    max_sat: f32,
    min_val: f32,
    max_val: f32,
    is_moving: bool,
}

impl ColorMixerWidget {
    /// Port of `SetColorLimits`. The texture follows when the widget next draws.
    pub fn set_color_limits(&mut self, min_sat: f32, max_sat: f32, min_val: f32, max_val: f32, new_hue: Option<f32>) {
        self.min_sat = min_sat;
        self.max_sat = max_sat;
        self.min_val = min_val;
        self.max_val = max_val;
        self.shown_hue = new_hue.unwrap_or(self.h);
    }

    pub fn color(&self) -> Color {
        from_hsv(self.h, self.s, self.v)
    }

    /// Port of `Set` without the change event. Returns true if the color changed.
    pub fn set(&mut self, color: Color) -> bool {
        let (h, s, v) = to_hsv(color);
        if self.h == h && self.s == s && self.v == v {
            return false;
        }
        self.h = h;
        self.s = s.clamp(self.min_sat, self.max_sat);
        self.v = v.clamp(self.min_val, self.max_val);
        true
    }

    fn set_value_from_px(&mut self, x: i32, y: i32, rb: Rect) {
        let v = lerp(self.min_val, self.max_val, x as f32 / rb.w as f32);
        let s = lerp(self.min_sat, self.max_sat, 1.0 - y as f32 / rb.h as f32);
        self.v = v.clamp(self.min_val, self.max_val);
        self.s = s.clamp(self.min_sat, self.max_sat);
    }

    fn px_from_value(&self, rb: Rect) -> (i32, i32) {
        let x = rb.w as f32 * (self.v - self.min_val) / (self.max_val - self.min_val);
        let y = rb.h as f32 * (1.0 - (self.s - self.min_sat) / (self.max_sat - self.min_sat));
        (x.clamp(0.0, rb.w as f32) as i32, y.clamp(0.0, rb.h as f32) as i32)
    }

    fn pixels(hue: f32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(4 * 256 * 256);
        for s in 0..256 {
            for v in 0..256 {
                let c = from_hsv(hue, 1.0 - s as f32 / 255.0, v as f32 / 255.0);
                rgba.extend_from_slice(&[c.r(), c.g(), c.b(), 255]);
            }
        }
        rgba
    }

    fn sprite(&self, dc: &mut DrawContext) -> Sprite {
        let texture = match dc.assets.mixer_texture {
            Some((texture, hue)) if hue == self.shown_hue => texture,
            Some((texture, _)) => {
                dc.renderer
                    .update_ui_texture(texture, 0, 0, 256, 256, &Self::pixels(self.shown_hue));
                dc.assets.mixer_texture = Some((texture, self.shown_hue));
                texture
            }
            None => {
                let texture = dc.renderer.add_ui_texture(256, 256, &Self::pixels(self.shown_hue), false);
                dc.assets.mixer_texture = Some((texture, self.shown_hue));
                texture
            }
        };
        Sprite {
            texture,
            x: (255.0 * self.min_val) as i32,
            y: (255.0 * (1.0 - self.max_sat)) as i32,
            w: (255.0 * (self.max_val - self.min_val)) as i32,
            h: (255.0 * (self.max_sat - self.min_sat)) as i32 + 1,
        }
    }

    fn fire_change(&self, ui: &mut Ui) {
        if let Some(on_change) = self.on_change.clone() {
            ui.defer(move |ui| on_change(ui));
        }
    }
}

impl WidgetKind for ColorMixerWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, _value: &str) -> bool {
        key == "ClickSound"
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let rb = ui.render_bounds(id);
        let mixer = self.sprite(dc);
        dc.draw_sprite_sized(mixer, rb.x as f32, rb.y as f32, rb.w as f32, rb.h as f32);
        if let Some(picker) = dc.image("lobby-bits", "colorpicker") {
            let (px, py) = self.px_from_value(rb);
            let (x, y) = (rb.x + px - picker.w / 2, rb.y + py - picker.h / 2);
            dc.fill_ellipse(Rect::new(x + 1, y + 1, picker.w - 2, picker.h - 2), self.color());
            dc.draw_sprite(picker, x as f32, y as f32);
        }
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        if mi.button != MouseButton::Left {
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
                self.set_value_from_px(mi.x - rb.x, mi.y - rb.y, rb);
                self.fire_change(ui);
            }
            MouseEvent::Move if self.is_moving => {
                self.set_value_from_px(mi.x - rb.x, mi.y - rb.y, rb);
                self.fire_change(ui);
            }
            _ => {}
        }
        true
    }
}
