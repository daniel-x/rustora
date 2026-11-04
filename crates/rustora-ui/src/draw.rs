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
    clips: Vec<Rect>,
}

const WHITE: [f32; 4] = [1.0; 4];

impl<'a> DrawContext<'a> {
    pub fn new(renderer: &'a mut Renderer, list: &'a mut UiList, assets: RefMut<'a, Assets>) -> DrawContext<'a> {
        DrawContext {
            renderer,
            list,
            assets,
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

    /// Port of `DrawPanel`.
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
        let assets = &mut *self.assets;
        match assets.fonts.get_mut(font) {
            Some(f) => f.measure(text, &mut assets.glyphs, self.renderer),
            None => (0, 0),
        }
    }

    pub fn font_top_offset(&self, font: &str) -> i32 {
        self.assets.fonts.get(font).map_or(0, |f| f.top_offset)
    }

    /// Port of `WidgetUtils.WrapText`.
    pub fn wrap_text(&mut self, font: &str, text: &str, width: i32) -> String {
        if self.measure(font, text).0 <= width {
            return text.to_string();
        }
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].clone();
            if self.measure(font, &line).0 > width {
                let mut start = 0;
                loop {
                    let Some(space) = line[start..].find(' ').map(|p| p + start) else {
                        break;
                    };
                    if self.measure(font, &line[..space]).0 > width {
                        break;
                    }
                    start = space + 1;
                }
                if start > 0 {
                    lines[i] = line[..start - 1].to_string();
                    lines.insert(i + 1, line[start..].to_string());
                }
            }
            i += 1;
        }
        lines.join("\n")
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
                let bg = contrast_color(color, dark, light);
                let r = radius as f32;
                for (dx, dy) in [(-r, 0.0), (r, 0.0), (0.0, -r), (0.0, r)] {
                    f.draw_text(text, x + dx, y + dy, bg, glyphs, self.renderer, self.list);
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
