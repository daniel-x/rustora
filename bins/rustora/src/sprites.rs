// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Sprite loading for the renderer: decoded sprite files in GPU sheets, and the frame layout of sprite sequences.
//! Port of the renderer half of `DefaultSpriteSequence`, `ClassicSpriteSequence` and `TilesetSpecificSpriteSequence`.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::fieldloader::get_value;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_core::world_types::{WAngle, WDist};
use rustora_engine::sequence::{ReadFile, Sequence};
use rustora_render::{Renderer, SpriteRect};

/// A frame in a sheet with its offset from the frame center, in pixels.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub rect: SpriteRect,
    pub offset: [f32; 3],
}

/// Facing lookup of `ClassicSpriteSequence` for 32 facings (`Mods.Cnc/Util.cs` `SpriteRanges`).
const CLASSIC_SPRITE_RANGES: [i32; 32] = [
    20, 56, 88, 132, 156, 184, 212, 240, 268, 296, 324, 352, 384, 416, 452, 488, 532, 568, 604, 644, 668, 696, 724, 752, 780, 808, 836,
    864, 896, 928, 964, 1000,
];

/// `Util.IndexFacing`.
pub fn index_facing(facing: WAngle, facings: i32) -> i32 {
    let step = 1024 / facings;
    ((facing.angle + step / 2) & 1023) / step
}

fn classic_index_facing(facing: WAngle, facings: i32) -> i32 {
    if facings != 32 {
        return index_facing(facing, facings);
    }
    CLASSIC_SPRITE_RANGES.iter().position(|&r| facing.angle < r).map_or(0, |i| i as i32)
}

/// A sequence with its frames resolved to sprites.
pub struct ResolvedSequence {
    sprites: Vec<Option<Sprite>>,
    pub length: i32,
    pub facings: i32,
    classic_facings: bool,
    pub z_offset: i32,
}

impl ResolvedSequence {
    /// `DefaultSpriteSequence.GetSprite(frame, facing)`.
    pub fn sprite(&self, frame: i32, facing: WAngle) -> Option<Sprite> {
        let f = if self.classic_facings {
            classic_index_facing(facing, self.facings)
        } else {
            index_facing(facing, self.facings)
        };
        let i = f * self.length + frame.rem_euclid(self.length.max(1));
        self.sprites.get(i as usize).copied().flatten()
    }
}

fn node<'a>(data: &'a MiniYaml, key: &str) -> Option<&'a MiniYamlNode> {
    data.nodes.iter().find(|n| n.key == key)
}

fn node_d<'a>(data: &'a MiniYaml, defaults: &'a MiniYaml, key: &str) -> Option<&'a MiniYamlNode> {
    node(data, key).or_else(|| node(defaults, key))
}

fn value<'a>(data: &'a MiniYaml, defaults: &'a MiniYaml, key: &str) -> Option<&'a str> {
    node_d(data, defaults, key).and_then(|n| n.value.value.as_deref())
}

fn int(data: &MiniYaml, defaults: &MiniYaml, key: &str, fallback: i32) -> i32 {
    value(data, defaults, key)
        .and_then(|v| get_value::<i32>(key, Some(v)).ok())
        .unwrap_or(fallback)
}

fn ints(data: &MiniYaml, defaults: &MiniYaml, key: &str) -> Option<Vec<i32>> {
    value(data, defaults, key).and_then(|v| get_value::<Vec<i32>>(key, Some(v)).ok())
}

fn flag(data: &MiniYaml, defaults: &MiniYaml, key: &str) -> bool {
    value(data, defaults, key).is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
}

fn offset(data: &MiniYaml, defaults: &MiniYaml) -> [f32; 3] {
    let mut o = [0.0; 3];
    if let Some(v) = value(data, defaults, "Offset") {
        for (i, p) in v.split(',').take(3).enumerate() {
            o[i] = p.trim().parse().unwrap_or(0.0);
        }
    }
    o
}

/// `CalculateFrameIndices` without explicit frames.
fn frame_indices(start: i32, length: i32, stride: i32, facings: i32, transpose: bool, reverse_facings: bool) -> Vec<i32> {
    let mut out = Vec::new();
    for facing in 0..facings {
        let inner = if reverse_facings { (facings - facing) % facings } else { facing };
        for frame in 0..length {
            out.push(
                start
                    + if transpose {
                        frame * facings + inner
                    } else {
                        inner * stride + frame
                    },
            );
        }
    }
    out
}

/// One file of a sequence: its name, the frames to take (all if `None`), and an extra pixel offset.
struct Reservation {
    filename: String,
    frames: Option<Vec<i32>>,
    offset: [f32; 3],
}

/// Decoded sprite files in sheets, and resolved sequences.
pub struct SpriteCache {
    read_file: ReadFile,
    tileset: String,
    files: HashMap<String, Rc<Vec<Option<Sprite>>>>,
    sequences: HashMap<(String, String), Option<Rc<ResolvedSequence>>>,
}

impl SpriteCache {
    pub fn new(read_file: ReadFile, tileset: &str) -> SpriteCache {
        SpriteCache {
            read_file,
            tileset: tileset.to_string(),
            files: HashMap::new(),
            sequences: HashMap::new(),
        }
    }

