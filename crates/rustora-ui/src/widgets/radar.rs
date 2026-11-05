// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `RadarWidget.cs` for rectangular maps without height. The three layers are composed into one texture
//! on the CPU. Terrain colors come from the current map and resources instead of `PlayerRadarTerrain`, which keeps
//! the colors seen last under fog; the radar sounds are not ported.

use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::{CPos, MersenneTwister};
use rustora_mods_common::radar::{AppearsOnRadar, RadarPings};
use rustora_mods_common::resources::ResourceLayer;
use rustora_render::ui::UiTexture;

use super::impl_as_any;
use crate::chrome::Sprite;
use crate::draw::{DrawContext, Rect};
use crate::input::{MouseButton, MouseEvent, MouseInput};
use crate::logic::ingame::Ingame;
use crate::logic::services;
use crate::widget::{Getter, Ui, WidgetId, WidgetKind};

#[derive(Clone)]
pub struct RadarWidget {
    pub world_interaction_controller: Option<String>,
    pub animation_length: i32,
    pub sound_up: String,
    pub sound_down: String,
    pub is_enabled: Option<Getter<bool>>,
    /// `Animating`, with the fraction of the open animation.
    pub animating: Option<Rc<dyn Fn(f32)>>,
    ingame: Option<Rc<Ingame>>,
    /// Map bounds in cells: left, top, width, height.
    bounds: Rect,
    terrain: Vec<[u8; 4]>,
    pixels: Vec<u8>,
    dirty: bool,
    texture: Option<UiTexture>,
    frame: i32,
    has_radar: bool,
    minimap_height: f32,
    preview_scale: f32,
    preview_origin: (i32, i32),
    map_rect: Rect,
    cursor: Option<String>,
}

impl Default for RadarWidget {
    fn default() -> Self {
        RadarWidget {
            world_interaction_controller: None,
            animation_length: 5,
            sound_up: String::new(),
            sound_down: String::new(),
            is_enabled: None,
            animating: None,
            ingame: None,
            bounds: Rect::default(),
            terrain: Vec::new(),
            pixels: Vec::new(),
            dirty: true,
            texture: None,
            frame: 0,
            has_radar: false,
            minimap_height: 0.0,
            preview_scale: 0.0,
            preview_origin: (0, 0),
            map_rect: Rect::default(),
            cursor: None,
        }
    }
}

fn rgba(c: Color) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

