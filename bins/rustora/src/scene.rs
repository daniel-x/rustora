// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Turns the world into sorted quads: terrain, resources, actor animations and shroud.
//! Coordinates are world pixels of the RA rectangular grid: 24 px per 1024 world units, height drawn upwards.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::world_types::{CPos, WAngle, WDist, WPos};
use rustora_engine::player::PlayerRef;
use rustora_engine::render::{Renderable, render_actor};
use rustora_engine::world::World;
use rustora_mods_common::render::WithSpriteBody;
use rustora_mods_common::resources::ResourceLayer;
use rustora_mods_common::selection::mouse_bounds_info;
use rustora_render::{Camera, Quad, Renderer};
use rustora_ui::draw::Rect;
use rustora_ui::screen_map::ActorBounds;

use crate::palettes::Palettes;
use crate::sprites::{Sprite, SpriteCache};

pub const TILE_SIZE: f32 = 24.0;

pub fn world_to_px(p: WPos) -> (f32, f32) {
    (p.x as f32 * TILE_SIZE / 1024.0, (p.y - p.z) as f32 * TILE_SIZE / 1024.0)
}

/// `WorldRenderer.ScreenPxPosition`.
pub fn screen_px_position(p: WPos) -> (i32, i32) {
    let (x, y) = world_to_px(p);
    (x.round_ties_even() as i32, y.round_ties_even() as i32)
}

/// The world pixel rectangle of `Interactable.Bounds` style bounds around `center`.
pub fn bounds_rect(center: (i32, i32), bounds: &[WDist]) -> Rect {
    let ts = TILE_SIZE as i32;
    let (w, h) = (bounds[0].length * ts / 1024, bounds[1].length * ts / 1024);
    let (mut ox, mut oy) = (-w / 2, -h / 2);
    if bounds.len() > 3 {
        ox += bounds[2].length * ts / 1024;
        oy += bounds[3].length * ts / 1024;
    }
    Rect::new(center.0 + ox, center.1 + oy, w, h)
}

fn on_camera(q: &Quad, cam: &Camera) -> bool {
    let (w, h) = (cam.width as f32 / cam.zoom, cam.height as f32 / cam.zoom);
    q.x < cam.x + w && q.x + q.w > cam.x && q.y < cam.y + h && q.y + q.h > cam.y
}

pub struct Scene {
    sprites: SpriteCache,
    terrain: HashMap<u16, Rc<Vec<Option<Sprite>>>>,
    palettes: Palettes,
    variant_seed: u32,
}

