// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use super::{impl_as_any, parse_bool};
use crate::draw::{DrawContext, Rect};
use crate::widget::{Getter, Ui, WidgetId, WidgetKind};

/// Port of `ProgressBarWidget`.
#[derive(Clone)]
pub struct ProgressBarWidget {
    pub background: String,
    pub bar: String,
    pub bar_margin: (i32, i32),
    pub percentage: i32,
    pub indeterminate: bool,
    pub get_percentage: Option<Getter<i32>>,
    pub is_indeterminate: Option<Getter<bool>>,
    offset: f32,
    tick_step: f32,
    was_indeterminate: bool,
}

impl Default for ProgressBarWidget {
    fn default() -> Self {
        ProgressBarWidget {
            background: "progressbar-bg".to_string(),
            bar: "progressbar-thumb".to_string(),
            bar_margin: (2, 2),
            percentage: 0,
            indeterminate: false,
            get_percentage: None,
            is_indeterminate: None,
            offset: 0.0,
            tick_step: 0.04,
            was_indeterminate: false,
        }
    }
}

impl WidgetKind for ProgressBarWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "Background" => self.background = value.to_string(),
            "Bar" => self.bar = value.to_string(),
            "BarMargin" => {
                let mut parts = value.split(',').map(|p| p.trim().parse().unwrap_or(0));
                self.bar_margin = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
            }
            "Percentage" => self.percentage = value.trim().parse().unwrap_or(0),
            "Indeterminate" => self.indeterminate = parse_bool(value),
            _ => return false,
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let rb = ui.render_bounds(id);
        let percentage = self.get_percentage.as_ref().map_or(self.percentage, |f| f());
        dc.draw_panel(&self.background, rb);
        let min_bar_width = dc.minimum_panel_size(&self.bar).0;
        let max_bar_width = rb.w - self.bar_margin.0 * 2;
        let bar_width = if self.was_indeterminate {
            max_bar_width / 4
        } else {
            percentage * max_bar_width / 100
        };
        let bar_width = bar_width.max(min_bar_width);
        let bar_offset = if self.was_indeterminate {
            (0.75 * f64::from(self.offset) * f64::from(max_bar_width)) as i32
        } else {
            0
        };
        let bar = Rect::new(
            rb.x + self.bar_margin.0 + bar_offset,
            rb.y + self.bar_margin.1,
            bar_width,
            rb.h - 2 * self.bar_margin.1,
        );
        dc.draw_panel(&self.bar, bar);
    }

    fn tick(&mut self, _ui: &mut Ui, _id: WidgetId) {
        let indeterminate = self.is_indeterminate.as_ref().map_or(self.indeterminate, |f| f());
        if indeterminate != self.was_indeterminate {
            self.offset = 0.0;
        }
        if indeterminate {
            self.offset = (self.offset + self.tick_step).clamp(0.0, 1.0);
            if self.offset == 0.0 || self.offset == 1.0 {
                self.tick_step = -self.tick_step;
            }
        }
        self.was_indeterminate = indeterminate;
    }
}
