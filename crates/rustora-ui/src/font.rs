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
    advances: HashMap<char, i32>,
    alpha: HashMap<char, Vec<u8>>,
    contrast_glyphs: HashMap<(char, i32), Glyph>,
    dilation_elements: HashMap<i32, Vec<f32>>,
}

/// Port of `CreateCircularWeightMap`: the intersection of each pixel of a `2 * r + 1` square with a circle.
fn circular_weight_map(r: i32) -> Vec<f32> {
    let stride = 2 * r + 1;
    let mut elem = vec![0.0f32; (stride * stride) as usize];
    for j in 0..stride {
        for i in 0..stride {
            let (di, dj) = (i - r, j - r);
            let distance = di * di + dj * dj;
            if distance > (r + 1) * (r + 1) {
                continue;
            }
            let weight = &mut elem[(j * stride + i) as usize];
            if distance < (r - 1) * (r - 1) {
                *weight = 1.0;
                continue;
            }
            for jj in 0..5 {
                for ii in 0..5 {
                    let si = di as f32 - (di.signum() * ii) as f32 / 5.0;
                    let sj = dj as f32 - (dj.signum() * jj) as f32 / 5.0;
                    if si * si + sj * sj <= (r * r) as f32 {
                        *weight += 0.04;
                    }
                }
            }
        }
    }
    elem
}

