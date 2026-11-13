// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `PlaceBuildingOrderGenerator.cs` and the `IPlaceBuildingPreview` classes, without plugs and variants.

use std::cell::Cell;
use std::rc::Rc;

use rustora_core::world_types::{CPos, WVec};
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::order::Order;
use rustora_engine::render::{PaletteRef, Renderable, SpriteRenderable, render_animation};
use rustora_engine::sequence::{Animation, Sequence};
use rustora_engine::target::Target;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_mods_common::base::{
    LineBuildInfo, can_place_building, clear_blockers_orders, get_line_build_cells, is_cell_buildable, is_close_enough_to_base,
};
use rustora_mods_common::building::BuildingInfo;
use rustora_mods_common::previews::{
    ActorPreview, ActorPreviewPlaceBuildingPreviewInfo, FootprintPlaceBuildingPreviewInfo, SequencePlaceBuildingPreviewInfo,
    render_previews,
};
use rustora_mods_common::production::{BuildableInfo, PlaceBuildingInfo, ProductionQueue};
use rustora_mods_common::render::RenderSpritesInfo;
use rustora_mods_common::resources::ResourceLayer;

use crate::input::{MouseButton, MouseEvent, MouseInput};
use crate::orders::{OrderContext, OrderGenerator};

const TILE_SIZE: i32 = 24;
const FOOTPRINT_Z_OFFSET: i32 = -511;

#[derive(Clone, Copy)]
struct FootprintCell {
    cell: CPos,
    valid: bool,
    line_build: bool,
}

/// What is drawn between the footprint cells of a preview.
enum PreviewBody {
    Footprint,
    Sequence {
        animation: Rc<Animation>,
        palette: PaletteRef,
        alpha: f32,
    },
    Actor {
        previews: Vec<ActorPreview>,
        animated: bool,
        alpha: f32,
        z_offset: i32,
    },
}

/// `FootprintPlaceBuildingPreviewPreview` with its two subclasses.
struct PlacePreview {
    info: FootprintPlaceBuildingPreviewInfo,
    body: PreviewBody,
    center_offset: WVec,
    top_left_screen_offset: (i32, i32),
    valid_tile: Rc<Sequence>,
    blocked_tile: Rc<Sequence>,
}

impl PlacePreview {
    fn new(queue: &ProductionQueue, actor: &ActorInfo, building: &BuildingInfo) -> Option<PlacePreview> {
        let world = queue.actor.world();
        let owner = queue.actor.owner();
        let faction = actor
            .trait_info::<BuildableInfo>()
            .and_then(|b| b.force_faction.clone())
            .or_else(|| queue.most_likely_producer().map(|(_, p)| p.faction()))
            .unwrap_or_else(|| owner.faction.clone());
        let sequence_info = actor.trait_info::<SequencePlaceBuildingPreviewInfo>();
        let actor_info = actor.trait_info::<ActorPreviewPlaceBuildingPreviewInfo>();
        let footprint_info = actor.trait_info::<FootprintPlaceBuildingPreviewInfo>();
        let (info, body) = if let Some(i) = actor_info {
            let body = PreviewBody::Actor {
                previews: render_previews(&world, actor, &owner, &faction, true),
                animated: i.animated,
                alpha: i.preview_alpha,
                z_offset: i.z_offset,
            };
            (i.footprint.clone(), body)
        } else if let Some(i) = sequence_info {
            let rsi = actor.trait_info::<RenderSpritesInfo>()?;
            let animation = Animation::new(world.map().sequences().clone(), &rsi.get_image(actor, &faction), None, None);
            animation.play_repeating(&i.sequence);
            let palette_name = rsi.palette.as_deref().unwrap_or(&rsi.player_palette);
            let palette = PaletteRef::resolve(Some(palette_name), rsi.palette.is_none(), &owner);
            let body = PreviewBody::Sequence {
                animation,
                palette,
                alpha: i.sequence_alpha,
            };
            (i.footprint.clone(), body)
        } else {
            ((*footprint_info?).clone(), PreviewBody::Footprint)
        };
        let center_offset = building.center_offset(&world);
        let sequences = world.map().sequences().clone();
        let tileset_valid = format!("build-valid-{}", world.map().tileset.to_lowercase());
        let valid_name = if sequences.has_sequence("overlay", &tileset_valid) {
            tileset_valid.as_str()
        } else {
            "build-valid"
        };
        let blocked_tile = sequences.get_sequence("overlay", "build-invalid");
        Some(PlacePreview {
            info,
            body,
            center_offset,
            top_left_screen_offset: (-screen_px(center_offset.x), -screen_px(center_offset.y - center_offset.z)),
            valid_tile: sequences.get_sequence("overlay", valid_name),
            blocked_tile,
        })
    }

