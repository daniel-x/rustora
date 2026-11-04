// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ChromeProvider.cs` and `ChromeMetrics.cs`.

use std::collections::HashMap;

use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_fs::ReadFs;
use rustora_render::Renderer;
use rustora_render::ui::UiTexture;

/// A region of a user interface texture.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub texture: UiTexture,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Default)]
struct Collection {
    image: String,
    panel_region: Option<Vec<i32>>,
    panel_sides: u32,
    regions: HashMap<String, [i32; 4]>,
}

const LEFT: u32 = 1;
const TOP: u32 = 2;
const RIGHT: u32 = 4;
const BOTTOM: u32 = 8;
const CENTER: u32 = 16;
const ALL: u32 = 31;

fn int_list(value: &str) -> Vec<i32> {
    value.split(',').filter_map(|v| v.trim().parse().ok()).collect()
}

fn parse_sides(value: &str) -> u32 {
    value.split(',').fold(0, |sides, side| {
        sides
            | match side.trim() {
                "Left" => LEFT,
                "Top" => TOP,
                "Right" => RIGHT,
                "Bottom" => BOTTOM,
                "Center" => CENTER,
                "Edges" => LEFT | TOP | RIGHT | BOTTOM,
                "All" => ALL,
                _ => 0,
            }
    })
}

/// Decodes a PNG file to premultiplied RGBA rows, as `Sheet` does.
pub fn decode_png(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buffer).map_err(|e| e.to_string())?;
    let pixels = (info.width * info.height) as usize;
    let mut rgba = Vec::with_capacity(pixels * 4);
    for i in 0..pixels {
        let (r, g, b, a) = match info.color_type {
            png::ColorType::Rgba => (buffer[4 * i], buffer[4 * i + 1], buffer[4 * i + 2], buffer[4 * i + 3]),
            png::ColorType::Rgb => (buffer[3 * i], buffer[3 * i + 1], buffer[3 * i + 2], 255),
            png::ColorType::GrayscaleAlpha => (buffer[2 * i], buffer[2 * i], buffer[2 * i], buffer[2 * i + 1]),
            png::ColorType::Grayscale => (buffer[i], buffer[i], buffer[i], 255),
            png::ColorType::Indexed => return Err("indexed png after expansion".into()),
        };
        let premultiply = |c: u8| ((c as u32 * a as u32) / 255) as u8;
        rgba.extend_from_slice(&[premultiply(r), premultiply(g), premultiply(b), a]);
    }
    Ok((info.width, info.height, rgba))
}

/// The chrome image collections of a mod.
pub struct Chrome {
    collections: HashMap<String, Collection>,
    textures: HashMap<String, (UiTexture, u32, u32)>,
    panels: HashMap<String, Option<Vec<Option<Sprite>>>>,
}

impl Chrome {
    /// The width and height of a region of a collection, without loading its image.
    pub fn region_size(&self, collection: &str, image: &str) -> Option<(i32, i32)> {
        let region = self.collections.get(collection)?.regions.get(image)?;
        Some((region[2], region[3]))
    }

    pub fn new(nodes: &[MiniYamlNode]) -> Chrome {
        let mut collections = HashMap::new();
        for node in nodes.iter().filter(|n| !n.key.starts_with('^')) {
            let mut c = Collection {
                panel_sides: ALL,
                ..Default::default()
            };
            for field in &node.value.nodes {
                match field.key.as_str() {
                    "Image" => c.image = field.value.value_str().to_string(),
                    "PanelRegion" => c.panel_region = Some(int_list(field.value.value_str())),
                    "PanelSides" => c.panel_sides = parse_sides(field.value.value_str()),
                    "Regions" => {
                        for r in &field.value.nodes {
                            let v = int_list(r.value.value_str());
                            if let [x, y, w, h] = v[..] {
                                c.regions.insert(r.key.clone(), [x, y, w, h]);
                            }
                        }
                    }
                    _ => {}
                }
            }
            collections.insert(node.key.clone(), c);
        }
        Chrome {
            collections,
            textures: HashMap::new(),
            panels: HashMap::new(),
        }
    }

    fn texture(&mut self, collection: &str, fs: &dyn ReadFs, renderer: &mut Renderer) -> Option<UiTexture> {
        let image = self.collections.get(collection)?.image.clone();
        if let Some(t) = self.textures.get(&image) {
            return Some(t.0);
        }
        let data = fs.try_open(&image)?;
        let (w, h, rgba) = decode_png(&data).ok()?;
        let texture = renderer.add_ui_texture(w, h, &rgba, true);
        self.textures.insert(image, (texture, w, h));
        Some(texture)
    }

