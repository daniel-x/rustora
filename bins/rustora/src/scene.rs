// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Turns the world into sorted quads: terrain, resources, actor animations and shroud.
//! Coordinates are world pixels of the RA rectangular grid: 24 px per 1024 world units, height drawn upwards.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::world_types::{CPos, WAngle, WDist, WPos};
use rustora_engine::frozen::FrozenActorLayer;
use rustora_engine::player::PlayerRef;
use rustora_engine::render::{Renderable, render_actor};
use rustora_engine::world::World;
use rustora_mods_common::palettes::ShroudRendererInfo;
use rustora_mods_common::render::WithSpriteBody;
use rustora_mods_common::resources::ResourceLayer;
use rustora_mods_common::selection::mouse_bounds_info;
use rustora_mods_common::smudge::SmudgeLayer;
use rustora_mods_common::visibility::FrozenUnderFog;
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

const ALL_CORNERS: u8 = 0x0f;
const ALL_SIDES: u8 = 0xf0;

/// The shroud variant of a cell. `ShroudRenderer` draws it with `Game.CosmeticRandom` once per map load; a hash of
/// the cell keeps it stable without per-map state.
fn cell_variant(x: i32, y: i32, count: usize) -> usize {
    let h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    (h ^ (h >> 15)) as usize % count
}

/// `ShroudRenderer.GetEdges` for the neighbors of a cell, where `set` tells which cells do not need an edge.
fn neighbor_edges(x: i32, y: i32, set: &impl Fn(i32, i32) -> bool, extended: bool) -> u8 {
    const TL: u8 = 0x01;
    const TR: u8 = 0x02;
    const BR: u8 = 0x04;
    const BL: u8 = 0x08;
    const TOP: u8 = 0x10 | TL | TR;
    const RIGHT: u8 = 0x20 | TR | BR;
    const BOTTOM: u8 = 0x40 | BR | BL;
    const LEFT: u8 = 0x80 | TL | BL;
    let mut edges = 0;
    for (dx, dy, e) in [(0, -1, TOP), (1, 0, RIGHT), (0, 1, BOTTOM), (-1, 0, LEFT)] {
        if !set(x + dx, y + dy) {
            edges |= e;
        }
    }
    let ucorner = edges & ALL_CORNERS;
    for (dx, dy, e) in [(-1, -1, TL), (1, -1, TR), (1, 1, BR), (-1, 1, BL)] {
        if !set(x + dx, y + dy) {
            edges |= e;
        }
    }
    if extended { edges ^ ucorner } else { edges & ALL_CORNERS }
}

/// The quad of a sprite centered on a cell.
fn cell_quad(s: &Sprite, x: i32, y: i32, row: u32) -> Quad {
    Quad::sprite(
        s.rect,
        x as f32 * TILE_SIZE + TILE_SIZE / 2.0 + s.offset[0] - s.rect.w as f32 / 2.0,
        y as f32 * TILE_SIZE + TILE_SIZE / 2.0 + s.offset[1] - s.rect.h as f32 / 2.0,
        row,
    )
}

fn on_camera(q: &Quad, cam: &Camera) -> bool {
    let (w, h) = (cam.width as f32 / cam.zoom, cam.height as f32 / cam.zoom);
    q.x < cam.x + w && q.x + q.w > cam.x && q.y < cam.y + h && q.y + q.h > cam.y
}

/// `TerrainSpriteLayer.UpdateTint`: the lighting at the four corners of the cell under the center of a terrain quad,
/// so the tint varies linearly across the cell.
fn cell_corner_tints(l: &dyn rustora_engine::render::ITerrainLighting, q: &Quad) -> [[f32; 3]; 4] {
    let cx = ((q.x + q.w / 2.0) / TILE_SIZE).floor() as i32;
    let cy = ((q.y + q.h / 2.0) / TILE_SIZE).floor() as i32;
    let center = WPos::new(cx * 1024 + 512, cy * 1024 + 512, 0);
    let at = |dx: i32, dy: i32| l.tint_at(center + rustora_core::world_types::WVec::new(dx, dy, 0));
    [at(-512, -512), at(512, -512), at(512, 512), at(-512, 512)]
}

