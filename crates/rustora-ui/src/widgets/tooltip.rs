// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use super::impl_as_any;
use crate::draw::DrawContext;
use crate::widget::{EventBounds, Ui, WidgetArgs, WidgetId, WidgetKind};

const CURSOR_OFFSET: (i32, i32) = (0, 20);
const BOTTOM_EDGE_Y_OFFSET: i32 = -5;

/// Port of `TooltipContainerWidget`. The tooltip loads on the first tick after the delay, because fonts are not
/// measurable while a frame is being drawn.
#[derive(Clone)]
pub struct TooltipContainerWidget {
    pub tooltip_delay_milliseconds: i64,
    tooltip: Option<WidgetId>,
    pending: Option<(String, WidgetArgs)>,
    next_token: i32,
    current_token: i32,
}

impl Default for TooltipContainerWidget {
    fn default() -> Self {
        TooltipContainerWidget {
            tooltip_delay_milliseconds: 200,
            tooltip: None,
            pending: None,
            next_token: 1,
            current_token: 0,
        }
    }
}

impl TooltipContainerWidget {
    fn delay_passed(&self, ui: &Ui) -> bool {
        ui.run_time() > ui.last_move_time + self.tooltip_delay_milliseconds
    }

    /// Port of `SetTooltip`.
    pub fn set_tooltip(ui: &mut Ui, id: WidgetId, template: &str, args: WidgetArgs) -> i32 {
        Self::remove_tooltip(ui, id);
        let mut widget = ui.get::<TooltipContainerWidget>(id);
        widget.current_token = widget.next_token;
        widget.next_token += 1;
        widget.tooltip = None;
        widget.pending = Some((template.to_string(), args));
        widget.current_token
    }

    /// Port of `RemoveTooltip()`.
    pub fn remove_tooltip(ui: &mut Ui, id: WidgetId) {
        let token = ui.get::<TooltipContainerWidget>(id).current_token;
        Self::remove_tooltip_token(ui, id, token);
    }

    /// Port of `RemoveTooltip(int)`.
    pub fn remove_tooltip_token(ui: &mut Ui, id: WidgetId, token: i32) {
        {
            let mut widget = ui.get::<TooltipContainerWidget>(id);
            if widget.current_token != token {
                return;
            }
            widget.tooltip = None;
            widget.pending = None;
        }
        ui.remove_children(id);
    }

    /// Port of `ChildOrigin`.
    fn child_origin(&self, ui: &Ui) -> (i32, i32) {
        let (mx, my) = ui.last_mouse;
        let (mut x, mut y) = (mx + CURSOR_OFFSET.0, my + CURSOR_OFFSET.1);
        if let Some(tooltip) = self.tooltip {
            let b = ui.node(tooltip).bounds;
            if x + b.right() > ui.resolution.0 {
                x = ui.resolution.0 - b.right();
            }
            if y + b.bottom() > ui.resolution.1 {
                y = my + BOTTOM_EDGE_Y_OFFSET - b.h;
            }
        }
        (x, y)
    }
}

impl WidgetKind for TooltipContainerWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "TooltipDelayMilliseconds" => self.tooltip_delay_milliseconds = value.parse().unwrap_or(self.tooltip_delay_milliseconds),
            "CursorOffset" | "BottomEdgeYOffset" => {}
            _ => return false,
        }
        true
    }

    fn event_bounds(&self) -> EventBounds {
        EventBounds::Never
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        if self.delay_passed(ui)
            && self.tooltip.is_none()
            && let Some((template, args)) = self.pending.take()
        {
            let args = args.with_value("tooltipContainer", id);
            self.tooltip = Some(ui.load_widget(&template, Some(id), &args));
        }
    }

    fn draw_outer(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) -> bool {
        if self.delay_passed(ui) {
            let origin = self.child_origin(ui);
            let own = ui.render_origin(id);
            ui.node_mut(id).child_offset = (origin.0 - own.0, origin.1 - own.1);
            ui.draw_children(id, dc);
        }
        true
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        None
    }
}