    fn tick(&self) {
        match &self.body {
            PreviewBody::Footprint => {}
            PreviewBody::Sequence { animation, .. } => animation.tick(),
            PreviewBody::Actor { previews, animated, .. } => {
                if *animated {
                    for p in previews {
                        p.tick();
                    }
                }
            }
        }
    }

    /// `RenderFootprint`.
    fn render_footprint(
        &self,
        ctx: &OrderContext,
        top_left: CPos,
        cells: &[FootprintCell],
        filter: impl Fn(&FootprintCell) -> bool,
        out: &mut Vec<Renderable>,
    ) {
        let map = ctx.world.map();
        let top_left_z = map.center_of_cell(top_left).z;
        let palette = PaletteRef::named(&self.info.palette);
        for c in cells.iter().filter(|c| filter(c)) {
            let pos = map.center_of_cell(c.cell);
            let alpha = if c.line_build {
                self.info.line_build_footprint_alpha
            } else {
                self.info.footprint_alpha
            };
            let sequence = if c.valid { &self.valid_tile } else { &self.blocked_tile };
            out.push(Renderable::Sprite(SpriteRenderable {
                sequence: sequence.clone(),
                frame: 0,
                facing: Default::default(),
                pos,
                offset: WVec::new(0, 0, top_left_z - pos.z),
                z_offset: FOOTPRINT_Z_OFFSET,
                palette: palette.clone(),
                alpha,
                replace_color: None,
                is_decoration: true,
            }));
        }
    }

    /// `Render`.
    fn render(&self, ctx: &OrderContext, top_left: CPos, cells: &[FootprintCell], out: &mut Vec<Renderable>) {
        let center = ctx.world.map().center_of_cell(top_left) + self.center_offset;
        match &self.body {
            PreviewBody::Footprint => self.render_footprint(ctx, top_left, cells, |_| true, out),
            PreviewBody::Sequence { animation, palette, alpha } => {
                self.render_footprint(ctx, top_left, cells, |c| c.valid || c.line_build, out);
                let mut body = Vec::new();
                render_animation(animation, center, WVec::ZERO, 0, palette, &mut body);
                out.extend(body.into_iter().map(|r| with_alpha(r, *alpha)));
                self.render_footprint(ctx, top_left, cells, |c| !c.valid, out);
            }
            PreviewBody::Actor {
                previews, alpha, z_offset, ..
            } => {
                self.render_footprint(ctx, top_left, cells, |c| c.valid || c.line_build, out);
                let mut body = Vec::new();
                for p in previews {
                    p.render(center, &mut body);
                }
                body.sort_by_key(Renderable::sort_key);
                out.extend(body.into_iter().map(|r| {
                    let r = if *alpha < 1.0 { with_alpha(r, *alpha) } else { r };
                    if *z_offset != 0 { with_z_offset(r, *z_offset) } else { r }
                }));
                self.render_footprint(ctx, top_left, cells, |c| !c.valid, out);
            }
        }
    }
}

