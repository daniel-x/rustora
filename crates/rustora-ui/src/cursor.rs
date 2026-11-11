// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `CursorProvider`, `CursorSequence` and the hardware cursor part of `CursorManager`.

use std::collections::HashMap;

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_formats::palette::Palette;
use rustora_formats::sprite::{self, SpriteFrame};
use rustora_fs::ReadFs;
use rustora_game::GameError;
use rustora_game::moddata::ModData;

/// One cursor frame padded as for a hardware cursor. `rgba` has straight alpha.
pub struct CursorImage {
    pub rgba: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub hotspot: (u16, u16),
}

impl CursorImage {
    /// `DoublePixelData` applied `log2(factor)` times: each pixel becomes a `factor` by `factor` square, and the
    /// hotspot moves with it.
    pub fn scaled(&self, factor: u16) -> CursorImage {
        let f = factor.max(1) as usize;
        let (w, h) = (self.width as usize, self.height as usize);
        let mut rgba = vec![0; 4 * w * h * f * f];
        for y in 0..h * f {
            for x in 0..w * f {
                let src = 4 * ((y / f) * w + x / f);
                let dest = 4 * (y * w * f + x);
                rgba[dest..dest + 4].copy_from_slice(&self.rgba[src..src + 4]);
            }
        }
        CursorImage {
            rgba,
            width: self.width * factor.max(1),
            height: self.height * factor.max(1),
            hotspot: (self.hotspot.0 * factor.max(1), self.hotspot.1 * factor.max(1)),
        }
    }
}

/// All cursors of a mod by name.
#[derive(Default)]
pub struct Cursors {
    sequences: HashMap<String, Vec<CursorImage>>,
}

fn field<'a>(node: &'a MiniYaml, key: &str) -> Option<&'a str> {
    node.nodes.iter().find(|n| n.key == key).map(|n| n.value.value_str())
}

/// Finds the file of the `PaletteFromFile` trait named `name` with `CursorPalette` set.
fn cursor_palette(rules: &[MiniYamlNode], name: &str, fs: &dyn ReadFs) -> Option<Palette> {
    let trait_node = rules.iter().flat_map(|actor| &actor.value.nodes).find(|t| {
        t.key.starts_with("PaletteFromFile") && field(&t.value, "Name") == Some(name) && field(&t.value, "CursorPalette") == Some("true")
    })?;
    let data = fs.try_open(field(&trait_node.value, "Filename")?)?;
    let transparent: usize = field(&trait_node.value, "TransparentIndex").map_or(Some(0), |v| v.parse().ok())?;
    rustora_formats::palette::load(&data, &[transparent], &[])
}

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// `CursorSequence`: selects the frames of a sequence.
fn sequence_frames<'a>(frames: &'a [SpriteFrame], info: &MiniYaml) -> &'a [SpriteFrame] {
    let start: usize = field(info, "Start").and_then(|v| v.parse().ok()).unwrap_or(0).min(frames.len());
    let rest = &frames[start..];
    let length = match (field(info, "Length"), field(info, "End")) {
        (Some("*"), _) | (_, Some("*")) => rest.len(),
        (Some(length), _) => length.parse().unwrap_or(1),
        (None, Some(end)) => end.parse::<usize>().unwrap_or(start + 1).saturating_sub(start),
        (None, None) => 1,
    };
    &rest[..length.min(rest.len())]
}

fn straight(c: [u8; 4]) -> [u8; 4] {
    match c[3] {
        0 => [0, 0, 0, 0],
        255 => c,
        a => {
            let up = |v: u8| ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            [up(c[0]), up(c[1]), up(c[2]), a]
        }
    }
}

