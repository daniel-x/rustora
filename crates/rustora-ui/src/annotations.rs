// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The selection annotations of `SelectionDecorationsBase.cs`: `SelectionBoxAnnotationRenderable` and the health
//! bar of `SelectionBarsAnnotationRenderable`. Extra bars, decorations and path debugging follow with slice 7.

use rustora_core::fieldloader::Color;
use rustora_engine::damage::DamageState;
use rustora_engine::traits::IHealth;
use rustora_engine::world::WorldType;
use rustora_game::settings::{Settings, StatusBarsType};
use rustora_mods_common::selection::SelectableInfo;

use crate::draw::{DrawContext, Rect};
use crate::logic::ingame::Ingame;
use crate::screen_map::SelectionContext;

fn line(dc: &mut DrawContext, a: (f32, f32), b: (f32, f32), c: Color) {
    let (l, r) = (a.0.min(b.0), a.0.max(b.0));
    let (t, bt) = (a.1.min(b.1), a.1.max(b.1));
    if a.1 == b.1 {
        dc.list.solid([l, t - 0.5, r - l, 1.0], crate::font::premultiplied(c));
    } else {
        dc.list.solid([l - 0.5, t, 1.0, bt - t], crate::font::premultiplied(c));
    }
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

/// Draws the selection boxes and health bars of all actors in the screen map, in screen pixels.
pub fn draw_selection_annotations(dc: &mut DrawContext, ingame: &Ingame, settings: &Settings) {
    {
        let viewport = ingame.viewport.borrow();
        let generator = ingame.order_generator();
        ingame.with_order_context(settings, |ctx| generator.draw(ctx, dc, &viewport));
    }
    let screen_map = ingame.screen_map.borrow();
    let viewport = ingame.viewport.borrow();
    let sel = SelectionContext {
        world: &ingame.world,
        screen_map: &screen_map,
        local_player: ingame.local_player.as_ref(),
        render_player: ingame.render_player.as_ref(),
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
    }
}
