// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The selection annotations of `SelectionDecorationsBase.cs`: `SelectionBoxAnnotationRenderable` and the health
//! bar of `SelectionBarsAnnotationRenderable`. Extra bars and path debugging are not ported.

use std::collections::HashSet;

use rustora_client::settings::{Settings, StatusBarsType, TargetLinesType};
use rustora_core::fieldloader::Color;
use rustora_core::world_types::{WAngle, WPos, WRot, WVec};
use rustora_engine::actor::ActorRef;
use rustora_engine::damage::DamageState;
use rustora_engine::player::PlayerRef;
use rustora_engine::traits::{IHealth, ISelectionBar};
use rustora_engine::world::WorldType;
use rustora_mods_common::decorations::{AnnotationCircle, DecorationLabel, IDecoration, IRenderCircles};
use rustora_mods_common::production::RallyPointIndicator;
use rustora_mods_common::selection::SelectableInfo;
use rustora_mods_common::target_lines::{DrawLineToTarget, TargetLine};

use crate::draw::{DrawContext, Rect, TextStyle};
use crate::logic::ingame::Ingame;
use crate::screen_map::SelectionContext;
use crate::viewport::Viewport;

fn line(dc: &mut DrawContext, a: (f32, f32), b: (f32, f32), c: Color) {
    let (l, r) = (a.0.min(b.0), a.0.max(b.0));
    let (t, bt) = (a.1.min(b.1), a.1.max(b.1));
    if a.1 == b.1 {
        dc.list.solid([l, t - 0.5, r - l, 1.0], crate::font::premultiplied(c));
    } else {
        dc.list.solid([l - 0.5, t, 1.0, bt - t], crate::font::premultiplied(c));
    }
}

/// `DrawLineToTarget.RenderAnnotations` of the selected actors, with `INotifySelected.Selected`.
fn draw_target_lines(dc: &mut DrawContext, ingame: &Ingame, settings: &Settings, shift: bool) {
    let selected = ingame.selection.actors();
    let mut previous = ingame.target_line_selection.borrow_mut();
    let lines_enabled = settings.game.target_lines != TargetLinesType::Disabled;
    let automatic = settings.game.target_lines == TargetLinesType::Automatic;
    let force = shift || ingame.order_generator().is_force_modifiers();
    let viewport = ingame.viewport.borrow();
    let view = |p: WPos| {
        let world_px = (p.x * 24 / 1024, (p.y - p.z) * 24 / 1024);
        let (x, y) = viewport.world_to_view_px(world_px);
        (x as f32, y as f32)
    };
    let mut current = HashSet::with_capacity(selected.len());
    for a in &selected {
        current.insert(a.id);
        let Some(line) = a.trait_or_default::<DrawLineToTarget>() else {
            continue;
        };
        if automatic && !previous.contains(&a.id) {
            line.show(a);
        }
        let allied = a.owner().is_allied_with(ingame.world.sound.local_player().as_deref());
        if !lines_enabled || !allied || !(force || (automatic && line.within_lifetime())) {
            continue;
        }
        for l in line.lines(a) {
            draw_target_line(dc, &l, &view);
        }
    }
    *previous = current;
    if lines_enabled {
        for e in ingame.world.effects() {
            if let Some(indicator) = e.as_any().downcast_ref::<RallyPointIndicator>() {
                for l in indicator.target_lines() {
                    draw_target_line(dc, &l, &view);
                }
            }
        }
    }
}

/// `TargetLineRenderable.Render`: the line and a marker at both ends.
fn draw_target_line(dc: &mut DrawContext, l: &TargetLine, view: &impl Fn(WPos) -> (f32, f32)) {
    let (from, to) = (view(l.from), view(l.to));
    let color = crate::font::premultiplied(l.color);
    dc.list.line([from.0, from.1], [to.0, to.1], l.line_width as f32, color);
    for (x, y) in [to, from] {
        let m = l.marker_width as f32;
        dc.list.solid([x - m, y - m, 2.0 * m, 2.0 * m], color);
    }
}

/// `ISelectionDecorations.RenderSelectionAnnotations` with a given color: the selection box of `actor` around its
/// decoration bounds in the screen map.
pub(crate) fn draw_selection_box_of(
    dc: &mut DrawContext,
    viewport: &Viewport,
    screen_map: &crate::screen_map::ScreenMap,
    actor: &ActorRef,
    c: Color,
) {
    let Some(e) = screen_map.entries().iter().find(|e| e.actor.id == actor.id) else {
        return;
    };
    let b = e.decoration;
    let view = |x: i32, y: i32| {
        let (vx, vy) = viewport.world_to_view_px((x, y));
        (vx as f32, vy as f32)
    };
    selection_box(dc, view(b.x, b.y), view(b.right(), b.bottom()), c);
}

