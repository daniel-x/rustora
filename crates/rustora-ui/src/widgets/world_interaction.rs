// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `WorldInteractionControllerWidget.cs` without click sounds, order voices and order flashes.

use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::{CPos, WPos};

use super::impl_as_any;
use crate::assets::Assets;
use crate::draw::DrawContext;
use crate::input::{MouseButton, MouseEvent, MouseInput};
use crate::logic::ingame::Ingame;
use crate::logic::services;
use crate::orders::OrderContext;
use crate::widget::{Ui, WidgetId, WidgetKind};

#[derive(Clone)]
pub struct WorldInteractionControllerWidget {
    ingame: Option<Rc<Ingame>>,
    normal_selection_color: Color,
    alt_selection_color: Color,
    ctrl_selection_color: Color,
    drag_start: (i32, i32),
    mouse_pos: (i32, i32),
    is_dragging: bool,
    deadzone: i32,
    cursor: Option<String>,
}

/// `ChromeMetrics.TryGet` with white as the fallback.
fn metric_color(assets: &Assets, key: &str) -> Color {
    assets.metrics.color(key)
}

/// `Viewport.ViewToWorld` on flat maps: the cell under a world pixel.
pub fn view_to_world(world_px: (i32, i32)) -> CPos {
    let to_world = |px: i32| (px as i64 * 1024 / 24) as i32;
    let pos = WPos::new(to_world(world_px.0), to_world(world_px.1), 0);
    CPos::new(pos.x.div_euclid(1024), pos.y.div_euclid(1024))
}

impl WorldInteractionControllerWidget {
    pub fn new(assets: &Assets) -> WorldInteractionControllerWidget {
        WorldInteractionControllerWidget {
            ingame: None,
            normal_selection_color: metric_color(assets, "NormalSelectionColor"),
            alt_selection_color: metric_color(assets, "AltSelectionColor"),
            ctrl_selection_color: metric_color(assets, "CtrlSelectionColor"),
            drag_start: (0, 0),
            mouse_pos: (0, 0),
            is_dragging: false,
            deadzone: 24,
            cursor: None,
        }
    }

    fn is_valid_dragbox(&self) -> bool {
        let (dx, dy) = (
            (self.drag_start.0 - self.mouse_pos.0) as f64,
            (self.drag_start.1 - self.mouse_pos.1) as f64,
        );
        self.is_dragging && (dx * dx + dy * dy).sqrt() as i32 > self.deadzone
    }

    fn world_px(ingame: &Ingame, view: (i32, i32)) -> (i32, i32) {
        ingame.viewport.borrow().view_to_world_px(view)
    }

    /// `ApplyOrders`.
    fn apply_orders(ingame: &Ingame, ctx: &OrderContext, mi: &MouseInput) {
        let world_px = Self::world_px(ingame, (mi.x, mi.y));
        let cell = view_to_world(world_px);
        for o in ingame.order_generator().order(ctx, cell, world_px, mi) {
            ingame.world.issue_order(o);
        }
    }