/// Port of the dilation in `CreateContrastGlyph`: expands the alpha of a glyph by `r` pixels on each side.
fn dilate(alpha: &[u8], w: i32, h: i32, r: i32, elem: &[f32]) -> Vec<u8> {
    let (out_w, out_h) = (w + 2 * r, h + 2 * r);
    let stride = 2 * r + 1;
    let mut out = vec![0; (out_w * out_h) as usize];
    for j in 0..out_h {
        for i in 0..out_w {
            let mut best = 0u8;
            for wj in 0..stride {
                for wi in 0..stride {
                    let (ii, jj) = (i + wi - 2 * r, j + wj - 2 * r);
                    if ii < 0 || ii >= w || jj < 0 || jj >= h {
                        continue;
                    }
                    let source = alpha[(jj * w + ii) as usize] as f32;
                    best = best.max((elem[(wj * stride + wi) as usize] * source) as u8);
                }
            }
            out[(j * out_w + i) as usize] = best;
        }
    }
    out
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
            advances: HashMap::new(),
            alpha: HashMap::new(),
            contrast_glyphs: HashMap::new(),
            dilation_elements: HashMap::new(),
        }
    }

    fn create_glyph(&mut self, c: char, sheet: &mut GlyphSheet, renderer: &mut Renderer) -> Glyph {
        let data = std::mem::take(&mut self.data);
        let glyph = self.rasterize(&data, c, sheet, renderer);
        self.data = data;
        glyph
    }

    fn rasterize(&mut self, data: &[u8], c: char, sheet: &mut GlyphSheet, renderer: &mut Renderer) -> Glyph {
        let Some(font) = FontRef::from_index(data, 0) else {
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
        self.alpha.insert(c, image.data);
        glyph
    }

    /// Port of `contrastGlyphs` and `CreateContrastGlyph`.
    fn contrast_glyph(&mut self, c: char, r: i32, sheet: &mut GlyphSheet, renderer: &mut Renderer) -> Glyph {
        if let Some(g) = self.contrast_glyphs.get(&(c, r)) {
            return *g;
        }
        let source = self.glyph(c, sheet, renderer);
        let elem = self.dilation_elements.entry(r).or_insert_with(|| circular_weight_map(r));
        let alpha = self.alpha.get(&c).map(Vec::as_slice).unwrap_or_default();
        let dilated = dilate(alpha, source.w as i32, source.h as i32, r, elem);
        let (w, h) = (source.w + 2 * r as u32, source.h + 2 * r as u32);
        let (texture, x, y) = sheet.allocate(w, h, renderer);
        let rgba: Vec<u8> = dilated.iter().flat_map(|&p| [p, p, p, p]).collect();
        renderer.update_ui_texture(texture, x, y, w, h, &rgba);
        let glyph = Glyph { x, y, w, h, ..source };
        self.contrast_glyphs.insert((c, r), glyph);
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

    /// The advance of a glyph, without loading its image.
    fn advance(&mut self, c: char) -> i32 {
        if let Some(a) = self.advances.get(&c) {
            return *a;
        }
        let advance = FontRef::from_index(&self.data, 0).map_or(0, |font| {
            let id = font.charmap().map(c);
            (font.glyph_metrics(&[]).scale(self.size as f32).advance_width(id) + 0.5) as i32
        });
        self.advances.insert(c, advance);
        advance
    }

    /// Port of `Measure`.
    pub fn measure(&mut self, text: &str) -> (i32, i32) {
        if text.is_empty() {
            return (0, self.size);
        }
        let mut max_width = 0;
        let mut rows = 0;
        for line in text.split('\n') {
            rows += 1;
            let width = line.chars().map(|c| self.advance(c)).sum();
            max_width = i32::max(max_width, width);
        }
        (max_width, rows * self.size)
    }

    /// Port of `DrawTextContrast`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_contrast(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: Color,
        r: i32,
        sheet: &mut GlyphSheet,
        renderer: &mut Renderer,
        list: &mut UiList,
    ) {
        let tint = [color.r() as f32 / 255.0, color.g() as f32 / 255.0, color.b() as f32 / 255.0, 1.0];
        let y = y + self.size as f32;
        for (row, line) in text.split('\n').enumerate() {
            let mut pen = (x + 0.5) as i32;
            let base = (y + (row as i32 * self.size) as f32 + 0.5) as i32;
            for c in line.chars() {
                let source = self.glyph(c, sheet, renderer);
                if source.w > 0 {
                    let g = self.contrast_glyph(c, r, sheet, renderer);
                    list.quads.push(UiQuad {
                        texture: sheet.texture.expect("glyph sheet"),
                        dst: [(pen + g.left - r) as f32, (base + g.top - r) as f32, g.w as f32, g.h as f32],
                        src: [g.x as f32, g.y as f32, g.w as f32, g.h as f32],
                        color: tint,
                        gradient: None,
                        corners: None,
                        pixel_art: false,
                    });
                }
                pen += source.advance;
            }
        }
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
        for (row, line) in text.split('\n').enumerate() {
            let mut pen = (x + 0.5) as i32;
            let base = (y + (row as i32 * self.size) as f32 + 0.5) as i32;
            for c in line.chars() {
                let g = self.glyph(c, sheet, renderer);
                if g.w > 0 {
                    list.quads.push(UiQuad {
                        texture: sheet.texture.expect("glyph sheet"),
                        dst: [(pen + g.left) as f32, (base + g.top) as f32, g.w as f32, g.h as f32],
                        src: [g.x as f32, g.y as f32, g.w as f32, g.h as f32],
                        color: tint,
                        gradient: None,
                        corners: None,
                        pixel_art: false,
                    });
                }
                pen += g.advance;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_map_of_radius_1_matches_the_csharp_comment() {
        let m = circular_weight_map(1);
        let rounded: Vec<i32> = m.iter().map(|v| (v * 100.0).round() as i32).collect();
        assert_eq!(rounded, [60, 100, 60, 100, 100, 100, 60, 100, 60]);
    }

    #[test]
    fn weight_map_of_radius_3_matches_the_csharp_comment() {
        let m = circular_weight_map(3);
        let row = |j: usize| m[j * 7..j * 7 + 7].iter().map(|v| (v * 100.0).round() as i32).collect::<Vec<_>>();
        assert_eq!(row(0), [0, 44, 80, 100, 80, 44, 0]);
        assert_eq!(row(1), [44, 100, 100, 100, 100, 100, 44]);
        assert_eq!(row(3), [100; 7]);
    }

    #[test]
    fn dilation_grows_one_pixel_by_the_radius() {
        let out = dilate(&[200], 1, 1, 1, &circular_weight_map(1));
        assert_eq!(out, [119, 200, 119, 200, 200, 200, 119, 200, 119]);
    }
}
