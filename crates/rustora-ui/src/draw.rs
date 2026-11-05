// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Drawing context of a frame and a port of the drawing parts of `WidgetUtils.cs`.

use std::cell::RefMut;

use rustora_core::fieldloader::Color;
use rustora_render::Renderer;
use rustora_render::ui::{UiList, UiQuad};

use crate::assets::Assets;
use crate::chrome::Sprite;
use crate::font::premultiplied;

/// A screen rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// `Rectangle.IntersectsWith`.
    pub fn intersects(&self, other: Rect) -> bool {
        self.x < other.right() && self.right() > other.x && self.y < other.bottom() && self.bottom() > other.y
    }

    /// `Rectangle.InflateBy`.
    pub fn inflate(&self, left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(self.x - left, self.y - top, self.w + left + right, self.h + top + bottom)
    }

    pub fn intersect(&self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Rect::new(
            x,
            y,
            (self.right().min(other.right()) - x).max(0),
            (self.bottom().min(other.bottom()) - y).max(0),
        )
    }
}

/// How a font draws around its glyphs.
#[derive(Clone, Copy)]
pub enum TextStyle {
    Plain,
    Contrast { dark: Color, light: Color, radius: i32 },
    Shadow { dark: Color, light: Color, offset: i32 },
}

/// Everything a widget needs to draw itself.
pub struct DrawContext<'a> {
    pub renderer: &'a mut Renderer,
    pub list: &'a mut UiList,
    pub assets: RefMut<'a, Assets>,
    /// `Renderer.EnableAntialiasingFilter`: sprites sample with the pixel art scaling filter.
    pub antialiasing: bool,
    clips: Vec<Rect>,
}

const WHITE: [f32; 4] = [1.0; 4];