/// `Exts.ColorLerp`.
fn color_lerp(t: f32, a: Color, b: Color) -> Color {
    let l = |x: u8, y: u8| (x as f32 + t * (y as f32 - x as f32)) as u8;
    Color::from_argb(l(a.a(), b.a()), l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

impl RadarWidget {
    /// The terrain colors of `Map.GetTerrainColorPair` with a cosmetic random of our own.
    fn compute_terrain(&mut self, ingame: &Ingame) {
        let map = ingame.world.map();
        let b = self.bounds;
        let mut random = MersenneTwister::new(0);
        self.terrain = Vec::with_capacity((b.w * b.h) as usize);
        for y in b.y..b.bottom() {
            for x in b.x..b.right() {
                let tile = *map.tiles.at(CPos::new(x, y));
                let color = map.terrain.tile_info(tile).map_or_else(
                    || map.terrain_type_info(CPos::new(x, y)).color,
                    |t| {
                        if t.min_color != t.max_color {
                            color_lerp(random.next_float(), t.min_color, t.max_color)
                        } else {
                            t.min_color
                        }
                    },
                );
                self.terrain.push(rgba(color));
            }
        }
    }

    /// The resource layer of `ResourceRenderer.TryGetTerrainColorPair` over the terrain.
    fn terrain_with_resources(&self, ingame: &Ingame) -> Vec<[u8; 4]> {
        let mut out = self.terrain.clone();
        let world = &ingame.world;
        let map = world.map();
        let Some(layer) = world.world_actor().trait_or_default::<ResourceLayer>() else {
            return out;
        };
        let b = self.bounds;
        for y in b.y..b.bottom() {
            for x in b.x..b.right() {
                let Some(ty) = layer.get_resource(CPos::new(x, y)).ty else {
                    continue;
                };
                let Some(info) = layer.info.resource_types.get(&*ty) else {
                    continue;
                };
                if let Some(t) = map.terrain.terrain_types.iter().find(|t| t.ty == info.terrain_type) {
                    out[((y - b.y) * b.w + (x - b.x)) as usize] = rgba(t.color);
                }
            }
        }
        out
    }

    /// Composes terrain, actors and shroud into `pixels`, once per tick like the actor layer in C#.
    fn compose(&mut self, ingame: &Ingame) {
        let b = self.bounds;
        let mut px = self.terrain_with_resources(ingame);
        let world = &ingame.world;
        let render_player = ingame.render_player();
        let viewer = render_player.as_ref().or(ingame.local_player.as_ref());
        if self.is_enabled.as_ref().is_none_or(|f| f()) {
            let mut cells = Vec::new();
            for (a, t) in world.actors_with_trait::<AppearsOnRadar>() {
                let obscured = render_player.is_some() && !a.can_be_viewed_by_player(render_player.as_ref());
                if !a.is_in_world() || obscured {
                    continue;
                }
                cells.clear();
                t.radar_signature_cells(&a, viewer, &mut cells);
                for (c, color) in &cells {
                    if b.contains(c.x(), c.y()) {
                        px[((c.y() - b.y) * b.w + (c.x() - b.x)) as usize] = rgba(*color);
                    }
                }
            }
        }
        if let Some(player) = &render_player {
            let shroud = player.shroud();
            for y in b.y..b.bottom() {
                for x in b.x..b.right() {
                    let c = CPos::new(x, y);
                    let p = &mut px[((y - b.y) * b.w + (x - b.x)) as usize];
                    if !shroud.is_explored(c) {
                        *p = [0, 0, 0, 255];
                    } else if !shroud.is_visible(c) {
                        *p = [p[0] / 2, p[1] / 2, p[2] / 2, 255];
                    }
                }
            }
        }
        self.pixels = px.into_iter().flatten().collect();
        self.dirty = true;
    }

    /// `DrawRadarPings`.
    fn draw_radar_pings(&self, ingame: &Ingame, dc: &mut DrawContext) {
        let Some(pings) = ingame.world.world_actor().trait_or_default::<RadarPings>() else {
            return;
        };
        let map = ingame.world.map();
        for ping in pings
            .pings
            .borrow()
            .iter()
            .filter(|p| p.is_visible(ingame.render_player().as_ref()))
        {
            let (x, y) = self.cell_to_minimap_pixel(map.cell_containing(ping.position));
            let points = ping.points((x as f32, y as f32));
            let color = crate::font::premultiplied(ping.color);
            for i in 0..points.len() {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                dc.list.line([a.0, a.1], [b.0, b.1], 2.0, color);
            }
        }
    }

    fn cell_to_minimap_pixel(&self, c: CPos) -> (i32, i32) {
        let dx = (self.preview_scale * (c.x() - self.bounds.x) as f32) as i32;
        let dy = (self.preview_scale * (c.y() - self.bounds.y) as f32) as i32;
        (self.map_rect.x + dx, self.map_rect.y + dy)
    }

    fn minimap_pixel_to_cell(&self, p: (i32, i32)) -> CPos {
        let u = ((p.0 - self.map_rect.x) as f32 / self.preview_scale) as i32 + self.bounds.x;
        let v = ((p.1 - self.map_rect.y) as f32 / self.preview_scale) as i32 + self.bounds.y;
        CPos::new(u, v)
    }

    fn cell_center_px(c: CPos) -> (i32, i32) {
        (c.x() * 24 + 12, c.y() * 24 + 12)
    }
}

impl WidgetKind for RadarWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "WorldInteractionController" => self.world_interaction_controller = Some(value.to_string()),
            "AnimationLength" => self.animation_length = value.trim().parse().unwrap_or(5),
            "SoundUp" => self.sound_up = value.to_string(),
            "SoundDown" => self.sound_down = value.to_string(),
            "RadarOnlineSound" | "RadarOfflineSound" => {}
            _ => return false,
        }
        true
    }

    fn initialize(&mut self, ui: &mut Ui, id: WidgetId) {
        self.ingame = services(ui).ingame.borrow().clone();
        let Some(ingame) = self.ingame.clone() else { return };
        let mb = ingame.world.map().bounds;
        self.bounds = Rect::new(mb.x, mb.y, mb.width, mb.height);
        let rb = ui.node(id).bounds;
        let b = self.bounds;
        self.preview_scale = (rb.w as f32 / b.w as f32).min(rb.h as f32 / b.h as f32);
        self.preview_origin = (
            ((rb.w as f32 - self.preview_scale * b.w as f32) / 2.0) as i32,
            ((rb.h as f32 - self.preview_scale * b.h as f32) / 2.0) as i32,
        );
        self.map_rect = Rect::new(
            self.preview_origin.0,
            self.preview_origin.1,
            (self.preview_scale * b.w as f32) as i32,
            (self.preview_scale * b.h as f32) as i32,
        );
        self.compute_terrain(&ingame);
        self.compose(&ingame);
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        let Some(ingame) = self.ingame.clone() else { return };
        let enabled = self.is_enabled.as_ref().is_none_or(|f| f());
        self.compose(&ingame);
        let target = if enabled { self.animation_length } else { 0 };
        self.has_radar = enabled && self.frame == self.animation_length;
        if self.frame == target {
            return;
        }
        self.frame += if enabled { 1 } else { -1 };
        self.minimap_height = self.frame as f32 / self.animation_length as f32;
        if let Some(f) = &self.animating {
            f(self.frame as f32 / self.animation_length as f32);
        }
        let (ox, oy) = ui.render_origin(id);
        self.map_rect = Rect::new(
            self.preview_origin.0 + ox,
            self.preview_origin.1 + oy,
            self.map_rect.w,
            self.map_rect.h,
        );
    }

    fn draw(&mut self, ui: &mut Ui, _id: WidgetId, dc: &mut DrawContext) {
        let Some(ingame) = self.ingame.clone() else { return };
        let (w, h) = (self.bounds.w as u32, self.bounds.h as u32);
        if w == 0 || h == 0 {
            return;
        }
        match self.texture {
            None => self.texture = Some(dc.renderer.add_ui_texture(w, h, &self.pixels, false)),
            Some(t) if self.dirty => dc.renderer.update_ui_texture(t, 0, 0, w, h, &self.pixels),
            _ => {}
        }
        self.dirty = false;
        let sprite = Sprite {
            texture: self.texture.expect("radar texture"),
            x: 0,
            y: 0,
            w: w as i32,
            h: h as i32,
        };
        let o = (
            self.map_rect.x as f32,
            self.map_rect.y as f32 + self.bounds.h as f32 * self.preview_scale * (1.0 - self.minimap_height) / 2.0,
        );
        dc.draw_sprite_sized(
            sprite,
            o.0,
            o.1,
            self.map_rect.w as f32,
            self.map_rect.h as f32 * self.minimap_height,
        );
        if self.has_radar {
            let viewport = ingame.viewport.borrow();
            let cell = |p: (i32, i32)| CPos::new(p.0.div_euclid(24), p.1.div_euclid(24));
            let tl = self.cell_to_minimap_pixel(cell(viewport.top_left()));
            let br = self.cell_to_minimap_pixel(cell(viewport.bottom_right()));
            dc.push_clip(self.map_rect);
            dc.draw_rect(tl, br, 1.0, Color::from_argb(255, 255, 255, 255));
            dc.pop_clip();
            self.draw_radar_pings(&ingame, dc);
        }
        self.cursor = None;
        let mouse = ui.last_mouse;
        if self.has_radar && self.map_rect.contains(mouse.0, mouse.1) {
            let settings = services(ui).settings.clone();
            let settings = settings.borrow();
            let c = self.minimap_pixel_to_cell(mouse);
            let world_px = Self::cell_center_px(c);
            let location = ingame.viewport.borrow().world_to_view_px(world_px);
            let generator = ingame.order_generator();
            let cursor = ingame.with_order_context(&settings, |ctx| {
                let mi = MouseInput {
                    event: MouseEvent::Move,
                    button: ctx.action_button(),
                    x: location.0,
                    y: location.1,
                    scroll_delta: 0,
                    modifiers: ui.modifiers,
                    multi_tap: 0,
                };
                generator.get_cursor(ctx, c, world_px, &mi)
            });
            self.cursor = Some(match cursor {
                None => ui.assets.borrow().metrics.string("WorldDefaultCursor"),
                Some(c) if ui.cursor_names.contains(&format!("{c}-minimap")) => format!("{c}-minimap"),
                Some(c) => c,
            });
        }
    }

    fn handle_mouse(&mut self, ui: &mut Ui, _id: WidgetId, mi: &MouseInput) -> bool {
        if !self.map_rect.contains(mi.x, mi.y) {
            return false;
        }
        if !self.has_radar {
            return true;
        }
        let Some(ingame) = self.ingame.clone() else { return true };
        let classic = services(ui).settings.borrow().game.use_classic_mouse_style;
        let (action, cancel) = if classic {
            (MouseButton::Left, MouseButton::Right)
        } else {
            (MouseButton::Right, MouseButton::Left)
        };
        let c = self.minimap_pixel_to_cell((mi.x, mi.y));
        let pos = Self::cell_center_px(c);
        if matches!(mi.event, MouseEvent::Down | MouseEvent::Move) && mi.button == cancel {
            ingame.viewport.borrow_mut().center_on(pos);
        }
        if mi.event == MouseEvent::Down && mi.button == action {
            let location = ingame.viewport.borrow().world_to_view_px(pos);
            let mut fake = MouseInput {
                event: MouseEvent::Down,
                button: action,
                x: location.0,
                y: location.1,
                scroll_delta: 0,
                modifiers: mi.modifiers,
                multi_tap: 1,
            };
            if let Some(controller) = self.world_interaction_controller.as_deref().and_then(|n| ui.find(ui.root, n)) {
                let kind = ui.kind(controller);
                kind.borrow_mut().handle_mouse(ui, controller, &fake);
                fake.event = MouseEvent::Up;
                kind.borrow_mut().handle_mouse(ui, controller, &fake);
            }
        }
        true
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        self.cursor.clone()
    }
}
