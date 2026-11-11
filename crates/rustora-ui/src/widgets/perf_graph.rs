// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `PerfGraphWidget.cs` and `StrategicProgressWidget.cs`.

use rustora_core::fieldloader::Color;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_mods_common::player::objectives::{StrategicPoint, StrategicVictoryConditions};

use super::impl_as_any;
use crate::draw::{DrawContext, TextStyle};
use crate::font::premultiplied;
use crate::widget::{Ui, WidgetId, WidgetKind};

const WHITE: Color = Color::from_argb(255, 255, 255, 255);

fn line(dc: &mut DrawContext, a: (f32, f32), b: (f32, f32), color: Color) {
    dc.list.line([a.0, a.1], [b.0, b.1], 1.0, premultiplied(color));
}

/// Port of `PerfGraphWidget`: the last 100 samples of each `PerfHistory` item, 100 units high.
#[derive(Clone, Default)]
pub struct PerfGraphWidget;

impl WidgetKind for PerfGraphWidget {
    impl_as_any!();

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let rect = ui.render_bounds(id);
        let (left, top, right, bottom) = (rect.x as f32, rect.y as f32, rect.right() as f32, rect.bottom() as f32);
        let basis = (-((rect.w / 100) as f32), -((rect.h / 100) as f32));
        let at = |x: f32, y: f32| (right + x * basis.0, bottom + y * basis.1);
        line(dc, (left, top), (left, bottom), WHITE);
        line(dc, (left, bottom), (right, bottom), WHITE);
        line(dc, at(100.0, 0.0), at(100.0, 100.0), WHITE);
        let items: Vec<(String, Color, Vec<f64>)> =
            crate::perf::with_items(|items| items.iter().map(|i| (i.name.clone(), i.color, i.samples())).collect());
        for (k, (_, color, samples)) in items.iter().enumerate() {
            for (i, pair) in samples.windows(2).enumerate() {
                line(dc, at(i as f32, pair[0] as f32), at(i as f32 + 1.0, pair[1] as f32), *color);
            }
            let y = top + 10.0 * k as f32;
            line(dc, (left + 10.0, y + 5.0), (left + 12.0, y + 5.0), *color);
            line(dc, (left + 10.0, y + 4.0), (left + 12.0, y + 4.0), *color);
        }
        for (k, (name, _, _)) in items.iter().enumerate() {
            dc.draw_text("Tiny", name, rect.x + 18, rect.y + 10 * k as i32 - 3, WHITE, TextStyle::Plain);
        }
    }
}

/// Port of `StrategicProgressWidget`: the strategic points and the time until a strategic victory or defeat.
#[derive(Clone, Default)]
pub struct StrategicProgressWidget {
    visible: Option<bool>,
}

/// `FindFirstWinningPlayer`: the holding player with the shortest time to win.
fn first_winning_player(players: &[PlayerRef]) -> Option<(PlayerRef, i32)> {
    players
        .iter()
        .filter(|p| !p.non_combatant)
        .filter_map(|p| {
            let svc = p.player_actor().trait_or_default::<StrategicVictoryConditions>()?;
            let left = svc.ticks_left.get();
            (svc.holding(p.player_actor()) && left > 0).then(|| (p.clone(), left))
        })
        .min_by_key(|(_, left)| *left)
}

impl WidgetKind for StrategicProgressWidget {
    impl_as_any!();

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let Some(ingame) = crate::logic::services(ui).ingame.borrow().clone() else {
            return;
        };
        let world = &ingame.world;
        let visible = *self.visible.get_or_insert_with(|| {
            !world.actors_having_trait::<StrategicVictoryConditions>().is_empty()
                && !world.actors_having_trait::<StrategicPoint>().is_empty()
        });
        if !visible {
            return;
        }
        let rb = ui.render_bounds(id);
        let points = world.actors_having_trait::<StrategicPoint>();
        let local = ingame.local_player.as_ref();
        let mut x = -(points.len() as i32 * 32) / 2;
        let (unowned, player_owned, enemy_owned) = (
            dc.image("strategic", "critical_unowned"),
            dc.image("strategic", "player_owned"),
            dc.image("strategic", "enemy_owned"),
        );
        for a in &points {
            let pos = ((rb.x + x) as f32, rb.y as f32);
            if let Some(s) = unowned {
                dc.draw_sprite(s, pos.0, pos.1);
            }
            let owner = a.owner();
            let overlay = if local.is_some_and(|l| owner.relationship_with(Some(l)) == PlayerRelationship::ALLY) {
                player_owned
            } else if !owner.non_combatant {
                enemy_owned
            } else {
                None
            };
            if let Some(s) = overlay {
                dc.draw_sprite(s, pos.0, pos.1);
            }
            x += 32;
        }
        let Some(local) = local else { return };
        let Some((winner, ticks_left)) = first_winning_player(&world.players()) else {
            return;
        };
        let is_victory = winner.relationship_with(Some(local)) == PlayerRelationship::ALLY;
        let text = format!(
            "Strategic {} in {}",
            if is_victory { "victory" } else { "defeat" },
            super::production::format_time(ticks_left, world.timestep)
        );
        let size = dc.measure("Bold", &text);
        let style = TextStyle::Contrast {
            dark: Color::from_argb(255, 0, 0, 0),
            light: Color::from_argb(255, 0, 0, 0),
            radius: 1,
        };
        dc.draw_text("Bold", &text, rb.x - size.0 / 2 + 1, rb.y + 32 + 1, WHITE, style);
    }
}