/// `SelectionBoxAnnotationRenderable.Render`.
fn selection_box(dc: &mut DrawContext, tl: (f32, f32), br: (f32, f32), c: Color) {
    let (tr, bl) = ((br.0, tl.1), (tl.0, br.1));
    for ((cx, cy), dx, dy) in [(tl, 4.0, 4.0), (tr, -4.0, 4.0), (br, -4.0, -4.0), (bl, 4.0, -4.0)] {
        line(dc, (cx, cy), (cx + dx, cy), c);
        line(dc, (cx, cy), (cx, cy + dy), c);
    }
}

fn half(c: Color) -> Color {
    Color::from_argb(255, c.r() / 2, c.g() / 2, c.b() / 2)
}

/// `DrawExtraBars`.
fn extra_bars(dc: &mut DrawContext, a: &ActorRef, viewer: Option<&PlayerRef>, start: (f32, f32), end: (f32, f32)) {
    let (mut start, mut end) = (start, end);
    let rows = [(-4.0, true), (-3.0, false), (-2.0, true)];
    for bar in a.traits_implementing::<dyn ISelectionBar>() {
        let value = bar.value(a, viewer);
        if value == 0.0 && !bar.display_when_empty() {
            continue;
        }
        start.1 += 4.0;
        end.1 += 4.0;
        let z = start.0 + (end.0 - start.0) * value;
        let color = bar.color();
        for (dy, outer) in rows {
            let background = Color::from_argb(
                128,
                if outer { 30 } else { 10 },
                if outer { 30 } else { 10 },
                if outer { 30 } else { 10 },
            );
            line(dc, (start.0, start.1 + dy), (end.0, end.1 + dy), background);
        }
        for (dy, outer) in rows {
            line(
                dc,
                (start.0, start.1 + dy),
                (z, end.1 + dy),
                if outer { half(color) } else { color },
            );
        }
    }
}

/// `DrawHealthBar`.
fn health_bar(dc: &mut DrawContext, health: &dyn IHealth, start: (f32, f32), end: (f32, f32)) {
    if health.is_dead() {
        return;
    }
    let c = Color::from_argb(128, 30, 30, 30);
    let c2 = Color::from_argb(128, 10, 10, 10);
    let state = health.damage_state();
    let health_color = if state == DamageState::CRITICAL {
        Color::from_argb(255, 255, 0, 0)
    } else if state == DamageState::HEAVY {
        Color::from_argb(255, 255, 255, 0)
    } else {
        Color::from_argb(255, 50, 205, 50)
    };
    let lerp = |v: f32| (start.0 + (end.0 - start.0) * v, start.1);
    let z = lerp(health.hp() as f32 / health.max_hp() as f32);
    let rows = [(-4.0, true), (-3.0, false), (-2.0, true)];
    for (dy, outer) in rows {
        line(dc, (start.0, start.1 + dy), (end.0, end.1 + dy), if outer { c } else { c2 });
    }
    for (dy, outer) in rows {
        line(
            dc,
            (start.0, start.1 + dy),
            (z.0, z.1 + dy),
            if outer { half(health_color) } else { health_color },
        );
    }
    if health.display_hp() != health.hp() {
        let delta = Color::from_argb(255, 255, 69, 0);
        let zz = lerp(health.display_hp() as f32 / health.max_hp() as f32);
        for (dy, outer) in rows {
            line(dc, (z.0, z.1 + dy), (zz.0, zz.1 + dy), if outer { half(delta) } else { delta });
        }
    }
}

const RANGE_CIRCLE_SEGMENTS: i32 = 32;

/// `RangeCircleAnnotationRenderable.DrawRangeCircle` and the sweep lines of `DetectionCircleAnnotationRenderable`.
pub(crate) fn draw_circle(dc: &mut DrawContext, viewport: &Viewport, c: &AnnotationCircle) {
    let view = |p: WPos| {
        let (x, y) = viewport.world_to_view_px((p.x * 24 / 1024, (p.y - p.z) * 24 / 1024));
        [x as f32, y as f32]
    };
    let at = |facing: i32| {
        let rot = WRot::from_yaw(WAngle::from_facing(facing));
        view(c.center + WVec::new(c.radius.length, 0, 0).rotate(&rot))
    };
    if let Some((count, separation, first)) = c.trail {
        let center = view(c.center);
        for i in 0..count {
            let step = if separation.angle <= 512 { 1 } else { -1 };
            let angle = first - WAngle::new(i * step);
            let end = view(c.center + WVec::new(angle.cos(), angle.sin(), 0) * c.radius.length / 1024);
            let alpha = c.color.a() as i32 - i * c.color.a() as i32 / count;
            let with_alpha = |col: Color| Color::from_argb(alpha as u8, col.r(), col.g(), col.b());
            dc.list
                .line(center, end, c.border_width, crate::font::premultiplied(with_alpha(c.border_color)));
            dc.list.line(center, end, c.width, crate::font::premultiplied(with_alpha(c.color)));
        }
    }
    for i in 0..RANGE_CIRCLE_SEGMENTS {
        let (a, b) = (at(8 * i), at(8 * i + 6));
        if c.border_width > 0.0 {
            dc.list.line(a, b, c.border_width, crate::font::premultiplied(c.border_color));
        }
        if c.width > 0.0 {
            dc.list.line(a, b, c.width, crate::font::premultiplied(c.color));
        }
    }
}

