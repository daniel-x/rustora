// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Player color validation (`ColorPickerManagerInfo`) and the HSV conversions of `Primitives/Color.cs`.

use rustora_core::fieldloader::Color;
use rustora_core::world_types::MersenneTwister;

const PLAYER_COLOR_TERRAIN: &str = "notification-player-color-terrain";
const PLAYER_COLOR_PLAYER: &str = "notification-player-color-player";
const INVALID_PLAYER_COLOR: &str = "notification-invalid-player-color";

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

/// `Color.FromAhsv` with full alpha.
pub fn from_hsv(h: f32, s: f32, v: f32) -> Color {
    let px = (h * 6.0 - 3.0).abs();
    let py = ((h + 2.0 / 3.0) % 1.0 * 6.0 - 3.0).abs();
    let pz = ((h + 1.0 / 3.0) % 1.0 * 6.0 - 3.0).abs();
    let channel = |p: f32| (255.0 * (v * lerp(1.0, (p - 1.0).clamp(0.0, 1.0), s))).round_ties_even() as u8;
    Color::from_argb(255, channel(px), channel(py), channel(pz))
}

/// `Color.ToAhsv` without the alpha.
pub fn to_hsv(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let max = r.max(g.max(b));
    let delta = max - r.min(g.min(b));
    if delta == 0.0 {
        return (0.0, 0.0, max);
    }
    let hue = if r == max {
        (g - b) / (6.0 * delta)
    } else if g == max {
        (b - r) / (6.0 * delta) + 1.0 / 3.0
    } else {
        (r - g) / (6.0 * delta) + 2.0 / 3.0
    };
    let mut h = hue - hue.trunc();
    if h < 0.0 {
        h += 1.0;
    }
    (h, delta / max, max)
}

/// The fields of `ColorPickerManagerInfo` that the server and the chooser use.
#[derive(Clone, Debug)]
pub struct ColorPicker {
    pub saturation_range: [f32; 2],
    pub value_range: [f32; 2],
    pub similarity_threshold: i32,
    pub preset_colors: Vec<Color>,
}

impl Default for ColorPicker {
    fn default() -> Self {
        ColorPicker {
            saturation_range: [0.3, 0.95],
            value_range: [0.3, 0.95],
            similarity_threshold: 0x50,
            preset_colors: Vec::new(),
        }
    }
}

impl ColorPicker {
    /// `IsInvalidColor`: true when `color` is too similar to a blocker, by the metric of compuphase.com/cmetric.htm.
    pub fn is_invalid(&self, color: Color, blockers: &[Color]) -> bool {
        blockers.iter().any(|c| {
            let (r1, g1, b1) = (color.r() as i32, color.g() as i32, color.b() as i32);
            let (r2, g2, b2) = (c.r() as i32, c.g() as i32, c.b() as i32);
            let rmean = (r1 + r2) >> 1;
            let (dr, dg, db) = (r1 - r2, g1 - g2, b1 - b2);
            let weight = (((512 + rmean) * dr * dr) >> 8) + 4 * dg * dg + (((767 - rmean) * db * db) >> 8);
            (weight as f64).sqrt() < self.similarity_threshold as f64
        })
    }

    fn random_sat(&self, random: &mut MersenneTwister) -> f32 {
        lerp(self.saturation_range[0], self.saturation_range[1], random.next_float())
    }

    fn random_val(&self, random: &mut MersenneTwister) -> f32 {
        lerp(self.value_range[0], self.value_range[1], random.next_float())
    }

    fn make_valid_hsv(
        &self,
        hue: f32,
        sat: f32,
        val: f32,
        random: &mut MersenneTwister,
        terrain: &[Color],
        players: &[Color],
    ) -> (Color, Option<&'static str>) {
        let sat = sat.clamp(self.saturation_range[0], self.saturation_range[1]);
        let val = val.clamp(self.value_range[0], self.value_range[1]);
        let color = from_hsv(hue, sat, val);
        let error = if self.is_invalid(color, terrain) {
            PLAYER_COLOR_TERRAIN
        } else if self.is_invalid(color, players) {
            PLAYER_COLOR_PLAYER
        } else {
            return (color, None);
        };
        for i in 2..402 {
            let sign = if i % 2 == 0 { -1.0 } else { 1.0 };
            let h = hue + sign * (i / 2) as f32 * 0.1 * (0.2 + random.next_float());
            let s = self.random_sat(random);
            let v = self.random_val(random);
            let color = from_hsv(h, s, v);
            if !self.is_invalid(color, terrain) && !self.is_invalid(color, players) {
                return (color, Some(error));
            }
        }
        let s = self.random_sat(random);
        let v = self.random_val(random);
        (from_hsv(random.next_float(), s, v), Some(INVALID_PLAYER_COLOR))
    }

    /// `MakeValid`: `color` if allowed, else a similar allowed color and the fluent key of the error to report.
    pub fn make_valid(
        &self,
        color: Color,
        random: &mut MersenneTwister,
        terrain: &[Color],
        players: &[Color],
    ) -> (Color, Option<&'static str>) {
        let (h, s, v) = to_hsv(color);
        self.make_valid_hsv(h, s, v, random, terrain, players)
    }

    /// `RandomValidColor`.
    pub fn random_valid_color(&self, random: &mut MersenneTwister, terrain: &[Color], players: &[Color]) -> Color {
        let h = random.next_float();
        let s = self.random_sat(random);
        let v = self.random_val(random);
        self.make_valid_hsv(h, s, v, random, terrain, players).0
    }

    /// `RandomPresetColor`: a free preset color in random order, else a random valid color.
    pub fn random_preset_color(&self, random: &mut MersenneTwister, terrain: &[Color], players: &[Color]) -> Color {
        for color in rustora_core::world_types::shuffle(&self.preset_colors, random) {
            if !self.is_invalid(color, terrain) && !self.is_invalid(color, players) {
                return color;
            }
        }
        let h = random.next_float();
        let s = self.random_sat(random);
        let v = self.random_val(random);
        self.make_valid_hsv(h, s, v, random, terrain, players).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trip() {
        let c = Color::from_argb(255, 0xF5, 0x76, 0x06);
        let (h, s, v) = to_hsv(c);
        assert_eq!(from_hsv(h, s, v), c);
    }

    #[test]
    fn similar_colors_are_invalid() {
        let p = ColorPicker::default();
        let c = Color::from_argb(255, 200, 10, 10);
        assert!(p.is_invalid(c, &[Color::from_argb(255, 205, 12, 8)]));
        assert!(!p.is_invalid(c, &[Color::from_argb(255, 10, 200, 200)]));
    }

    #[test]
    fn taken_color_moves_away() {
        let p = ColorPicker::default();
        let mut r = MersenneTwister::new(1);
        let c = Color::from_argb(255, 200, 40, 40);
        let (got, err) = p.make_valid(c, &mut r, &[], &[c]);
        assert_eq!(err, Some(PLAYER_COLOR_PLAYER));
        assert!(!p.is_invalid(got, &[c]));
    }
}
