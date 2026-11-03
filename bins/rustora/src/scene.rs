// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Turns the world into sorted quads: terrain, resources, actor animations and shroud.
//! Coordinates are world pixels of the RA rectangular grid: 24 px per 1024 world units, height drawn upwards.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::world_types::{CPos, WAngle, WPos};
use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;
use rustora_mods_common::render::RenderSprites;
use rustora_mods_common::resources::ResourceLayer;
use rustora_render::{Camera, Quad, Renderer};

use crate::sprites::{Sprite, SpriteCache};

pub const TILE_SIZE: f32 = 24.0;

/// Palette rows: terrain, neutral player palette, effects, then one remapped player palette per player.
const TERRAIN_PALETTE: u32 = 0;
const PLAYER_BASE_PALETTE: u32 = 1;
const EFFECT_PALETTE: u32 = 2;
const FIRST_PLAYER_PALETTE: u32 = 3;
const PLAYER_REMAP: [usize; 16] = [80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95];

pub fn world_to_px(p: WPos) -> (f32, f32) {
    (p.x as f32 * TILE_SIZE / 1024.0, (p.y - p.z) as f32 * TILE_SIZE / 1024.0)
}

pub struct Scene {
    sprites: SpriteCache,
    terrain: HashMap<u16, Rc<Vec<Option<Sprite>>>>,
    player_palettes: HashMap<u32, u32>,
    variant_seed: u32,
}

/// Loads `.pal` files with the transparent and shadow indices of RA's `rules/palettes.yaml`.
fn load_palette(read_file: &rustora_engine::sequence::ReadFile, name: &str, shadow: &[usize]) -> rustora_formats::palette::Palette {
    let d = read_file(name).unwrap_or_else(|| panic!("palette {name} not found"));
    rustora_formats::palette::load(&d, &[0], shadow).expect("palette")
}

impl Scene {
    pub fn new(renderer: &mut Renderer, world: &World, read_file: rustora_engine::sequence::ReadFile) -> Scene {
        let map = world.map();
        let terrain_pal = format!("{}.pal", map.tileset.to_lowercase());
        renderer.set_palette(TERRAIN_PALETTE, &load_palette(&read_file, &terrain_pal, &[3, 4]));
        let player = load_palette(&read_file, "temperat.pal", &[4]);
        renderer.set_palette(PLAYER_BASE_PALETTE, &player);
        renderer.set_palette(EFFECT_PALETTE, &player);
        let mut player_palettes = HashMap::new();
        for (i, p) in world.players().iter().enumerate() {
            let row = FIRST_PLAYER_PALETTE + i as u32;
            if row >= rustora_render::MAX_PALETTES {
                break;
            }
            let c = p.color.argb;
            let rgb = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
            renderer.set_palette(row, &rustora_formats::palette::player_remap(&player, &PLAYER_REMAP, rgb));
            player_palettes.insert(p.player_actor().id, row);
        }
        Scene {
            sprites: SpriteCache::new(read_file, &map.tileset),
            terrain: HashMap::new(),
            player_palettes,
            variant_seed: 0x9e37_79b9,
        }
    }

    fn palette_row(&self, name: Option<&str>, is_player_palette: bool, owner: &PlayerRef) -> u32 {
        if is_player_palette || name == Some("player") {
            return self
                .player_palettes
                .get(&owner.player_actor().id)
                .copied()
                .unwrap_or(PLAYER_BASE_PALETTE);
        }
        match name {
            Some("terrain") => TERRAIN_PALETTE,
            Some("player") | Some("player-noshadow") => PLAYER_BASE_PALETTE,
            _ => EFFECT_PALETTE,
        }
    }

    /// `DefaultTileCache`: the frames of a terrain template, after the optional `Frames` remap.
    fn template_frames(&mut self, renderer: &mut Renderer, world: &World, ty: u16) -> Rc<Vec<Option<Sprite>>> {
        if let Some(f) = self.terrain.get(&ty) {
            return f.clone();
        }
        let mut frames = Vec::new();
        if let Some(t) = world.map().terrain.templates.get(&ty) {
            let mut all = Vec::new();
            for image in &t.images {
                all.extend(self.sprites.file(renderer, image).iter().copied());
            }
            frames = match &t.frames {
                Some(fs) => fs.iter().map(|&f| all.get(f as usize).copied().flatten()).collect(),
                None => all,
            };
        }
        let frames = Rc::new(frames);
        self.terrain.insert(ty, frames.clone());
        frames
    }

    /// Cells overlapping the camera, clamped to the map bounds.
    fn visible_cells(world: &World, cam: &Camera) -> impl Iterator<Item = CPos> {
        let b = world.map().bounds;
        let ts = TILE_SIZE;
        let x0 = ((cam.x / ts).floor() as i32 - 1).max(b.x);
        let y0 = ((cam.y / ts).floor() as i32 - 1).max(b.y);
        let x1 = (((cam.x + cam.width as f32 / cam.zoom) / ts).ceil() as i32 + 1).min(b.x + b.width);
        let y1 = (((cam.y + cam.height as f32 / cam.zoom) / ts).ceil() as i32 + 3).min(b.y + b.height);
        (y0..y1).flat_map(move |y| (x0..x1).map(move |x| CPos::new(x, y)))
    }