/// `CursorManager`: pads every frame of a sequence to one size, a multiple of 8, with the hotspot inside it.
fn build_images(
    frames: &[SpriteFrame],
    pixel: &dyn Fn(&SpriteFrame, usize, usize) -> [u8; 4],
    sequence_hotspot: (i32, i32),
) -> Vec<CursorImage> {
    let hotspots: Vec<(i32, i32)> = frames
        .iter()
        .map(|f| {
            (
                f.offset.0 as i32 - sequence_hotspot.0 - f.width as i32 / 2,
                f.offset.1 as i32 - sequence_hotspot.1 - f.height as i32 / 2,
            )
        })
        .collect();
    let (mut left, mut top, mut right, mut bottom) = (0, 0, 1, 1);
    for (f, h) in frames.iter().zip(&hotspots) {
        left = left.min(h.0);
        top = top.min(h.1);
        right = right.max(h.0 + f.width as i32);
        bottom = bottom.max(h.1 + f.height as i32);
    }
    let width = 8 * ((right - left + 7) / 8);
    let height = 8 * ((bottom - top + 7) / 8);
    frames
        .iter()
        .zip(&hotspots)
        .map(|(f, h)| {
            let mut rgba = vec![0; 4 * (width * height) as usize];
            let (pad_x, pad_y) = ((h.0 - left) as usize, (h.1 - top) as usize);
            for j in 0..f.height {
                for i in 0..f.width {
                    let dest = 4 * ((j + pad_y) * width as usize + i + pad_x);
                    rgba[dest..dest + 4].copy_from_slice(&pixel(f, i, j));
                }
            }
            CursorImage {
                rgba,
                width: width as u16,
                height: height as u16,
                hotspot: ((-left) as u16, (-top) as u16),
            }
        })
        .collect()
}

impl Cursors {
    pub fn load(mod_data: &ModData, fs: &dyn ReadFs) -> Result<Cursors, GameError> {
        let yaml = ModData::load_yaml(fs, &mod_data.manifest.cursors, None)?;
        let rules = ModData::load_yaml(fs, &mod_data.manifest.rules, None)?;
        let mut sequences = HashMap::new();
        let Some(root) = yaml.iter().find(|n| n.key == "Cursors") else {
            return Ok(Cursors { sequences });
        };
        for source in &root.value.nodes {
            let data = fs.open(&source.key).map_err(|e| GameError::Data(e.to_string()))?;
            if data.starts_with(PNG_SIGNATURE) {
                let (width, height, rgba) =
                    crate::chrome::decode_png(&data).map_err(|e| GameError::Data(format!("{}: {e}", source.key)))?;
                let (width, height) = (width as usize, height as usize);
                let frame = SpriteFrame {
                    width,
                    height,
                    frame_width: width,
                    frame_height: height,
                    offset: (0.0, 0.0),
                    data: Vec::new(),
                };
                let pixel = |_: &SpriteFrame, i: usize, j: usize| {
                    let at = 4 * (j * width + i);
                    straight([rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]])
                };
                for sequence in &source.value.nodes {
                    let hotspot = |key| field(&sequence.value, key).and_then(|v| v.parse().ok()).unwrap_or(0);
                    let images = build_images(std::slice::from_ref(&frame), &pixel, (hotspot("X"), hotspot("Y")));
                    sequences.insert(sequence.key.clone(), images);
                }
                continue;
            }
            let frames = sprite::decode(&data).ok_or_else(|| GameError::Data(format!("cursor file {}", source.key)))?;
            let palette_name = source.value.value_str();
            let palette =
                cursor_palette(&rules, palette_name, fs).ok_or_else(|| GameError::Data(format!("cursor palette {palette_name}")))?;
            for sequence in &source.value.nodes {
                let hotspot = |key| field(&sequence.value, key).and_then(|v| v.parse().ok()).unwrap_or(0);
                let pixel = |f: &SpriteFrame, i: usize, j: usize| straight(palette[f.data[j * f.width + i] as usize]);
                let images = build_images(sequence_frames(&frames, &sequence.value), &pixel, (hotspot("X"), hotspot("Y")));
                sequences.insert(sequence.key.clone(), images);
            }
        }
        Ok(Cursors { sequences })
    }

    /// `CursorProvider.HasCursorSequence` for all names at once.
    pub fn names(&self) -> std::collections::HashSet<String> {
        self.sequences.keys().cloned().collect()
    }

    pub fn image(&self, name: &str, frame: usize) -> Option<&CursorImage> {
        self.sequences.get(name).and_then(|s| s.get(frame))
    }

    fn length(&self, name: &str) -> usize {
        self.sequences.get(name).map_or(0, Vec::len)
    }
}