    /// All frames of a sprite file; empty frames are `None`.
    pub fn file(&mut self, renderer: &mut Renderer, filename: &str) -> Rc<Vec<Option<Sprite>>> {
        if let Some(f) = self.files.get(filename) {
            return f.clone();
        }
        let frames = (self.read_file)(filename)
            .and_then(|d| rustora_formats::sprite::decode(&d))
            .unwrap_or_else(|| {
                eprintln!("cannot load sprite file {filename}");
                Vec::new()
            });
        let sprites: Vec<Option<Sprite>> = frames
            .iter()
            .map(|f| {
                (f.width > 0 && f.height > 0).then(|| Sprite {
                    rect: renderer.add_sprite(f.width as u32, f.height as u32, &f.data),
                    offset: [f.offset.0, f.offset.1, 0.0],
                })
            })
            .collect();
        let sprites = Rc::new(sprites);
        self.files.insert(filename.to_string(), sprites.clone());
        sprites
    }

    pub fn sequence(&mut self, renderer: &mut Renderer, seq: &Sequence) -> Option<Rc<ResolvedSequence>> {
        let key = (seq.image.clone(), seq.name.clone());
        if let Some(s) = self.sequences.get(&key) {
            return s.clone();
        }
        let resolved = self.resolve(renderer, seq).map(Rc::new);
        if resolved.is_none() {
            eprintln!("cannot resolve sequence {}.{}", seq.image, seq.name);
        }
        self.sequences.insert(key, resolved.clone());
        resolved
    }

    fn tileset_node<'a>(&self, data: &'a MiniYaml, defaults: &'a MiniYaml, key: &str) -> Option<&'a MiniYamlNode> {
        node_d(data, defaults, key).and_then(|n| node(&n.value, &self.tileset))
    }

    /// `ParseFilenames` / `ParseCombineFilenames` of `TilesetSpecificSpriteSequence`.
    fn reservations(&self, data: &MiniYaml, defaults: &MiniYaml) -> Option<Vec<Reservation>> {
        let base_offset = offset(data, defaults);
        if let Some(c) = node(data, "Combine") {
            let empty = MiniYaml::default();
            let mut out = Vec::new();
            for sub in &c.value.nodes {
                let d = &sub.value;
                let filename = self
                    .tileset_node(d, &empty, "TilesetFilenames")
                    .or_else(|| node(d, "Filename"))
                    .and_then(|n| n.value.value.clone())?;
                let mut frames = ints(d, &empty, "Frames");
                if frames.is_none() && value(d, &empty, "Length") != Some("*") {
                    let (s, l) = (int(d, &empty, "Start", 0), int(d, &empty, "Length", 1));
                    frames = Some((s..s + l).collect());
                }
                let o = offset(d, &empty);
                out.push(Reservation {
                    filename,
                    frames,
                    offset: [o[0] + base_offset[0], o[1] + base_offset[1], o[2] + base_offset[2]],
                });
            }
            return Some(out);
        }
        let pattern = self
            .tileset_node(data, defaults, "TilesetFilenamesPattern")
            .or_else(|| node_d(data, defaults, "FilenamePattern"));
        if let Some(p) = pattern.filter(|p| p.value.value.as_deref().is_some_and(|v| !v.is_empty())) {
            let pat = p.value.value.clone()?;
            let empty = MiniYaml::default();
            let (s, c) = (int(&p.value, &empty, "Start", 0), int(&p.value, &empty, "Count", 1));
            return Some(
                (s..s + c)
                    .map(|i| Reservation {
                        filename: pat.replace("{0}", &i.to_string()),
                        frames: Some(vec![0]),
                        offset: base_offset,
                    })
                    .collect(),
            );
        }
        let filename = self
            .tileset_node(data, defaults, "TilesetFilenames")
            .or_else(|| node_d(data, defaults, "Filename"))
            .and_then(|n| n.value.value.clone())?;
        Some(vec![Reservation {
            filename,
            frames: ints(data, defaults, "Frames"),
            offset: base_offset,
        }])
    }

    /// `DefaultSpriteSequence.ResolveSprites`.
    fn resolve(&mut self, renderer: &mut Renderer, seq: &Sequence) -> Option<ResolvedSequence> {
        let (data, defaults) = (&seq.data, &seq.defaults);
        let mut all = Vec::new();
        for r in self.reservations(data, defaults)? {
            let file = self.file(renderer, &r.filename);
            let pick = |s: &Option<Sprite>| {
                s.map(|s| Sprite {
                    rect: s.rect,
                    offset: [s.offset[0] + r.offset[0], s.offset[1] + r.offset[1], s.offset[2] + r.offset[2]],
                })
            };
            match &r.frames {
                Some(fs) => all.extend(fs.iter().map(|&f| file.get(f as usize).and_then(pick))),
                None => all.extend(file.iter().map(pick)),
            }
        }
        let start = seq.start;
        let length = if value(data, defaults, "Length") == Some("*") {
            all.len() as i32 - start
        } else {
            int(data, defaults, "Length", 1)
        };
        let stride = int(data, defaults, "Stride", length);
        let mut index = frame_indices(
            start,
            length,
            stride,
            seq.facings,
            flag(data, defaults, "Transpose"),
            seq.reverse_facings,
        );
        let mut length = length;
        if seq.reverses {
            let back: Vec<i32> = index.iter().skip(1).take((length - 2).max(0) as usize).rev().copied().collect();
            index.extend(back);
            length = 2 * length - 2;
        }
        let z_offset = value(data, defaults, "ZOffset")
            .and_then(|v| get_value::<WDist>("ZOffset", Some(v)).ok())
            .map_or(0, |d| d.length);
        Some(ResolvedSequence {
            sprites: index.iter().map(|&i| all.get(i as usize).copied().flatten()).collect(),
            length,
            facings: seq.facings,
            classic_facings: flag(data, defaults, "UseClassicFacings"),
            z_offset,
        })
    }
}