/// `SelectionDecorationsBase.DrawDecorations`, the `IDecoration` part.
fn draw_decorations(
    dc: &mut DrawContext,
    ingame: &Ingame,
    viewport: &Viewport,
    a: &rustora_engine::actor::ActorRef,
    b: Rect,
    selected: bool,
) {
    for d in a.traits_implementing::<dyn IDecoration>() {
        if d.requires_selection() && !selected {
            continue;
        }
        let Some(deco) = d.render_decoration(a, ingame.render_player().as_ref()) else {
            continue;
        };
        let (anchor, margin) = match deco.position.as_str() {
            "TopLeft" => ((b.x, b.y), (deco.margin.x, deco.margin.y)),
            "TopRight" => ((b.right(), b.y), (-deco.margin.x, deco.margin.y)),
            "BottomLeft" => ((b.x, b.bottom()), (deco.margin.x, -deco.margin.y)),
            "BottomRight" => ((b.right(), b.bottom()), (-deco.margin.x, -deco.margin.y)),
            "Top" => ((b.x + b.w / 2, b.y), (0, deco.margin.y)),
            _ => ((b.x + b.w / 2, b.y + b.h / 2), (0, 0)),
        };
        let (ox, oy) = viewport.world_to_view_px(anchor);
        let origin = (ox + margin.0, oy + margin.1);
        for s in &deco.sprites {
            let Some(f) = ingame.world_sprites.frame(dc.renderer, &s.image, &s.sequence, s.frame, &s.palette) else {
                continue;
            };
            let stride = if s.stride == Default::default() {
                (f.sprite.w, 0)
            } else {
                (s.stride.x, s.stride.y)
            };
            let pos = ((origin.0 + s.index * stride.0) as f32, (origin.1 + s.index * stride.1) as f32);
            dc.draw_frame_centered(f, pos);
        }
        if let Some(t) = &deco.text {
            let text = match &t.label {
                DecorationLabel::Text(text) => text.clone(),
                DecorationLabel::OwnerName { max_length } => {
                    let name = crate::logic::sidebar::resolve_player_name(&dc.assets.messages, &ingame.world, &a.owner());
                    name.chars().take(*max_length).collect()
                }
            };
            let (w, h) = dc.measure(&t.font, &text);
            dc.draw_text(&t.font, &text, origin.0 - w / 2, origin.1 - h / 2, t.color, TextStyle::Plain);
        }
    }
}

