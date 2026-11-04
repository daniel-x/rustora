// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ViewportControllerWidget.cs` without the world tooltip, which follows with the selection.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_game::settings::MouseScrollType;

use super::impl_as_any;
use crate::draw::DrawContext;
use crate::input::{KeyEvent, KeyInput, MouseButton, MouseEvent, MouseInput};
use crate::logic::services;
use crate::viewport::{ScrollDirection, Viewport};
use crate::widget::{Ui, WidgetId, WidgetKind};

const SCROLL_CURSORS: [(ScrollDirection, &str); 8] = [
    (ScrollDirection(1 | 2), "scroll-tl"),
    (ScrollDirection(1 | 8), "scroll-tr"),
    (ScrollDirection(4 | 2), "scroll-bl"),
    (ScrollDirection(4 | 8), "scroll-br"),
    (ScrollDirection::UP, "scroll-t"),
    (ScrollDirection::DOWN, "scroll-b"),
    (ScrollDirection::LEFT, "scroll-l"),
    (ScrollDirection::RIGHT, "scroll-r"),
];

const JOYSTICK_CURSORS: [(ScrollDirection, &str); 8] = [
    (ScrollDirection(1 | 2), "joystick-tl-blocked"),
    (ScrollDirection(1 | 8), "joystick-tr-blocked"),
    (ScrollDirection(4 | 2), "joystick-bl-blocked"),
    (ScrollDirection(4 | 8), "joystick-br-blocked"),
    (ScrollDirection::UP, "joystick-t-blocked"),
    (ScrollDirection::DOWN, "joystick-b-blocked"),
    (ScrollDirection::LEFT, "joystick-l-blocked"),
    (ScrollDirection::RIGHT, "joystick-r-blocked"),
];

const SCROLL_OFFSETS: [(ScrollDirection, f32, f32); 4] = [
    (ScrollDirection::UP, 0.0, -1.0),
    (ScrollDirection::DOWN, 0.0, 1.0),
    (ScrollDirection::LEFT, -1.0, 0.0),
    (ScrollDirection::RIGHT, 1.0, 0.0),
];

/// Port of `ViewportControllerWidget`.
#[derive(Clone, Default)]
pub struct ViewportControllerWidget {
    viewport: Option<Rc<RefCell<Viewport>>>,
    keys: Keys,
    bookmark_save_prefix: String,
    bookmark_restore_prefix: String,
    bookmark_count: usize,
    bookmarks: Vec<Option<(i32, i32)>>,
    edge_directions: ScrollDirection,
    keyboard_directions: ScrollDirection,
    joystick_start: Option<(i32, i32)>,
    joystick_end: Option<(i32, i32)>,
    standard_start: Option<(i32, i32)>,
    is_standard_scrolling: bool,
    last_scroll_time: i64,
    deadzone: i32,
    edge_scroll: bool,
    mouse_over: bool,
}

/// The hotkey names set by the layout.
#[derive(Clone, Default)]
struct Keys {
    zoom_in: String,
    zoom_out: String,
    scroll_up: String,
    scroll_down: String,
    scroll_left: String,
    scroll_right: String,
    jump_top: String,
    jump_bottom: String,
    jump_left: String,
    jump_right: String,
}

fn length(a: (i32, i32), b: (i32, i32)) -> f32 {
    let (dx, dy) = ((a.0 - b.0) as f32, (a.1 - b.1) as f32);
    (dx * dx + dy * dy).sqrt()
}

impl ViewportControllerWidget {
    fn is_joystick_scrolling(&self) -> bool {
        match (self.joystick_start, self.joystick_end) {
            (Some(start), Some(end)) => length(start, end) > self.deadzone as f32,
            _ => false,
        }
    }

    /// Port of `CheckForDirections`.
    fn check_for_directions(ui: &Ui, margin: i32) -> ScrollDirection {
        let (x, y) = ui.last_mouse;
        ScrollDirection::NONE
            .with(ScrollDirection::LEFT, x < margin)
            .with(ScrollDirection::UP, y < margin)
            .with(ScrollDirection::RIGHT, x >= ui.resolution.0 - margin)
            .with(ScrollDirection::DOWN, y >= ui.resolution.1 - margin)
    }

    /// The `HandleMapScrollKey` local function. `None` if `name` has another key.
    fn map_scroll_key(&mut self, ui: &Ui, name: &str, direction: ScrollDirection, e: &KeyInput) -> bool {
        let Some(hotkey) = ui.assets.borrow().hotkeys.get(name) else {
            return false;
        };
        if hotkey.key != e.key {
            return false;
        }
        let is_hotkey = hotkey.modifiers == e.modifiers;
        let held = e.event == KeyEvent::Down && (is_hotkey || hotkey.modifiers == Default::default());
        self.keyboard_directions = self.keyboard_directions.with(direction, held);
        is_hotkey
    }
}