impl<'a> DrawContext<'a> {
    pub fn new(renderer: &'a mut Renderer, list: &'a mut UiList, assets: RefMut<'a, Assets>) -> DrawContext<'a> {
        DrawContext {
            renderer,
            list,
            assets,
            antialiasing: false,
            clips: Vec::new(),
        }
    }

    /// Port of `Renderer.EnableScissor`: nested clips intersect.
    pub fn push_clip(&mut self, rect: Rect) {
        let clip = self.clips.last().map_or(rect, |c| c.intersect(rect));
        self.clips.push(clip);
        self.list.set_clip(Some([clip.x, clip.y, clip.w, clip.h]));
    }

    pub fn pop_clip(&mut self) {
        self.clips.pop();
        self.list.set_clip(self.clips.last().map(|c| [c.x, c.y, c.w, c.h]));
    }

    pub fn image(&mut self, collection: &str, image: &str) -> Option<Sprite> {
        let assets = &mut *self.assets;
        assets.chrome.image(collection, image, &*assets.fs, self.renderer)
    }

    pub fn draw_sprite(&mut self, s: Sprite, x: f32, y: f32) {
        self.draw_sprite_sized(s, x, y, s.w as f32, s.h as f32);
    }

    pub fn draw_sprite_sized(&mut self, s: Sprite, x: f32, y: f32, w: f32, h: f32) {
        self.list.quads.push(UiQuad {
            texture: s.texture,
            dst: [x, y, w, h],
            src: [s.x as f32, s.y as f32, s.w as f32, s.h as f32],
            color: WHITE,
            gradient: None,
            corners: None,
            pixel_art: self.antialiasing,
        });
    }

    /// Port of `FillRectWithSprite`: tiles the sprite and cuts the last row and column.
    pub fn fill_rect_with_sprite(&mut self, r: Rect, s: Sprite) {
        if s.w <= 0 || s.h <= 0 {
            return;
        }
        let mut x = r.x;
        while x < r.right() {
            let mut y = r.y;
            while y < r.bottom() {
                let cut = Sprite {
                    w: s.w.min(r.right() - x),
                    h: s.h.min(r.bottom() - y),
                    ..s
                };
                self.draw_sprite(cut, x as f32, y as f32);
                y += s.h;
            }
            x += s.w;
        }
    }

    pub fn fill_rect_with_color(&mut self, r: Rect, c: Color) {
        self.list.solid([r.x as f32, r.y as f32, r.w as f32, r.h as f32], premultiplied(c));
    }

    /// Port of `RgbaColorRenderer.DrawRect` for an axis aligned rectangle: lines of `width` centered on the edges.
    pub fn draw_rect(&mut self, a: (i32, i32), b: (i32, i32), width: f32, c: Color) {
        let (l, t) = (a.0.min(b.0) as f32, a.1.min(b.1) as f32);
        let (r, bt) = (a.0.max(b.0) as f32, a.1.max(b.1) as f32);
        let h = width / 2.0;
        let color = premultiplied(c);
        self.list.solid([l - h, t - h, r - l + width, width], color);
        self.list.solid([l - h, bt - h, r - l + width, width], color);
        self.list.solid([l - h, t + h, width, bt - t - width], color);
        self.list.solid([r - h, t + h, width, bt - t - width], color);
    }

    /// Port of `RgbaColorRenderer.FillEllipse`.
    pub fn fill_ellipse(&mut self, r: Rect, c: Color) {
        let (a, b) = (r.w as f32 / 2.0, r.h as f32 / 2.0);
        let (xc, yc) = (r.x as f32 + a, r.y as f32 + b);
        let color = premultiplied(c);
        for y in r.y..=r.bottom() {
            let dy = y as f32 - yc;
            let dx = a * (1.0 - dy * dy / b / b).max(0.0).sqrt();
            self.list.solid([xc - dx, y as f32 - 0.5, 2.0 * dx, 1.0], color);
        }
    }

    /// Returns the image of `MapPreview.GetMinimap`, loaded on first use.
    pub fn minimap(&mut self, uid: &str, png: Option<&[u8]>) -> Option<Sprite> {
        if let Some(sprite) = self.assets.minimaps.get(uid) {
            return *sprite;
        }
        let sprite = png
            .and_then(|data| crate::chrome::decode_png(data).ok())
            .map(|(w, h, rgba)| Sprite {
                texture: self.renderer.add_ui_texture(w, h, &rgba, false),
                x: 0,
                y: 0,
                w: w as i32,
                h: h as i32,
            });
        self.assets.minimaps.insert(uid.to_string(), sprite);
        sprite
    }

    /// Port of `DrawPanel`.
    /// `ChromeProvider.GetMinimumPanelSize`.
    pub fn minimum_panel_size(&mut self, collection: &str) -> (i32, i32) {
        let assets = &mut *self.assets;
        let Some(sprites) = assets.chrome.panel(collection, &*assets.fs, self.renderer) else {
            return (0, 0);
        };
        if sprites.len() != 9 {
            return (0, 0);
        }
        (
            sprites[3].map_or(0, |s| s.w) + sprites[5].map_or(0, |s| s.w),
            sprites[1].map_or(0, |s| s.h) + sprites[7].map_or(0, |s| s.h),
        )
    }

    pub fn draw_panel(&mut self, collection: &str, bounds: Rect) {
        let assets = &mut *self.assets;
        let Some(sprites) = assets.chrome.panel(collection, &*assets.fs, self.renderer) else {
            return;
        };
        if sprites.len() != 9 {
            return;
        }
        let margin_top = sprites[1].map_or(0, |s| s.h);
        let margin_left = sprites[3].map_or(0, |s| s.w);
        let margin_right = sprites[5].map_or(0, |s| s.w);
        let margin_bottom = sprites[7].map_or(0, |s| s.h);
        let margin_width = margin_right + margin_left;
        let margin_height = margin_bottom + margin_top;
        let (left, top, right, bottom) = (bounds.x, bounds.y, bounds.right(), bounds.bottom());

        if let Some(s) = sprites[4] {
            self.fill_rect_with_sprite(
                Rect::new(
                    left + margin_left,
                    top + margin_top,
                    bounds.w - margin_width,
                    bounds.h - margin_height,
                ),
                s,
            );
        }
        if let Some(s) = sprites[3] {
            self.fill_rect_with_sprite(Rect::new(left, top + margin_top, margin_left, bounds.h - margin_height), s);
        }
        if let Some(s) = sprites[5] {
            self.fill_rect_with_sprite(
                Rect::new(right - margin_right, top + margin_top, margin_left, bounds.h - margin_height),
                s,
            );
        }
        if let Some(s) = sprites[1] {
            self.fill_rect_with_sprite(Rect::new(left + margin_left, top, bounds.w - margin_width, margin_top), s);
        }
        if let Some(s) = sprites[7] {
            self.fill_rect_with_sprite(
                Rect::new(left + margin_left, bottom - margin_bottom, bounds.w - margin_width, margin_top),
                s,
            );
        }
        if let Some(s) = sprites[0] {
            self.draw_sprite(s, left as f32, top as f32);
        }
        if let Some(s) = sprites[2] {
            self.draw_sprite(s, (right - s.w) as f32, top as f32);
        }
        if let Some(s) = sprites[6] {
            self.draw_sprite(s, left as f32, (bottom - s.h) as f32);
        }
        if let Some(s) = sprites[8] {
            self.draw_sprite(s, (right - s.w) as f32, (bottom - s.h) as f32);
        }
    }

    /// Port of `SpriteFont.Measure`.
    pub fn measure(&mut self, font: &str, text: &str) -> (i32, i32) {
        self.assets.measure(font, text)
    }

    /// Port of `TryGetPanelImages` returning a value.
    pub fn has_panel(&mut self, collection: &str) -> bool {
        let assets = &mut *self.assets;
        assets.chrome.panel(collection, &*assets.fs, self.renderer).is_some()
    }

    pub fn font_top_offset(&self, font: &str) -> i32 {
        self.assets.fonts.get(font).map_or(0, |f| f.top_offset)
    }

    /// Port of `WidgetUtils.WrapText`.
    pub fn wrap_text(&mut self, font: &str, text: &str, width: i32) -> String {
        self.assets.wrap_text(font, text, width)
    }

    /// Port of `DrawTextWithShadow` with an angle of a quarter turn: the text reads from the bottom to the top and
    /// turns around `(x, y)`.
    pub fn draw_text_rotated(&mut self, font: &str, text: &str, x: i32, y: i32, color: Color, style: TextStyle) {
        if let TextStyle::Shadow { dark, light, offset } = style {
            let bg = contrast_color(color, dark, light);
            self.draw_text_turned(font, text, x + offset, y + offset, bg);
        }
        self.draw_text_turned(font, text, x, y, color);
    }

    fn draw_text_turned(&mut self, font: &str, text: &str, x: i32, y: i32, color: Color) {
        let start = self.list.quads.len();
        self.draw_text(font, text, x, y, color, TextStyle::Plain);
        let (cx, cy) = (x as f32, y as f32);
        let turn = |p: [f32; 2]| [cx + (p[1] - cy), cy - (p[0] - cx)];
        for q in &mut self.list.quads[start..] {
            let [qx, qy, w, h] = q.dst;
            let corners = q.corners.unwrap_or([[qx, qy], [qx + w, qy], [qx + w, qy + h], [qx, qy + h]]);
            q.corners = Some(corners.map(turn));
        }
    }

    /// Port of `DrawText`, `DrawTextWithContrast` and `DrawTextWithShadow`.
    pub fn draw_text(&mut self, font: &str, text: &str, x: i32, y: i32, color: Color, style: TextStyle) {
        let assets = &mut *self.assets;
        let Some(f) = assets.fonts.get_mut(font) else {
            return;
        };
        let (x, y) = (x as f32, y as f32);
        let glyphs = &mut assets.glyphs;
        match style {
            TextStyle::Plain => {}
            TextStyle::Contrast { dark, light, radius } => {
                if radius > 0 {
                    let bg = contrast_color(color, dark, light);
                    f.draw_contrast(text, x, y, bg, radius, glyphs, self.renderer, self.list);
                }
            }
            TextStyle::Shadow { dark, light, offset } => {
                let bg = contrast_color(color, dark, light);
                f.draw_text(text, x + offset as f32, y + offset as f32, bg, glyphs, self.renderer, self.list);
            }
        }
        f.draw_text(text, x, y, color, glyphs, self.renderer, self.list);
    }
}

/// Port of `SpriteFont.GetContrastColor`.
pub fn contrast_color(fg: Color, dark: Color, light: Color) -> Color {
    let (r, g, b) = (fg.r() as f32 / 255.0, fg.g() as f32 / 255.0, fg.b() as f32 / 255.0);
    let brightness = (r.max(g).max(b) + r.min(g).min(b)) / 2.0;
    if (fg.r(), fg.g(), fg.b(), fg.a()) == (255, 255, 255, 255) || brightness > 0.33 {
        dark
    } else {
        light
    }
}