    /// Builds the quads of one frame as seen by `viewer` (everything if `None`).
    pub fn build(&mut self, renderer: &mut Renderer, world: &World, viewer: Option<&PlayerRef>, cam: &Camera) -> Vec<Quad> {
        let mut quads = Vec::new();
        let cells: Vec<CPos> = Self::visible_cells(world, cam).collect();
        let map = world.map();
        for &c in &cells {
            let tile = *map.tiles.at(c);
            let frames = self.template_frames(renderer, world, tile.ty);
            if let Some(s) = frames.get(tile.index as usize).copied().flatten() {
                quads.push(Quad::sprite(
                    s.rect,
                    c.x() as f32 * TILE_SIZE + s.offset[0] - s.rect.w as f32 / 2.0 + TILE_SIZE / 2.0,
                    c.y() as f32 * TILE_SIZE + s.offset[1] - s.rect.h as f32 / 2.0 + TILE_SIZE / 2.0,
                    TERRAIN_PALETTE,
                ));
            }
        }
        self.add_resources(renderer, world, &cells, &mut quads);

        let mut actor_quads: Vec<(i32, Quad)> = Vec::new();
        for a in world.actors() {
            if !a.is_in_world() || viewer.is_some_and(|v| !a.can_be_viewed_by_player(Some(v))) {
                continue;
            }
            let Some(rs) = a.trait_or_default::<RenderSprites>() else {
                continue;
            };
            let owner = a.owner();
            for (anim, palette, is_player_palette) in rs.animations() {
                if anim.disable_func.as_ref().is_some_and(|f| f()) {
                    continue;
                }
                let Some(seq) = anim.animation.current_sequence() else { continue };
                let Some(resolved) = self.sprites.sequence(renderer, &seq) else {
                    continue;
                };
                let facing = (anim.animation.facing_func)();
                let Some(s) = resolved.sprite(anim.animation.current_frame(), facing) else {
                    continue;
                };
                let pos = a.center_position() + anim.offset_func.as_ref().map_or_else(Default::default, |f| f());
                let z = anim.zoffset_func.as_ref().map_or(0, |f| f(pos)) + resolved.z_offset;
                let (px, py) = world_to_px(pos);
                if std::env::var_os("RUSTORA_DUMP_ANIMS").is_some() {
                    eprintln!(
                        "{} {} {}.{} frame {} at {px:.0},{py:.0} palette {palette:?}",
                        a.id,
                        a.info.name,
                        seq.image,
                        seq.name,
                        anim.animation.current_frame()
                    );
                }
                let row = self.palette_row(palette.as_deref(), is_player_palette, &owner);
                actor_quads.push((
                    pos.y + pos.z + z,
                    Quad::sprite(
                        s.rect,
                        px + s.offset[0] - s.rect.w as f32 / 2.0,
                        py + s.offset[1] - s.rect.h as f32 / 2.0,
                        row,
                    ),
                ));
            }
        }
        actor_quads.sort_by_key(|(k, _)| *k);
        quads.extend(actor_quads.into_iter().map(|(_, q)| q));

        if let Some(v) = viewer {
            let shroud = v.shroud();
            for &c in &cells {
                let (x, y) = (c.x() as f32 * TILE_SIZE, c.y() as f32 * TILE_SIZE);
                if !shroud.is_explored(c) {
                    quads.push(Quad::solid(x, y, TILE_SIZE, TILE_SIZE, [0.0, 0.0, 0.0, 1.0]));
                } else if !shroud.is_visible(c) {
                    quads.push(Quad::solid(x, y, TILE_SIZE, TILE_SIZE, [0.0, 0.0, 0.0, 0.5]));
                }
            }
        }
        quads
    }

    /// `ResourceRenderer` of RA: the frame follows the density, the variant is a per-cell hash instead of
    /// `World.LocalRandom`.
    fn add_resources(&mut self, renderer: &mut Renderer, world: &World, cells: &[CPos], quads: &mut Vec<Quad>) {
        let Some(layer) = world.world_actor().trait_or_default::<ResourceLayer>() else {
            return;
        };
        let seqs = world.map().sequences().clone();
        for &c in cells {
            let r = layer.get_resource(c);
            let Some(ty) = r.ty.as_deref() else { continue };
            if r.density <= 0 {
                continue;
            }
            let names: &[&str] = match ty {
                "Ore" => &["gold01", "gold02", "gold03", "gold04"],
                "Gems" => &["gem01", "gem02", "gem03", "gem04"],
                _ => continue,
            };
            let h = (c.bits as u32).wrapping_mul(self.variant_seed) >> 16;
            let name = names[h as usize % names.len()];
            if !seqs.has_sequence("resources", name) {
                continue;
            }
            let Some(resolved) = self.sprites.sequence(renderer, &seqs.get_sequence("resources", name)) else {
                continue;
            };
            let max = layer.get_max_density(ty).max(1);
            let frame = (resolved.length - 1) * r.density / max;
            if let Some(s) = resolved.sprite(frame, WAngle::ZERO) {
                let (x, y) = (
                    c.x() as f32 * TILE_SIZE + TILE_SIZE / 2.0,
                    c.y() as f32 * TILE_SIZE + TILE_SIZE / 2.0,
                );
                quads.push(Quad::sprite(
                    s.rect,
                    x + s.offset[0] - s.rect.w as f32 / 2.0,
                    y + s.offset[1] - s.rect.h as f32 / 2.0,
                    PLAYER_BASE_PALETTE,
                ));
            }
        }
    }
}