    /// Port of `TryGetImage`.
    pub fn image(&mut self, collection: &str, image: &str, fs: &dyn ReadFs, renderer: &mut Renderer) -> Option<Sprite> {
        let [x, y, w, h] = *self.collections.get(collection)?.regions.get(image)?;
        let texture = self.texture(collection, fs, renderer)?;
        Some(Sprite { texture, x, y, w, h })
    }

    pub fn has_collection(&self, collection: &str) -> bool {
        self.collections.contains_key(collection)
    }

    /// Port of `TryGetPanelImages`: nine sprites TL, T, TR, L, C, R, BL, B, BR.
    pub fn panel(&mut self, name: &str, fs: &dyn ReadFs, renderer: &mut Renderer) -> Option<Vec<Option<Sprite>>> {
        if let Some(p) = self.panels.get(name) {
            return p.clone();
        }
        let result = self.load_panel(name, fs, renderer);
        self.panels.insert(name.to_string(), result.clone());
        result
    }

    fn load_panel(&mut self, name: &str, fs: &dyn ReadFs, renderer: &mut Renderer) -> Option<Vec<Option<Sprite>>> {
        let collection = self.collections.get(name)?;
        if let Some(pr) = collection.panel_region.clone() {
            if pr.len() != 8 {
                return None;
            }
            let sides = collection.panel_sides;
            let texture = self.texture(name, fs, renderer)?;
            let parts = [
                (TOP | LEFT, pr[0], pr[1], pr[2], pr[3]),
                (TOP, pr[0] + pr[2], pr[1], pr[4], pr[3]),
                (TOP | RIGHT, pr[0] + pr[2] + pr[4], pr[1], pr[6], pr[3]),
                (LEFT, pr[0], pr[1] + pr[3], pr[2], pr[5]),
                (CENTER, pr[0] + pr[2], pr[1] + pr[3], pr[4], pr[5]),
                (RIGHT, pr[0] + pr[2] + pr[4], pr[1] + pr[3], pr[6], pr[5]),
                (BOTTOM | LEFT, pr[0], pr[1] + pr[3] + pr[5], pr[2], pr[7]),
                (BOTTOM, pr[0] + pr[2], pr[1] + pr[3] + pr[5], pr[4], pr[7]),
                (BOTTOM | RIGHT, pr[0] + pr[2] + pr[4], pr[1] + pr[3] + pr[5], pr[6], pr[7]),
            ];
            return Some(
                parts
                    .iter()
                    .map(|&(side, x, y, w, h)| (sides & side == side).then_some(Sprite { texture, x, y, w, h }))
                    .collect(),
            );
        }
        if collection.regions.is_empty() {
            return Some(Vec::new());
        }
        let names = [
            "corner-tl",
            "border-t",
            "corner-tr",
            "border-l",
            "background",
            "border-r",
            "corner-bl",
            "border-b",
            "corner-br",
        ];
        Some(names.iter().map(|n| self.image(name, n, fs, renderer)).collect())
    }
}

/// Port of `ChromeMetrics`.
#[derive(Default)]
pub struct Metrics {
    data: HashMap<String, String>,
}

impl Metrics {
    pub fn new(nodes: &[MiniYamlNode]) -> Metrics {
        let mut data = HashMap::new();
        for m in nodes {
            for n in &m.value.nodes {
                data.insert(n.key.clone(), MiniYaml::value_str(&n.value).to_string());
            }
        }
        Metrics { data }
    }

    pub fn string(&self, key: &str) -> String {
        self.data.get(key).cloned().unwrap_or_default()
    }

    pub fn int(&self, key: &str) -> i32 {
        self.data.get(key).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    pub fn bool(&self, key: &str) -> bool {
        self.data.get(key).is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    /// `ChromeMetrics.Get<int2>`.
    pub fn int2(&self, key: &str) -> (i32, i32) {
        let values = int_list(&self.string(key));
        (values.first().copied().unwrap_or(0), values.get(1).copied().unwrap_or(0))
    }

    pub fn color(&self, key: &str) -> Color {
        self.data
            .get(key)
            .and_then(|v| Color::parse(v))
            .unwrap_or_else(|| Color::parse("FFFFFF").expect("white"))
    }
}