/// The cursor over a link: the pointing hand of the system, which the mods do not have.
pub const LINK_CURSOR: &str = "rustora-link";

/// The animation state of `CursorManager`.
#[derive(Default)]
pub struct CursorState {
    name: Option<String>,
    frame: usize,
    ticks: i32,
}

impl CursorState {
    /// `SetCursor`: an unknown name hides the cursor.
    pub fn set(&mut self, cursors: &Cursors, name: Option<&str>) {
        let known = name.filter(|n| cursors.length(n) > 0);
        if self.name.as_deref() != known {
            self.name = known.map(str::to_string);
        }
    }

    /// `Tick`.
    pub fn tick(&mut self, cursors: &Cursors) {
        let Some(name) = &self.name else { return };
        if cursors.length(name) == 1 {
            return;
        }
        self.ticks += 1;
        if self.ticks > 2 {
            self.ticks -= 2;
            self.frame += 1;
        }
    }

    /// The name and frame to show, or `None` when the cursor is hidden.
    pub fn current(&self, cursors: &Cursors) -> Option<(&str, usize)> {
        let name = self.name.as_deref()?;
        Some((name, self.frame % cursors.length(name)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: usize, height: usize, offset: (f32, f32)) -> SpriteFrame {
        SpriteFrame {
            width,
            height,
            frame_width: width,
            frame_height: height,
            offset,
            data: vec![1; width * height],
        }
    }

    fn palette() -> Palette {
        let mut p = [[0; 4]; 256];
        p[1] = [10, 20, 30, 255];
        p
    }

    #[test]
    fn pads_to_a_multiple_of_8_with_the_hotspot_inside() {
        let images = build_images(
            &[frame(29, 24, (0.0, 0.0))],
            &|f: &SpriteFrame, i, j| straight(palette()[f.data[j * f.width + i] as usize]),
            (0, 0),
        );
        assert_eq!((images[0].width, images[0].height), (32, 24));
        assert_eq!(images[0].hotspot, (14, 12));
    }

    #[test]
    fn sequence_hotspot_moves_the_frame() {
        let images = build_images(
            &[frame(10, 10, (0.0, 0.0))],
            &|f: &SpriteFrame, i, j| straight(palette()[f.data[j * f.width + i] as usize]),
            (0, -7),
        );
        assert_eq!(images[0].hotspot, (5, 0));
        let row_start = |row: usize| 4 * row * images[0].width as usize;
        assert_eq!(images[0].rgba[row_start(1) + 3], 0);
        assert_eq!(images[0].rgba[row_start(2) + 3], 255);
    }

    #[test]
    fn frames_of_a_sequence_share_one_size_and_hotspot() {
        let images = build_images(
            &[frame(4, 4, (0.0, 0.0)), frame(8, 8, (2.0, 2.0))],
            &|f: &SpriteFrame, i, j| straight(palette()[f.data[j * f.width + i] as usize]),
            (0, 0),
        );
        assert_eq!((images[0].width, images[0].height), (8, 8));
        assert_eq!((images[1].width, images[1].height), (8, 8));
        assert_eq!(images[0].hotspot, images[1].hotspot);
    }

    #[test]
    fn scaling_doubles_pixels_and_hotspot() {
        let image = CursorImage {
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
            width: 2,
            height: 1,
            hotspot: (1, 0),
        };
        let doubled = image.scaled(2);
        assert_eq!((doubled.width, doubled.height, doubled.hotspot), (4, 2, (2, 0)));
        assert_eq!(&doubled.rgba[..16], &[1, 2, 3, 4, 1, 2, 3, 4, 5, 6, 7, 8, 5, 6, 7, 8]);
        assert_eq!(doubled.rgba[..16], doubled.rgba[16..]);
    }

    #[test]
    fn straight_alpha_restores_premultiplied_colors() {
        assert_eq!(straight([50, 50, 50, 128]), [100, 100, 100, 128]);
        assert_eq!(straight([7, 7, 7, 0]), [0, 0, 0, 0]);
    }
}