/// Draws the selection boxes and health bars of all actors in the screen map, in screen pixels.
pub fn draw_selection_annotations(dc: &mut DrawContext, ingame: &Ingame, settings: &Settings, shift: bool, mouse: (i32, i32)) {
    draw_terrain_geometry(dc, ingame, mouse);
    draw_target_lines(dc, ingame, settings, shift);
    {
        let viewport = ingame.viewport.borrow();
        let generator = ingame.order_generator();
        ingame.with_order_context(settings, |ctx| generator.draw(ctx, dc, &viewport));
    }
    let screen_map = ingame.screen_map.borrow();
    let viewport = ingame.viewport.borrow();
    let render_player = ingame.render_player();
    let sel = SelectionContext {
        world: &ingame.world,
        screen_map: &screen_map,
        local_player: ingame.local_player.as_ref(),
        render_player: render_player.as_ref(),
        deadzone: 0,
    };
    let regular = ingame.world.world_type == WorldType::Regular;
    let status_bars = settings.game.status_bars;
    let view = |x: i32, y: i32| {
        let (vx, vy) = viewport.world_to_view_px((x, y));
        (vx as f32, vy as f32)
    };
    for e in screen_map.entries() {
        let a = &e.actor;
        if !a.is_in_world() || a.is_dead() || sel.fog_obscures(a) {
            continue;
        }
        let selected = ingame.selection.contains(a);
        let health = a.trait_or_default::<dyn IHealth>();
        let damaged = health.as_ref().is_some_and(|h| h.damage_state() != DamageState::UNDAMAGED);
        let display_health = selected
            || ingame.selection.rollover_contains(a)
            || (regular && status_bars == StatusBarsType::AlwaysShow)
            || (regular && status_bars == StatusBarsType::DamageShow && damaged);
        let b: Rect = e.decoration;
        if selected {
            selection_box(
                dc,
                view(b.x, b.y),
                view(b.right(), b.bottom()),
                Color::from_argb(255, 255, 255, 255),
            );
        }
        if display_health
            && a.info.has_trait_info::<SelectableInfo>()
            && let Some(h) = &health
        {
            health_bar(dc, &**h, view(b.x + 1, b.y), view(b.right() - 1, b.y));
        }
        if (selected || (regular && status_bars != StatusBarsType::Standard)) && a.info.has_trait_info::<SelectableInfo>() {
            extra_bars(dc, a, ingame.render_player().as_ref(), view(b.x + 1, b.y), view(b.right() - 1, b.y));
        }
        draw_decorations(dc, ingame, &viewport, a, b, selected);
    }
    let mut circles = Vec::new();
    for a in ingame.world.actors() {
        if !a.is_in_world() || a.is_dead() {
            continue;
        }
        let renderers = a.traits_implementing::<dyn IRenderCircles>();
        if renderers.is_empty() {
            continue;
        }
        let selected = ingame.selection.contains(&a);
        for r in renderers {
            r.circles(&a, ingame.render_player().as_ref(), false, &mut circles);
            if selected {
                r.circles(&a, ingame.render_player().as_ref(), true, &mut circles);
            }
        }
    }
    for c in &circles {
        draw_circle(dc, &viewport, c);
    }
    draw_effect_texts(dc, ingame, &viewport);
}

/// `TerrainGeometryOverlay.RenderAnnotations` on flat maps: the outline of each visible cell in the color of its
/// height, thicker under the mouse, and the projected cell under the mouse in navy.
fn draw_terrain_geometry(dc: &mut DrawContext, ingame: &Ingame, mouse: (i32, i32)) {
    if !ingame.terrain_geometry.get() {
        return;
    }
    let viewport = ingame.viewport.borrow();
    let map = ingame.world.map();
    let colors = &map.terrain.height_debug_colors;
    let render_player = ingame.render_player();
    let mouse_cell = crate::widgets::world_interaction::view_to_world(viewport.view_to_world_px(mouse));
    let (tl, br) = (viewport.top_left(), viewport.bottom_right());
    let view = |x: i32, y: i32| viewport.world_to_view_px((x * 24, y * 24));
    for v in (tl.1.div_euclid(24) - 1)..=(br.1.div_euclid(24) + 1) {
        for u in (tl.0.div_euclid(24) - 1)..=(br.0.div_euclid(24) + 1) {
            let uv = rustora_core::world_types::MPos::new(u, v);
            if !map.height.contains(uv) {
                continue;
            }
            let cell = rustora_core::world_types::CPos::new(u, v);
            if render_player.as_ref().is_some_and(|p| !p.shroud().is_explored(cell)) {
                continue;
            }
            let height = *map.height.get(uv) as usize;
            let color = colors[height.min(colors.len() - 1)];
            let width = if cell == mouse_cell { 3.0 } else { 1.0 };
            dc.draw_rect(view(u, v), view(u + 1, v + 1), width, color);
        }
    }
    if map.height.contains_cell(mouse_cell) {
        let (x, y) = (mouse_cell.x(), mouse_cell.y());
        dc.draw_rect(view(x, y), view(x + 1, y + 1), 3.0, Color::from_argb(255, 0, 0, 128));
    }
}

/// The `TextAnnotationRenderable`s of the `IEffectAnnotation` effects, centered on their position.
fn draw_effect_texts(dc: &mut DrawContext, ingame: &Ingame, viewport: &Viewport) {
    let render_player = ingame.render_player();
    let (dark, light) = {
        let m = &dc.assets.metrics;
        (m.color("TextContrastColorDark"), m.color("TextContrastColorLight"))
    };
    for e in ingame.world.effects() {
        let Some((p, color, text)) = e.annotation_text(render_player.as_ref()) else {
            continue;
        };
        let (x, y) = viewport.world_to_view_px((p.x * 24 / 1024, (p.y - p.z) * 24 / 1024));
        let (w, h) = dc.measure("TinyBold", &text);
        dc.draw_text(
            "TinyBold",
            &text,
            x - w / 2,
            y - h / 2,
            color,
            TextStyle::Contrast { dark, light, radius: 1 },
        );
    }
}
