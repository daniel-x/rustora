// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `SupportPowerTimerWidget.cs`: the countdown lines of the support powers whose `DisplayTimerRelationships`
//! include the relationship of their owner to the viewer, like the GPS satellite and the nuke of RA.

use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_mods_common::support_powers::SupportPowerManager;

use super::impl_as_any;
use super::label::TextAlign;
use super::production::format_time_with;
use crate::draw::{DrawContext, TextStyle};
use crate::fluent::MessageArg;
use crate::logic::ingame::Ingame;
use crate::logic::services;
use crate::logic::sidebar::resolved_player_name;
use crate::widget::{EventBounds, Ui, WidgetId, WidgetKind};

const WHITE: Color = Color::from_argb(255, 255, 255, 255);

#[derive(Clone)]
pub struct SupportPowerTimerWidget {
    font: String,
    align: TextAlign,
    /// `TimerOrder`: 1 for descending, -1 for ascending.
    order: i32,
    background_dark: Color,
    background_light: Color,
    ingame: Option<Rc<Ingame>>,
    texts: Vec<(String, Color)>,
}

impl Default for SupportPowerTimerWidget {
    fn default() -> Self {
        SupportPowerTimerWidget {
            font: "Bold".into(),
            align: TextAlign::Left,
            order: 1,
            background_dark: WHITE,
            background_light: WHITE,
            ingame: None,
            texts: Vec::new(),
        }
    }
}

impl WidgetKind for SupportPowerTimerWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "Font" => self.font = value.to_string(),
            "Align" => {
                self.align = match value.trim() {
                    "Center" => TextAlign::Center,
                    "Right" => TextAlign::Right,
                    _ => TextAlign::Left,
                }
            }
            "Order" => self.order = if value.trim() == "Ascending" { -1 } else { 1 },
            _ => return false,
        }
        true
    }

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.ingame = services(ui).ingame.borrow().clone();
        let assets = ui.assets.borrow();
        self.background_dark = assets.metrics.color("TextContrastColorDark");
        self.background_light = assets.metrics.color("TextContrastColorLight");
    }

    fn event_bounds(&self) -> EventBounds {
        EventBounds::Never
    }

    fn tick(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.texts.clear();
        let Some(ingame) = self.ingame.clone() else {
            return;
        };
        let world = &ingame.world;
        let viewer = ingame.render_player().or_else(|| ingame.local_player.clone());
        let blink = world.local_frame_number() % 50 < 25;
        for (actor, manager) in world.actors_with_trait::<SupportPowerManager>() {
            if actor.is_dead() || actor.owner().non_combatant {
                continue;
            }
            for power in manager.powers.borrow().values() {
                let Some(instance) = power.instances.borrow().first().cloned() else {
                    continue;
                };
                let info = &instance.info;
                if info.display_timer_relationships.0 == 0 || power.disabled() {
                    continue;
                }
                let owner = instance.self_.owner();
                if let Some(viewer) = &viewer
                    && !info
                        .display_timer_relationships
                        .has_relationship(owner.relationship_with(Some(viewer)))
                {
                    continue;
                }
                let time = format_time_with(power.remaining_ticks(), false, world.timestep);
                let name = resolved_player_name(ui, world, &owner);
                let assets = ui.assets.borrow();
                let power_name = info.name.as_deref().map(|n| assets.messages.get(n)).unwrap_or_default();
                let text = assets.messages.get_with(
                    "support-power-timer",
                    &[
                        ("player", MessageArg::Text(&name)),
                        ("support-power", MessageArg::Text(&power_name)),
                        ("time", MessageArg::Text(&time)),
                    ],
                );
                let color = if !power.ready() || blink { owner.color() } else { WHITE };
                self.texts.push((text, color));
            }
        }
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let rb = ui.render_bounds(id);
        let style = TextStyle::Shadow {
            dark: self.background_dark,
            light: self.background_light,
            offset: 1,
        };
        let mut y = 0;
        for (text, color) in &self.texts {
            let size = dc.measure(&self.font, text);
            let x = match self.align {
                TextAlign::Center => rb.x + (rb.w - size.0) / 2,
                TextAlign::Right => rb.x + rb.w - size.0,
                TextAlign::Left => rb.x,
            };
            dc.draw_text(&self.font, text, x, rb.y + y, *color, style);
            y += (size.1 + 5) * self.order;
        }
    }
}
