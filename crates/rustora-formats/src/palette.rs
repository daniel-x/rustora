// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ImmutablePalette` loading and `PlayerColorRemap`. Colors are premultiplied RGBA.

pub type Palette = [[u8; 4]; 256];

/// `ImmutablePalette(Stream, remapTransparent, remapShadow)` for a 6-bit VGA palette file.
pub fn load(d: &[u8], transparent: &[usize], shadow: &[usize]) -> Option<Palette> {
    let mut p = [[0u8; 4]; 256];
    for (i, c) in p.iter_mut().enumerate() {
        let ch = |k: usize| {
            d.get(3 * i + k).map(|&v| {
                let v = v << 2;
                v | (v >> 6)
            })
        };
        *c = [ch(0)?, ch(1)?, ch(2)?, 255];
    }
    for &i in transparent {
        p[i] = [0, 0, 0, 0];
    }
    for &i in shadow {
        p[i] = [0, 0, 0, 140];
    }
    Some(p)
}

/// A palette of one premultiplied color.
pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Palette {
    let m = |c: u8| ((c as u32 * a as u32) / 255) as u8;
    let mut p = [[m(r), m(g), m(b), a]; 256];
    p[0] = [0, 0, 0, 0];
    p
}

fn to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(c: f32) -> u8 {
    let c = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    (h, if max == 0.0 { 0.0 } else { delta / max }, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h6 = (h * 6.0).rem_euclid(6.0);
    let c = v * s;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let (r, g, b) = match h6 as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (r + m, g + m, b + m)
}

/// `PlayerColorRemap` applied to `indices` of `base` for the player color `rgb`.
pub fn player_remap(base: &Palette, indices: &[usize], rgb: [u8; 3]) -> Palette {
    let (hue, sat, val) = rgb_to_hsv(to_linear(rgb[0]), to_linear(rgb[1]), to_linear(rgb[2]));
    let mut p = *base;
    for &i in indices {
        let [r, g, b, a] = base[i];
        let v = to_linear(r).max(to_linear(g)).max(to_linear(b));
        let (r, g, b) = hsv_to_rgb(hue, sat, v * val);
        let m = |c: f32| ((from_linear(c) as u32 * a as u32) / 255) as u8;
        p[i] = [m(r), m(g), m(b), a];
    }
    p
}

/// `AlphaPaletteRemap`: scales the alpha, and with `premultiply` the color channels, by `alpha`.
pub fn alpha_remap(base: &Palette, alpha: f32, premultiply: bool) -> Palette {
    let mut p = *base;
    for c in p.iter_mut() {
        let [r, g, b, a] = *c;
        let scale = |v: u8| {
            if premultiply {
                ((alpha * v as f32 + 0.5) as i32).clamp(0, 255) as u8
            } else {
                v
            }
        };
        *c = [scale(r), scale(g), scale(b), ((a as f32 * alpha) as i32).clamp(0, 255) as u8];
    }
    p
}