/// One component of `WorldRenderer.ScreenPxOffset`: rounded to the nearest pixel, ties to even like `Math.Round`.
fn screen_px(w: i32) -> i32 {
    (TILE_SIZE as f32 * w as f32 / 1024.0).round_ties_even() as i32
}

fn with_alpha(r: Renderable, alpha: f32) -> Renderable {
    match r {
        Renderable::Sprite(mut s) => {
            s.alpha *= alpha;
            Renderable::Sprite(s)
        }
        other => other,
    }
}

fn with_z_offset(r: Renderable, z_offset: i32) -> Renderable {
    match r {
        Renderable::Sprite(mut s) => {
            s.z_offset = z_offset;
            Renderable::Sprite(s)
        }
        other => other,
    }
}

pub struct PlaceBuildingOrderGenerator {
    queue: Rc<ProductionQueue>,
    actor: Rc<ActorInfo>,
    building: Rc<BuildingInfo>,
    shift: Cell<bool>,
    preview: Option<PlacePreview>,
}

impl PlaceBuildingOrderGenerator {
    pub fn new(queue: Rc<ProductionQueue>, name: &str) -> PlaceBuildingOrderGenerator {
        let world = queue.actor.world();
        let actor = world.rules().actor(name).expect("building actor");
        let building = actor.trait_info::<BuildingInfo>().expect("Building");
        let preview = PlacePreview::new(&queue, &actor, &building);
        PlaceBuildingOrderGenerator {
            queue,
            actor,
            building,
            shift: Cell::new(false),
            preview,
        }
    }

    /// `TopLeft`: the cell under `Viewport.LastMousePos` moved by `TopLeftScreenOffset`.
    fn top_left(&self, ctx: &OrderContext) -> CPos {
        let (mx, my) = ctx.view.last_mouse;
        let (dx, dy) = self.preview.as_ref().map_or((0, 0), |p| p.top_left_screen_offset);
        let (x, y) = ctx.view.view_to_world_px((mx + dx, my + dy));
        CPos::new(x.div_euclid(TILE_SIZE), y.div_euclid(TILE_SIZE))
    }

    /// `InnerOrder`.
    fn inner_order(&self, ctx: &OrderContext, mi: &MouseInput) -> Vec<Order> {
        let world = ctx.world;
        if world.paused() || mi.button != MouseButton::Left {
            return Vec::new();
        }
        let owner = self.queue.actor.owner();
        let top_left = self.top_left(ctx);
        if !can_place_building(world, top_left, &self.building, None)
            || !is_close_enough_to_base(&self.building, world, &owner, &self.actor, top_left)
        {
            let place = owner.player_actor().info.trait_info::<PlaceBuildingInfo>();
            let notification = place.as_ref().and_then(|p| p.cannot_place_notification.as_deref());
            let text = place.as_ref().and_then(|p| p.cannot_place_text_notification.as_deref());
            rustora_engine::sound::notify_owner(owner.player_actor(), "Speech", notification, text);
            return clear_blockers_orders(world, &self.building.tiles(top_left), &owner, None);
        }
        let order_type = if self.actor.has_trait_info::<LineBuildInfo>() && !mi.modifiers.shift {
            "LineBuild"
        } else {
            "PlaceBuilding"
        };
        let mut o = Order::new(
            order_type,
            Some(owner.player_actor().clone()),
            Target::from_cell(world, top_left, FULL_CELL),
            false,
        );
        o.target_string = Some(self.actor.name.clone());
        o.extra_data = self.queue.actor.id;
        o.extra_location = CPos::new(0, 0);
        o.suppress_visual_feedback = true;
        vec![o]
    }
}