    fn handle(&mut self, ui: &mut Ui, id: WidgetId, ingame: &Ingame, ctx: &OrderContext, mi: &MouseInput) -> bool {
        let generator = ingame.order_generator();
        let Some(uog) = generator.as_unit_order_generator() else {
            Self::apply_orders(ingame, ctx, mi);
            self.is_dragging = false;
            self.yield_mouse_focus(ui, id, Some(mi));
            return true;
        };
        let multi_click = mi.multi_tap >= 2;
        let selection = &ingame.selection;
        if mi.button == MouseButton::Left && mi.event == MouseEvent::Down {
            if !ui.take_mouse_focus(id, mi) {
                return false;
            }
            self.drag_start = self.mouse_pos;
            self.is_dragging = true;
        }
        if mi.button == MouseButton::Left && mi.event == MouseEvent::Up {
            if ctx.use_classic_mouse_style
                && ui.has_mouse_focus(id)
                && !self.is_valid_dragbox()
                && !selection.actors().is_empty()
                && !multi_click
                && uog.input_overrides_selection(ctx, self.mouse_pos, mi)
            {
                Self::apply_orders(ingame, ctx, mi);
                self.is_dragging = false;
                self.yield_mouse_focus(ui, id, Some(mi));
                return true;
            }
            let sel = ctx.selection_context();
            if multi_click {
                let candidates = ctx.screen_map.actors_at_mouse(self.mouse_pos);
                let unit = crate::screen_map::with_highest_selection_priority(&candidates, self.mouse_pos, mi.modifiers);
                let eligible = sel.players_to_include_in_selection();
                if let Some(unit) = unit.filter(|u| eligible.iter().any(|p| Rc::ptr_eq(p, &u.owner())))
                    && let Some(class) = rustora_mods_common::selection::selection_class(&unit)
                {
                    let viewport = ingame.viewport.borrow();
                    let new_selection = sel.select_actors_on_screen(viewport.top_left(), viewport.bottom_right(), &[class], &eligible);
                    drop(viewport);
                    selection.combine(&new_selection, true, false);
                }
            } else if self.is_dragging && (uog.clear_selection_on_left_click() || self.is_valid_dragbox()) {
                let new_selection = sel.select_actors_in_box_with_deadzone(self.drag_start, self.mouse_pos, mi.modifiers);
                selection.combine(&new_selection, mi.modifiers.shift, self.drag_start == self.mouse_pos);
            }
            generator.selection_changed(ctx);
            ingame.cancel_input_mode();
            self.is_dragging = false;
            self.yield_mouse_focus(ui, id, Some(mi));
        }
        if mi.button == MouseButton::Right && mi.event == MouseEvent::Up && !self.is_valid_dragbox() {
            if ctx.use_classic_mouse_style {
                selection.clear();
            }
            Self::apply_orders(ingame, ctx, mi);
        }
        true
    }
}

impl WidgetKind for WorldInteractionControllerWidget {
    impl_as_any!();

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.ingame = services(ui).ingame.borrow().clone();
    }

    fn draw(&mut self, ui: &mut Ui, _id: WidgetId, dc: &mut DrawContext) {
        let Some(ingame) = self.ingame.clone() else {
            return;
        };
        let settings = services(ui).settings.clone();
        let settings = settings.borrow();
        self.deadzone = settings.game.selection_deadzone;
        let modifiers = ui.modifiers;
        ingame.with_order_context(&settings, |ctx| {
            let sel = ctx.selection_context();
            let rollover = if self.is_valid_dragbox() {
                let viewport = ingame.viewport.borrow();
                let a = viewport.world_to_view_px(self.drag_start);
                let b = viewport.world_to_view_px(self.mouse_pos);
                let color = if modifiers.alt && !modifiers.ctrl {
                    self.alt_selection_color
                } else if modifiers.ctrl && !modifiers.alt {
                    self.ctrl_selection_color
                } else {
                    self.normal_selection_color
                };
                dc.draw_rect(a, b, 1.0, color);
                sel.select_actors_in_box_with_deadzone(self.drag_start, self.mouse_pos, modifiers)
            } else {
                sel.select_actors_in_box_with_deadzone(self.mouse_pos, self.mouse_pos, modifiers)
            };
            ingame.selection.set_rollover(rollover);
            self.cursor = if self.is_valid_dragbox() {
                None
            } else {
                let screen = ui.last_mouse;
                let world_px = Self::world_px(&ingame, screen);
                let mi = MouseInput {
                    event: MouseEvent::Move,
                    button: ctx.action_button(),
                    x: screen.0,
                    y: screen.1,
                    scroll_delta: 0,
                    modifiers,
                    multi_tap: 0,
                };
                ingame.order_generator().get_cursor(ctx, view_to_world(world_px), world_px, &mi)
            };
        });
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        let Some(ingame) = self.ingame.clone() else {
            return false;
        };
        self.mouse_pos = Self::world_px(&ingame, (mi.x, mi.y));
        let settings = services(ui).settings.clone();
        let settings = settings.borrow();
        ingame.with_order_context(&settings, |ctx| self.handle(ui, id, &ingame, ctx, mi))
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        self.cursor.clone()
    }
}
