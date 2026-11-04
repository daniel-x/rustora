// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `SpriteFont.cs`, with swash in place of FreeType. Glyphs are packed on a shared texture.

use std::collections::HashMap;

use rustora_core::fieldloader::Color;
use rustora_render::Renderer;
use rustora_render::ui::{UiList, UiQuad, UiTexture};
use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::Format;

const SHEET_SIZE: u32 = 2048;

#[derive(Clone, Copy, Default)]
struct Glyph {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    left: i32,
    top: i32,
    advance: i32,
}

/// One font of `Fonts:` in `mod.yaml`.
pub struct SpriteFont {
    data: Vec<u8>,
    size: i32,
    pub top_offset: i32,
    glyphs: HashMap<char, Glyph>,
}

/// Packs glyphs on one texture, like the `SheetBuilder` of `Renderer.Fonts`.
pub struct GlyphSheet {
    texture: Option<UiTexture>,
    x: u32,
    y: u32,
    row: u32,
    scale: ScaleContext,
}

impl Default for GlyphSheet {
    fn default() -> Self {
        GlyphSheet {
            texture: None,
            x: 0,
            y: 0,
            row: 0,
            scale: ScaleContext::new(),
        }
    }
}

impl GlyphSheet {
    fn allocate(&mut self, w: u32, h: u32, renderer: &mut Renderer) -> (UiTexture, u32, u32) {
        let texture = *self.texture.get_or_insert_with(|| {
            renderer.add_ui_texture(SHEET_SIZE, SHEET_SIZE, &vec![0; (SHEET_SIZE * SHEET_SIZE * 4) as usize], false)
        });
        if self.x + w + 1 > SHEET_SIZE {
            self.x = 0;
            self.y += self.row + 1;
            self.row = 0;
        }
        let at = (texture, self.x, self.y);
        self.x += w + 1;
        self.row = self.row.max(h);
        at
    }
}

impl SpriteFont {
    pub fn new(data: Vec<u8>, size: i32, ascender: i32) -> SpriteFont {
        SpriteFont {
            data,
            size,
            top_offset: size - ascender,
            glyphs: HashMap::new(),
        }
    }

    fn create_glyph(&self, c: char, sheet: &mut GlyphSheet, renderer: &mut Renderer) -> Glyph {
        let Some(font) = FontRef::from_index(&self.data, 0) else {
            return Glyph::default();
        };
        let id = font.charmap().map(c);
        let advance = font.glyph_metrics(&[]).scale(self.size as f32).advance_width(id);
        let mut glyph = Glyph {
            advance: (advance + 0.5) as i32,
            ..Default::default()
        };
        let mut scaler = sheet.scale.builder(font).size(self.size as f32).hint(true).build();
        let sources = [Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline];
        let Some(image) = Render::new(&sources).format(Format::Alpha).render(&mut scaler, id) else {
            return glyph;
        };
        let (w, h) = (image.placement.width, image.placement.height);
        if w == 0 || h == 0 {
            return glyph;
        }
        let (texture, x, y) = sheet.allocate(w, h, renderer);
        let rgba: Vec<u8> = image.data.iter().flat_map(|&p| [p, p, p, p]).collect();
        renderer.update_ui_texture(texture, x, y, w, h, &rgba);
        glyph.x = x;
        glyph.y = y;
        glyph.w = w;
        glyph.h = h;
        glyph.left = image.placement.left;
        glyph.top = -image.placement.top;
        glyph
    }

    fn glyph(&mut self, c: char, sheet: &mut GlyphSheet, renderer: &mut Renderer) -> Glyph {
        if let Some(g) = self.glyphs.get(&c) {
            return *g;
        }
        let g = self.create_glyph(c, sheet, renderer);
        self.glyphs.insert(c, g);
        g
    }

    /// Port of `Measure`.
    pub fn measure(&mut self, text: &str, sheet: &mut GlyphSheet, renderer: &mut Renderer) -> (i32, i32) {
        if text.is_empty() {
            return (0, self.size);
        }
        let mut max_width = 0;
        let mut rows = 0;
        for line in text.split('\n') {
            rows += 1;
            let width = line.chars().map(|c| self.glyph(c, sheet, renderer).advance).sum();
            max_width = i32::max(max_width, width);
        }
        (max_width, rows * self.size)
    }

    /// Port of `DrawText`.
    pub fn draw_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: Color,
        sheet: &mut GlyphSheet,
        renderer: &mut Renderer,
        list: &mut UiList,
    ) {
        let tint = [color.r() as f32 / 255.0, color.g() as f32 / 255.0, color.b() as f32 / 255.0, 1.0];
        let y = y + self.size as f32;
        let mut row = 0;
        for line in text.split('\n') {
            let mut pen = (x + 0.5) as i32;
            let base = (y + (row * self.size) as f32 + 0.5) as i32;
            for c in line.chars() {
                let g = self.glyph(c, sheet, renderer);
                if g.w > 0 {
                    list.quads.push(UiQuad {
                        texture: sheet.texture.expect("glyph sheet"),
                        dst: [(pen + g.left) as f32, (base + g.top) as f32, g.w as f32, g.h as f32],
                        src: [g.x as f32, g.y as f32, g.w as f32, g.h as f32],
                        color: tint,
                    });
                }
                pen += g.advance;
            }
            row += 1;
        }
    }
}

/// Converts a color to the premultiplied form that the user interface pass expects.
pub fn premultiplied(color: Color) -> [f32; 4] {
    let a = color.a() as f32 / 255.0;
    [
        color.r() as f32 / 255.0 * a,
        color.g() as f32 / 255.0 * a,
        color.b() as f32 / 255.0 * a,
        a,
    ]
}