impl OrderGenerator for PlaceBuildingOrderGenerator {
    fn order(&self, ctx: &OrderContext, _cell: CPos, _world_px: (i32, i32), mi: &MouseInput) -> Vec<Order> {
        let left_down = mi.button == MouseButton::Left && mi.event == MouseEvent::Down;
        let right_up = mi.button == MouseButton::Right && mi.event == MouseEvent::Up;
        if !left_down && !right_up {
            return Vec::new();
        }
        if right_up {
            ctx.cancel_input_mode();
        }
        let orders = self.inner_order(ctx, mi);
        if orders
            .iter()
            .any(|o| matches!(o.order_string.as_str(), "PlaceBuilding" | "LineBuild" | "PlacePlug"))
        {
            ctx.cancel_input_mode();
        }
        orders
    }

    fn tick(&self, ctx: &OrderContext) {
        if self.queue.all_queued().iter().all(|i| !i.done.get() || i.item != self.actor.name) {
            ctx.cancel_input_mode();
        }
        if let Some(p) = &self.preview {
            p.tick();
        }
    }

    fn get_cursor(&self, ctx: &OrderContext, _cell: CPos, _world_px: (i32, i32), mi: &MouseInput) -> Option<String> {
        self.shift.set(mi.modifiers.shift);
        Some(ctx.world_default_cursor.clone())
    }

    /// `RenderAnnotations`: the circles of the placement decorations.
    fn draw(&self, ctx: &OrderContext, dc: &mut crate::draw::DrawContext, viewport: &crate::viewport::Viewport) {
        if self.preview.is_none() {
            return;
        }
        let center = ctx.world.map().center_of_cell(self.top_left(ctx)) + self.building.center_offset(ctx.world);
        let mut circles = Vec::new();
        rustora_mods_common::decorations::place_building_circles(ctx.world, &self.actor, center, ctx.render_player, &mut circles);
        for c in &circles {
            crate::annotations::draw_circle(dc, viewport, c);
        }
    }

    fn render_above_shroud(&self, ctx: &OrderContext, out: &mut Vec<Renderable>) {
        let Some(preview) = &self.preview else {
            return;
        };
        let world = ctx.world;
        let top_left = self.top_left(ctx);
        let owner = self.queue.actor.owner();
        let local = ctx.local_player.cloned().unwrap_or_else(|| owner.clone());
        let mut cells: Vec<FootprintCell> = Vec::new();
        let line_build = self.actor.has_trait_info::<LineBuildInfo>();
        if line_build && owner.shroud().is_explored(top_left) {
            if !self.shift.get() {
                let line_build_info = self.actor.trait_info::<LineBuildInfo>().expect("LineBuild");
                let segment = line_build_info
                    .segment_type
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map_or_else(|| self.actor.clone(), |s| world.rules().actor(s).expect("segment actor"));
                let segment_building = segment.trait_info::<BuildingInfo>().expect("Building");
                for (cell, _) in get_line_build_cells(world, top_left, &self.actor, &self.building, &owner) {
                    cells.push(FootprintCell {
                        cell,
                        valid: is_cell_buildable(world, cell, &segment_building, None)
                            && is_close_enough_to_base(&segment_building, world, &local, &segment, cell),
                        line_build: true,
                    });
                }
            }
            let valid = is_cell_buildable(world, top_left, &self.building, None)
                && is_close_enough_to_base(&self.building, world, &local, &self.actor, top_left);
            cells.retain(|c| c.cell != top_left);
            cells.push(FootprintCell {
                cell: top_left,
                valid,
                line_build: false,
            });
        } else {
            let resource_layer = world.world_actor().trait_or_default::<ResourceLayer>();
            let close_enough = is_close_enough_to_base(&self.building, world, &local, &self.actor, top_left);
            for cell in self.building.tiles(top_left) {
                cells.push(FootprintCell {
                    cell,
                    valid: close_enough
                        && is_cell_buildable(world, cell, &self.building, None)
                        && resource_layer.as_ref().is_none_or(|r| r.get_resource(cell).ty.is_none()),
                    line_build: false,
                });
            }
        }
        preview.render(ctx, top_left, &cells, out);
    }
}