pub struct Scene {
    sprites: SpriteCache,
    terrain: HashMap<u16, Rc<Vec<Option<Sprite>>>>,
    palettes: Palettes,
    variant_seed: u32,
    /// The number of quads of the last frame after the actors and effects: the `AfterActors` passes.
    pub after_actors: usize,
    /// The number of quads of the last frame after the `IRenderAboveWorld` traits: the `AfterWorld` passes.
    pub after_world: usize,
    /// The smallest zoom of the viewport, for `INotifyViewportZoomExtentsChanged` traits.
    pub min_zoom: f32,
    started: std::time::Instant,
    /// The `ITerrainLighting` of the world while a frame is built.
    lighting: Option<Rc<dyn rustora_engine::render::ITerrainLighting>>,
}

/// The `IRenderPostProcessPass` traits of the world actor as render passes, in the order of
/// `WorldRenderer.Draw`: `AfterActors` at `after_actors`, `AfterWorld` at `after_world`, `AfterShroud` at `end`.
pub fn post_passes(world: &World, after_actors: usize, after_world: usize, end: usize) -> Vec<rustora_render::post::PostPass> {
    use rustora_mods_common::post_effects::*;
    use rustora_render::post::{PostEffect, PostPass};
    let wa = world.world_actor();
    let mut passes = Vec::new();
    for t in wa.traits_implementing::<TintPostProcessEffect>() {
        passes.push(PostPass {
            at: after_actors,
            effect: PostEffect::Tint(t.tint()),
        });
    }
    for t in wa.traits_implementing::<ChronoshiftPostProcessEffect>() {
        if let Some(blend) = t.blend() {
            passes.push(PostPass {
                at: after_world,
                effect: PostEffect::Chronoshift { blend },
            });
        }
    }
    for t in wa.traits_implementing::<FlashPostProcessEffect>() {
        if t.enabled() {
            let c = t.info.color;
            let color = [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
            passes.push(PostPass {
                at: after_world,
                effect: PostEffect::Flash { blend: t.blend(), color },
            });
        }
    }
    for t in wa.traits_implementing::<MenuPostProcessEffect>() {
        if let Some((from, to, blend)) = t.state() {
            passes.push(PostPass {
                at: end,
                effect: PostEffect::MenuFade {
                    from: from as u32,
                    to: to as u32,
                    blend,
                },
            });
        }
    }
    passes
}

impl Scene {
    pub fn new(renderer: &mut Renderer, world: &World, read_file: rustora_engine::sequence::ReadFile) -> Scene {
        Scene {
            palettes: Palettes::new(renderer, world, &read_file),
            sprites: SpriteCache::new(read_file, &world.map().tileset),
            terrain: HashMap::new(),
            variant_seed: 0x9e37_79b9,
            after_actors: 0,
            after_world: 0,
            min_zoom: 1.0,
            started: std::time::Instant::now(),
            lighting: None,
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
    fn add_renderables(&mut self, renderer: &mut Renderer, world: &World, renderables: &[Renderable], cam: &Camera, quads: &mut Vec<Quad>) {
        let mut keyed: Vec<(i32, Quad)> = Vec::with_capacity(renderables.len());
        for r in renderables {
            let key = r.sort_key();
            self.add_renderable(renderer, world, r, key, cam, &mut keyed);
        }
        keyed.sort_by_key(|(k, _)| *k);
        quads.extend(keyed.into_iter().map(|(_, q)| q));
    }

    fn add_renderable(
        &mut self,
        renderer: &mut Renderer,
        world: &World,
        r: &Renderable,
        key: i32,
        cam: &Camera,
        out: &mut Vec<(i32, Quad)>,
    ) {
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
                if let Some(l) = &self.lighting
                    && !s.sequence.ignore_world_tint()
                {
                    quad.tint = [l.tint_at(s.pos); 4];
                }
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
            Renderable::Tile { tile, pos, .. } => {
                let frames = self.template_frames(renderer, world, tile.ty);
                let Some(s) = frames.get(tile.index as usize).copied().flatten() else {
                    return;
                };
                let (x, y) = world_to_px(*pos);
                let quad = Quad::sprite(
                    s.rect,
                    x + s.offset[0] - s.rect.w as f32 / 2.0,
                    y + s.offset[1] - s.rect.h as f32 / 2.0,
                    self.palettes.named("terrain"),
                );
                if on_camera(&quad, cam) {
                    out.push((key, quad));
                }
            }
            Renderable::Group { items, .. } => {
                for item in items {
                    self.add_renderable(renderer, world, item, key, cam, out);
                }
            }
        }
    }

    /// Builds the quads of one frame as seen by `viewer` (everything if `None`).
    /// `overlay` holds the unsorted renderables of the order generator, drawn last.
    pub fn build(
        &mut self,
        renderer: &mut Renderer,
        world: &World,
        viewer: Option<&PlayerRef>,
        cam: &Camera,
        overlay: &[Renderable],
    ) -> Vec<Quad> {
        self.palettes.update(renderer, world);
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
        self.add_smudges(renderer, world, viewer, &cells, &mut quads);
        let lighting = world
            .world_actor()
            .trait_or_default::<dyn rustora_engine::render::ITerrainLighting>();
        if let Some(l) = &lighting {
            for q in &mut quads {
                q.tint = cell_corner_tints(l.as_ref(), q);
            }
        }
        self.lighting = lighting;

        let mut renderables = Vec::new();
        for a in world.actors() {
            if !a.is_in_world() {
                continue;
            }
            if let Some(f) = a.trait_or_default::<FrozenUnderFog>() {
                f.tick_render(&a);
            }
            if viewer.is_none_or(|v| a.can_be_viewed_by_player(Some(v))) {
                render_actor(&a, &mut renderables);
            }
        }
        if let Some(v) = viewer
            && let Some(layer) = v.player_actor().trait_or_default::<FrozenActorLayer>()
            && let (Some(&first), Some(&last)) = (cells.first(), cells.last())
        {
            for f in layer.frozen_actors_in_region(first, last, true) {
                if !f.shrouded() {
                    renderables.extend(f.renderables.borrow().iter().cloned());
                }
            }
        }
        for e in world.effects() {
            e.render(viewer, &mut renderables);
        }
        self.add_renderables(renderer, world, &renderables, cam, &mut quads);
        self.after_actors = quads.len();
        self.add_above_world(world, cam, &mut quads);
        self.after_world = quads.len();

        self.add_shroud(renderer, world, viewer, cam, &mut quads);
        let mut above = Vec::new();
        for e in world.effects() {
            e.render_above_shroud(viewer, &mut above);
        }
        self.add_renderables(renderer, world, &above, cam, &mut quads);
        let mut unsorted = Vec::new();
        for r in overlay {
            self.add_renderable(renderer, world, r, 0, cam, &mut unsorted);
        }
        quads.extend(unsorted.into_iter().map(|(_, q)| q));
        quads
    }

    /// `IRenderAboveWorld` of the actors, drawn without sorting.
    fn add_above_world(&self, world: &World, cam: &Camera, quads: &mut Vec<Quad>) {
        use rustora_engine::render::{IRenderAboveWorld, WorldPixelShape, WorldRenderView};
        let view = WorldRenderView {
            center: (
                (cam.x + cam.width as f32 / cam.zoom / 2.0) as i32,
                (cam.y + cam.height as f32 / cam.zoom / 2.0) as i32,
            ),
            min_zoom: self.min_zoom,
            native_resolution: (cam.width, cam.height),
            run_time: self.started.elapsed().as_millis() as i64,
        };
        let mut shapes = Vec::new();
        for (a, t) in world.actors_with_trait::<dyn IRenderAboveWorld>() {
            t.render_above_world(&a, &view, &mut shapes);
        }
        quads.extend(shapes.into_iter().map(|s| match s {
            WorldPixelShape::Rect { x, y, w, h, color } => Quad::solid(x, y, w, h, color),
            WorldPixelShape::Line { from, to, width, color } => Quad::line(from, to, width, color, color),
        }));
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

    /// The `MouseBounds` of the visible frozen actors of `viewer`, from their interactable bounds at the frozen
    /// position, or the auto bounds of the actor while it exists.
    pub fn frozen_mouse_bounds(
        &mut self,
        renderer: &mut Renderer,
        world: &World,
        viewer: Option<&PlayerRef>,
    ) -> Vec<rustora_ui::screen_map::FrozenActorBounds> {
        let Some(layer) = viewer.and_then(|v| v.player_actor().trait_or_default::<FrozenActorLayer>()) else {
            return Vec::new();
        };
        let (w, h) = world.map().map_size;
        let mut out = Vec::new();
        for frozen in layer.frozen_actors_in_region(CPos::new(0, 0), CPos::new(w, h), true) {
            let Some(info) = mouse_bounds_info(frozen.info()) else {
                continue;
            };
            let center = screen_px_position(frozen.center_position());
            let bounds = match (info.bounds.as_ref(), frozen.actor()) {
                (Some(b), _) if b.len() >= 2 => Some(bounds_rect(center, b)),
                (_, Some(a)) => self.auto_bounds(renderer, &a, center),
                _ => None,
            };
            if let Some(bounds) = bounds.filter(|b| b.w > 0 || b.h > 0) {
                out.push(rustora_ui::screen_map::FrozenActorBounds { frozen, bounds });
            }
        }
        out
    }

    /// `Interactable.AutoBounds`: the first non-empty `WithSpriteBody.AutoMouseoverBounds`, the bounds of the default
    /// animation's current sequence. Actors like the V2 launcher have several sprite bodies.
    fn auto_bounds(&mut self, renderer: &mut Renderer, a: &rustora_engine::actor::ActorRef, center: (i32, i32)) -> Option<Rect> {
        a.traits_implementing::<WithSpriteBody>().iter().find_map(|body| {
            let seq = body.default_animation.current_sequence()?;
            let resolved = self.sprites.sequence(renderer, &seq)?;
            let [l, t, r, b] = resolved.bounds;
            Some(Rect::new(center.0 + l, center.1 + t, r - l, b - t)).filter(|r| r.w > 0 || r.h > 0)
        })
    }

    /// `SmudgeLayer.IRenderOverlay`.
    fn add_smudges(&mut self, renderer: &mut Renderer, world: &World, viewer: Option<&PlayerRef>, cells: &[CPos], quads: &mut Vec<Quad>) {
        let seqs = world.map().sequences().clone();
        for layer in world.world_actor().traits_implementing::<SmudgeLayer>() {
            layer.tick_render(viewer);
            if layer.is_empty() {
                continue;
            }
            let row = self.palettes.named(&layer.info.palette);
            for &c in cells {
                let Some(smudge) = layer.at(c) else { continue };
                let seq = seqs.get_sequence(&layer.info.sequence, &smudge.kind);
                let Some(resolved) = self.sprites.sequence(renderer, &seq) else {
                    continue;
                };
                if let Some(s) = resolved.sprite(smudge.depth, WAngle::ZERO) {
                    quads.push(cell_quad(&s, c.x(), c.y(), row));
                }
            }
        }
    }

    /// `ShroudRenderer.RenderShroud`, evaluated per frame for the cells on the camera, including the first row outside
    /// the map bounds. Without a viewer all cells inside the map bounds are visible and the others hidden.
    fn add_shroud(&mut self, renderer: &mut Renderer, world: &World, viewer: Option<&PlayerRef>, cam: &Camera, quads: &mut Vec<Quad>) {
        let Some(info) = world.world_actor().info.trait_info::<ShroudRendererInfo>() else {
            return;
        };
        let seqs = world.map().sequences().clone();
        let mut variants = Vec::new();
        for (shroud_variant, fog_variant) in info.shroud_variants.iter().zip(&info.fog_variants) {
            let shroud_seq = seqs.get_sequence(&info.sequence, shroud_variant);
            let fog_seq = seqs.get_sequence(&info.sequence, fog_variant);
            let (Some(shroud_sprites), Some(fog_sprites)) = (
                self.sprites.sequence(renderer, &shroud_seq),
                self.sprites.sequence(renderer, &fog_seq),
            ) else {
                return;
            };
            let full = match (&info.override_full_shroud, &info.override_full_fog) {
                (Some(shroud_full), Some(fog_full)) => {
                    let shroud_full = seqs.get_sequence(&info.sequence, shroud_full);
                    let fog_full = seqs.get_sequence(&info.sequence, fog_full);
                    match (
                        self.sprites.sequence(renderer, &shroud_full),
                        self.sprites.sequence(renderer, &fog_full),
                    ) {
                        (Some(a), Some(b)) => Some((a, b)),
                        _ => return,
                    }
                }
                _ => None,
            };
            variants.push((shroud_sprites, fog_sprites, full));
        }
        if variants.is_empty() {
            return;
        }
        let (fog_row, shroud_row) = (self.palettes.named(&info.fog_palette), self.palettes.named(&info.shroud_palette));
        let extended = info.use_extended_index;
        let not_visible = if extended { ALL_SIDES } else { ALL_CORNERS };
        let mut index_of = vec![0u8; 256];
        for (i, &e) in info.index.iter().enumerate() {
            index_of[e as usize] = i as u8;
        }
        let override_index = info.override_full_shroud.as_ref().map(|_| info.override_shroud_index as usize);
        let shroud = viewer.map(|v| v.shroud());
        let map = world.map();
        let contains = |x: i32, y: i32| {
            let c = CPos::new(x, y);
            map.contains(c).then_some(c)
        };
        let visible = |x: i32, y: i32| contains(x, y).is_some_and(|c| shroud.as_ref().is_none_or(|s| s.is_visible(c)));
        let explored = |x: i32, y: i32| contains(x, y).is_some_and(|c| shroud.as_ref().is_none_or(|s| s.is_explored(c)));
        let ts = TILE_SIZE;
        let (x0, y0) = ((cam.x / ts).floor() as i32 - 1, (cam.y / ts).floor() as i32 - 1);
        let x1 = ((cam.x + cam.width as f32 / cam.zoom) / ts).ceil() as i32 + 1;
        let y1 = ((cam.y + cam.height as f32 / cam.zoom) / ts).ceil() as i32 + 1;
        let b = map.bounds;
        let mut fog_quads = Vec::new();
        let mut shroud_quads = Vec::new();
        for y in y0.max(b.y - 1)..y1.min(b.y + b.height + 1) {
            for x in x0.max(b.x - 1)..x1.min(b.x + b.width + 1) {
                let (edges_shroud, edges_fog) = if !explored(x, y) {
                    (not_visible, not_visible)
                } else {
                    let fog = if visible(x, y) {
                        neighbor_edges(x, y, &visible, extended)
                    } else {
                        not_visible
                    };
                    (neighbor_edges(x, y, &explored, extended), fog)
                };
                let (shroud_sprites, fog_sprites, full) = &variants[cell_variant(x, y, variants.len())];
                for (edges, sprites, full, row, out) in [
                    (edges_fog, fog_sprites, full.as_ref().map(|f| &f.1), fog_row, &mut fog_quads),
                    (
                        edges_shroud,
                        shroud_sprites,
                        full.as_ref().map(|f| &f.0),
                        shroud_row,
                        &mut shroud_quads,
                    ),
                ] {
                    if edges == 0 {
                        continue;
                    }
                    let sprite = match full {
                        Some(full) if override_index == Some(edges as usize) => full.sprite(0, WAngle::ZERO),
                        _ => sprites.sprite(index_of[edges as usize] as i32, WAngle::ZERO),
                    };
                    if let Some(s) = sprite {
                        out.push(cell_quad(&s, x, y, row));
                    }
                }
            }
        }
        quads.append(&mut fog_quads);
        quads.append(&mut shroud_quads);
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
            let name = names[cell_hash(c.bits as u32 ^ self.variant_seed) as usize % names.len()];
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

/// The murmur3 finalizer: mixes every bit of `x` into every bit of the result, so neighboring cells get unrelated
/// values.
fn cell_hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 13;
    x = x.wrapping_mul(0xc2b2_ae35);
    x ^ (x >> 16)
}