impl WidgetKind for ViewportControllerWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        let target = match key {
            "ZoomInKey" => &mut self.keys.zoom_in,
            "ZoomOutKey" => &mut self.keys.zoom_out,
            "ScrollUpKey" => &mut self.keys.scroll_up,
            "ScrollDownKey" => &mut self.keys.scroll_down,
            "ScrollLeftKey" => &mut self.keys.scroll_left,
            "ScrollRightKey" => &mut self.keys.scroll_right,
            "JumpToTopEdgeKey" => &mut self.keys.jump_top,
            "JumpToBottomEdgeKey" => &mut self.keys.jump_bottom,
            "JumpToLeftEdgeKey" => &mut self.keys.jump_left,
            "JumpToRightEdgeKey" => &mut self.keys.jump_right,
            "BookmarkSaveKeyPrefix" => &mut self.bookmark_save_prefix,
            "BookmarkRestoreKeyPrefix" => &mut self.bookmark_restore_prefix,
            "BookmarkKeyCount" => {
                self.bookmark_count = value.parse().unwrap_or(0);
                return true;
            }
            "TooltipContainer" | "TooltipTemplate" => return true,
            _ => return false,
        };
        *target = value.to_string();
        true
    }

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        let services = services(ui);
        self.viewport = services.ingame.borrow().as_ref().map(|ingame| ingame.viewport.clone());
        self.bookmarks = vec![None; self.bookmark_count];
    }

    fn mouse_entered(&mut self, _ui: &mut Ui, _id: WidgetId) {
        self.mouse_over = true;
    }

    fn mouse_exited(&mut self, _ui: &mut Ui, _id: WidgetId) {
        self.mouse_over = false;
    }

    fn draw(&mut self, ui: &mut Ui, _id: WidgetId, _dc: &mut DrawContext) {
        let Some(viewport) = self.viewport.clone() else {
            return;
        };
        let settings = services(ui).settings.clone();
        let settings = settings.borrow();
        self.deadzone = settings.game.mouse_scroll_deadzone;
        self.edge_scroll = settings.game.viewport_edge_scroll;
        let step = settings.game.viewport_edge_scroll_step;
        if let (true, Some(start), Some(end)) = (self.is_joystick_scrolling(), self.joystick_start, self.joystick_end) {
            let rate = 0.01 * step;
            viewport
                .borrow_mut()
                .scroll(((end.0 - start.0) as f32 * rate, (end.1 - start.1) as f32 * rate));
        } else if !self.is_standard_scrolling {
            self.edge_directions = ScrollDirection::NONE;
            if self.edge_scroll && ui.window_has_focus {
                self.edge_directions = Self::check_for_directions(ui, settings.game.viewport_edge_scroll_margin);
            }
            if ui.keyboard_focus.is_some() {
                self.keyboard_directions = ScrollDirection::NONE;
            }
            if self.keyboard_directions != ScrollDirection::NONE || self.edge_directions != ScrollDirection::NONE {
                let (mut x, mut y) = (0.0f32, 0.0f32);
                for (direction, dx, dy) in SCROLL_OFFSETS {
                    if self.keyboard_directions.includes(direction) || self.edge_directions.includes(direction) {
                        x += dx;
                        y += dy;
                    }
                }
                let delta_scale = ((ui.run_time() - self.last_scroll_time) as f32).min(25.0);
                let scale = delta_scale / (25.0 * (x * x + y * y).sqrt().max(1.0)) * step;
                viewport.borrow_mut().scroll((x * scale, y * scale));
                self.last_scroll_time = ui.run_time();
            }
        }
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        let scrolling = self.is_joystick_scrolling() || self.is_standard_scrolling;
        if !scrolling && (!self.edge_scroll || !self.mouse_over) {
            return None;
        }
        let blocked = self.viewport.as_ref()?.borrow().blocked_directions();
        if scrolling {
            for (direction, cursor) in JOYSTICK_CURSORS {
                if blocked.includes(direction) {
                    return Some(cursor.to_string());
                }
            }
            return Some("joystick-all".to_string());
        }
        for (direction, cursor) in SCROLL_CURSORS {
            if self.edge_directions.includes(direction) {
                let suffix = if blocked.includes(direction) { "-blocked" } else { "" };
                return Some(format!("{cursor}{suffix}"));
            }
        }
        None
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        let Some(viewport) = self.viewport.clone() else {
            return false;
        };
        let settings = services(ui).settings.clone();
        let (zoom_modifier, zoom_speed, classic, alternate, mouse_scroll) = {
            let settings = settings.borrow();
            let game = &settings.game;
            (
                game.zoom_modifier,
                game.zoom_speed,
                game.use_classic_mouse_style,
                game.use_alternate_scroll_button,
                game.mouse_scroll,
            )
        };
        if mi.event == MouseEvent::Scroll && mi.modifiers.has_flags(zoom_modifier.0) {
            viewport
                .borrow_mut()
                .adjust_zoom_at(mi.scroll_delta as f32 * zoom_speed, (mi.x, mi.y));
            return true;
        }
        let scroll_button = if classic ^ alternate {
            MouseButton::Right
        } else {
            MouseButton::Middle
        };
        let scroll_type = if mi.button == scroll_button {
            mouse_scroll
        } else {
            MouseScrollType::Disabled
        };
        let location = (mi.x, mi.y);
        match scroll_type {
            MouseScrollType::Disabled => {}
            MouseScrollType::Standard | MouseScrollType::Inverted => match mi.event {
                MouseEvent::Down if !self.is_standard_scrolling => {
                    if !ui.take_mouse_focus(id, mi) {
                        return false;
                    }
                    self.standard_start = Some(location);
                }
                MouseEvent::Move
                    if self.is_standard_scrolling
                        || self
                            .standard_start
                            .is_some_and(|start| length(start, location) > self.deadzone as f32) =>
                {
                    self.is_standard_scrolling = true;
                    let d = if scroll_type == MouseScrollType::Inverted { -1.0 } else { 1.0 };
                    let last = ui.previous_mouse;
                    viewport
                        .borrow_mut()
                        .scroll(((last.0 - mi.x) as f32 * d, (last.1 - mi.y) as f32 * d));
                    return true;
                }
                MouseEvent::Up => {
                    let was_scrolling = self.is_standard_scrolling;
                    self.is_standard_scrolling = false;
                    self.standard_start = None;
                    self.yield_mouse_focus(ui, id, Some(mi));
                    if was_scrolling {
                        return true;
                    }
                }
                _ => {}
            },
            MouseScrollType::Joystick => match mi.event {
                MouseEvent::Down => {
                    if !ui.take_mouse_focus(id, mi) {
                        return false;
                    }
                    self.joystick_start = Some(location);
                }
                MouseEvent::Up => {
                    let was_scrolling = self.is_joystick_scrolling();
                    self.yield_mouse_focus(ui, id, Some(mi));
                    if was_scrolling {
                        return true;
                    }
                }
                MouseEvent::Move => {
                    if self.joystick_start.is_none() {
                        self.joystick_start = Some(location);
                    }
                    self.joystick_end = Some(location);
                }
                _ => {}
            },
        }
        self.is_joystick_scrolling() || self.is_standard_scrolling
    }

    fn yield_mouse_focus(&mut self, ui: &mut Ui, id: WidgetId, _mi: Option<&MouseInput>) -> bool {
        self.joystick_start = None;
        self.joystick_end = None;
        if ui.mouse_focus == Some(id) {
            ui.mouse_focus = None;
        }
        true
    }

    fn yield_keyboard_focus(&mut self, ui: &mut Ui, id: WidgetId) -> bool {
        self.keyboard_directions = ScrollDirection::NONE;
        if ui.keyboard_focus == Some(id) {
            ui.keyboard_focus = None;
        }
        true
    }

    fn handle_key(&mut self, ui: &mut Ui, _id: WidgetId, e: &KeyInput) -> bool {
        let Some(viewport) = self.viewport.clone() else {
            return false;
        };
        let keys = self.keys.clone();
        if self.map_scroll_key(ui, &keys.scroll_up, ScrollDirection::UP, e)
            || self.map_scroll_key(ui, &keys.scroll_down, ScrollDirection::DOWN, e)
            || self.map_scroll_key(ui, &keys.scroll_left, ScrollDirection::LEFT, e)
            || self.map_scroll_key(ui, &keys.scroll_right, ScrollDirection::RIGHT, e)
        {
            return true;
        }
        if e.event != KeyEvent::Down {
            return false;
        }
        let assets = ui.assets.clone();
        let assets = assets.borrow();
        let active = |name: &str| assets.hotkeys.is_activated(name, e);
        let mut viewport = viewport.borrow_mut();
        let bounds = viewport.map_bounds();
        let center = viewport.center;
        if active(&keys.zoom_in) {
            viewport.adjust_zoom(0.25);
        } else if active(&keys.zoom_out) {
            viewport.adjust_zoom(-0.25);
        } else if active(&keys.jump_top) {
            viewport.center_on((center.0, bounds.y));
        } else if active(&keys.jump_bottom) {
            viewport.center_on((center.0, bounds.bottom()));
        } else if active(&keys.jump_left) {
            viewport.center_on((bounds.x, center.1));
        } else if active(&keys.jump_right) {
            viewport.center_on((bounds.right(), center.1));
        } else {
            for i in 0..self.bookmark_count {
                let suffix = format!("{:02}", i + 1);
                if active(&format!("{}{suffix}", self.bookmark_save_prefix)) {
                    self.bookmarks[i] = Some(center);
                    return true;
                }
                if active(&format!("{}{suffix}", self.bookmark_restore_prefix))
                    && let Some(bookmark) = self.bookmarks[i]
                {
                    viewport.center_on(bookmark);
                    return true;
                }
            }
            return false;
        }
        true
    }
}