impl Scene {
    pub fn new(renderer: &mut Renderer, world: &World, read_file: rustora_engine::sequence::ReadFile) -> Scene {
        Scene {
            palettes: Palettes::new(renderer, world, &read_file),
            sprites: SpriteCache::new(read_file, &world.map().tileset),
            terrain: HashMap::new(),
            variant_seed: 0x9e37_79b9,
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

    /// Appends the quads of `renderables`, sorted as in `WorldRenderer.GenerateRenderables`, and culled to `cam`.
    fn add_renderables(&mut self, renderer: &mut Renderer, renderables: &[Renderable], cam: &Camera, quads: &mut Vec<Quad>) {
        let mut keyed: Vec<(i32, Quad)> = Vec::with_capacity(renderables.len());
        for r in renderables {
            let key = r.sort_key();
            self.add_renderable(renderer, r, key, cam, &mut keyed);
        }
        keyed.sort_by_key(|(k, _)| *k);
        quads.extend(keyed.into_iter().map(|(_, q)| q));
    }

    fn add_renderable(&mut self, renderer: &mut Renderer, r: &Renderable, key: i32, cam: &Camera, out: &mut Vec<(i32, Quad)>) {
        match r {
            Renderable::Sprite(s) => {
                let Some(resolved) = self.sprites.sequence(renderer, &s.sequence) else {
                    return;
                };
                let Some(sprite) = resolved.sprite(s.frame, s.facing) else {
                    return;
                };
                let (x, y) = world_to_px(s.pos + s.offset);
                let scale = resolved.scale;
                let (w, h) = (sprite.rect.w as f32 * scale, sprite.rect.h as f32 * scale);
                let mut quad = Quad::sprite(
                    sprite.rect,
                    x + sprite.offset[0] * scale - (w / 2.0).trunc(),
                    y + sprite.offset[1] * scale - (h / 2.0).trunc(),
                    self.palettes.row(&s.palette),
                );
                quad.w = w;
                quad.h = h;
                quad.color[3] = s.alpha;
                if let Some([r, g, b]) = s.replace_color {
                    quad.color = [r, g, b, s.alpha];
                    quad.replace_color = true;
                }
                if on_camera(&quad, cam) {
                    out.push((key + resolved.z_offset, quad));
                }
            }
            Renderable::Line {
                from,
                to,
                width,
                from_color,
                to_color,
            } => {
                let (a, b) = (world_to_px(*from), world_to_px(*to));
                let px_width = width * TILE_SIZE / 1024.0;
                let quad = Quad::line([a.0, a.1], [b.0, b.1], px_width, *from_color, *to_color);
                out.push((key, quad));
            }
            Renderable::Group { items, .. } => {
                for item in items {
                    self.add_renderable(renderer, item, key, cam, out);
                }
            }
        }
    }

    /// Builds the quads of one frame as seen by `viewer` (everything if `None`).
    pub fn build(&mut self, renderer: &mut Renderer, world: &World, viewer: Option<&PlayerRef>, cam: &Camera) -> Vec<Quad> {
        self.palettes.update(renderer, world.world_tick());
        let terrain_row = self.palettes.named("terrain");
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
                    terrain_row,
                ));
            }
        }
        self.add_resources(renderer, world, &cells, &mut quads);

        let mut renderables = Vec::new();
        for a in world.actors() {
            if a.is_in_world() && viewer.is_none_or(|v| a.can_be_viewed_by_player(Some(v))) {
                render_actor(&a, &mut renderables);
            }
        }
        for e in world.effects() {
            e.render(viewer, &mut renderables);
        }
        self.add_renderables(renderer, &renderables, cam, &mut quads);

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
        let mut above = Vec::new();
        for e in world.effects() {
            e.render_above_shroud(viewer, &mut above);
        }
        self.add_renderables(renderer, &above, cam, &mut quads);
        quads
    }

    /// `Actor.MouseBounds` of all actors in the world with `Interactable` or `Selectable`, the input of `ScreenMap`.
    /// Polygons are not supported, and auto bounds come from the default animation of `WithSpriteBody`.
    pub fn mouse_bounds(&mut self, renderer: &mut Renderer, world: &World) -> Vec<ActorBounds> {
        let mut out = Vec::new();
        for a in world.actors() {
            if !a.is_in_world() {
                continue;
            }
            let Some(info) = mouse_bounds_info(&a.info) else {
                continue;
            };
            let center = screen_px_position(a.center_position());
            let auto = self.auto_bounds(renderer, &a, center);
            let rect = |b: Option<&Vec<WDist>>| match b {
                Some(b) if b.len() >= 2 => Some(bounds_rect(center, b)),
                _ => auto,
            };
            let Some(bounds) = rect(info.bounds.as_ref()).filter(|b| b.w > 0 || b.h > 0) else {
                continue;
            };
            let decoration = rect(info.decoration_bounds.as_ref().or(info.bounds.as_ref())).unwrap_or_default();
            out.push(ActorBounds {
                actor: a,
                bounds,
                decoration,
            });
        }
        out
    }

    /// `WithSpriteBody.AutoMouseoverBounds`: the bounds of the default animation's current sequence.
    fn auto_bounds(&mut self, renderer: &mut Renderer, a: &rustora_engine::actor::ActorRef, center: (i32, i32)) -> Option<Rect> {
        let body = a.trait_or_default::<WithSpriteBody>()?;
        let seq = body.default_animation.current_sequence()?;
        let resolved = self.sprites.sequence(renderer, &seq)?;
        let [l, t, r, b] = resolved.bounds;
        Some(Rect::new(center.0 + l, center.1 + t, r - l, b - t))
    }

    /// `ResourceRenderer` of RA: the frame follows the density, the variant is a per-cell hash instead of
    /// `World.LocalRandom`.
    fn add_resources(&mut self, renderer: &mut Renderer, world: &World, cells: &[CPos], quads: &mut Vec<Quad>) {
        let Some(layer) = world.world_actor().trait_or_default::<ResourceLayer>() else {
            return;
        };
        let seqs = world.map().sequences().clone();
        let player_row = self.palettes.named("player");
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
                    player_row,
                ));
            }
        }
    }
}
