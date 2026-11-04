// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ScrollPanelWidget.cs` and `ListLayout.cs`. The list state lives in the tree node, see [`ListState`].

use super::button::{draw_background, stateful_image};
use super::{ScrollItemWidget, impl_as_any, parse_bool};
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect};
use crate::input::{MouseButton, MouseEvent, MouseInput};
use crate::widget::{EventBounds, ListState, Ui, WidgetId, WidgetKind};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScrollBar {
    Left,
    Right,
    Hidden,
}

/// Port of `ScrollPanelWidget`.
#[derive(Clone)]
pub struct ScrollPanelWidget {
    pub scrollbar_width: i32,
    pub border_width: i32,
    pub top_bottom_spacing: i32,
    pub item_spacing: i32,
    pub button_depth: i32,
    pub background: String,
    pub scrollbar_background: String,
    pub button: String,
    pub decorations: String,
    pub minimum_thumb_size: i32,
    pub align_bottom: bool,
    pub scrollbar: ScrollBar,
    pub collapse_hidden_children: bool,
    pub smooth_scroll_speed: f32,
    up_pressed: bool,
    down_pressed: bool,
    up_disabled: bool,
    down_disabled: bool,
    thumb_pressed: bool,
    up_button_rect: Rect,
    down_button_rect: Rect,
    thumb_rect: Rect,
    last_mouse_y: i32,
}

impl ScrollPanelWidget {
    pub fn new(assets: &Assets) -> ScrollPanelWidget {
        ScrollPanelWidget {
            scrollbar_width: 24,
            border_width: 1,
            top_bottom_spacing: 2,
            item_spacing: 0,
            button_depth: assets.metrics.int("ButtonDepth"),
            background: "scrollpanel-bg".into(),
            scrollbar_background: "scrollpanel-bg".into(),
            button: "scrollpanel-button".into(),
            decorations: "scrollpanel-decorations".into(),
            minimum_thumb_size: 10,
            align_bottom: false,
            scrollbar: ScrollBar::Right,
            collapse_hidden_children: false,
            smooth_scroll_speed: 0.333,
            up_pressed: false,
            down_pressed: false,
            up_disabled: false,
            down_disabled: false,
            thumb_pressed: false,
            up_button_rect: Rect::default(),
            down_button_rect: Rect::default(),
            thumb_rect: Rect::default(),
            last_mouse_y: 0,
        }
    }
}

impl Ui {
    pub(crate) fn list(&mut self, id: WidgetId) -> &mut ListState {
        self.node_mut(id).list.as_mut().expect("a scroll panel")
    }

    /// `ContentHeight`.
    pub fn content_height(&self, id: WidgetId) -> i32 {
        self.node(id).list.as_ref().map_or(0, |l| l.content_height)
    }

    /// Port of `ListLayout.AdjustChild`, for a child about to be added.
    pub(crate) fn adjust_child(&mut self, parent: WidgetId, child: WidgetId) {
        if self.node(parent).list.is_none() {
            return;
        }
        if self.list(parent).grid_pos.is_some() {
            self.adjust_grid_child(parent, child);
            return;
        }
        let no_children = self.node(parent).children.is_empty();
        let visible = self.is_visible(child);
        let height = self.node(child).bounds.h;
        let list = self.list(parent);
        if no_children {
            list.content_height = 2 * list.top_bottom_spacing - list.item_spacing;
        }
        let y = list.content_height - list.top_bottom_spacing + list.item_spacing;
        if !list.collapse_hidden_children || visible {
            list.content_height += height + list.item_spacing;
        }
        self.node_mut(child).bounds.y = y;
    }

    /// Port of `GridLayout.AdjustChild`.
    fn adjust_grid_child(&mut self, parent: WidgetId, child: WidgetId) {
        let no_children = self.node(parent).children.is_empty();
        let width = self.node(parent).bounds.w;
        let child_bounds = self.node(child).bounds;
        let list = self.list(parent);
        let (spacing, item_spacing) = (list.top_bottom_spacing, list.item_spacing);
        if no_children {
            list.content_height = 2 * spacing;
            list.grid_pos = Some((item_spacing, spacing));
        }
        let mut pos = list.grid_pos.unwrap_or_default();
        if pos.0 + child_bounds.w + item_spacing > width - list.scrollbar_width {
            pos = (item_spacing, list.content_height - spacing + item_spacing);
        }
        list.content_height = list.content_height.max(pos.1 + child_bounds.h + spacing);
        list.grid_pos = Some((pos.0 + child_bounds.w + item_spacing, pos.1));
        let bounds = &mut self.node_mut(child).bounds;
        bounds.x += pos.0;
        bounds.y += pos.1;
    }

    /// `Layout = new GridLayout(panel)`.
    pub fn set_grid_layout(&mut self, id: WidgetId) {
        let scrollbar_width = self.get::<ScrollPanelWidget>(id).scrollbar_width;
        let list = self.list(id);
        list.grid_pos = Some((0, 0));
        list.scrollbar_width = scrollbar_width;
    }

    /// Port of `ListLayout.AdjustChildren`. `GridLayout.AdjustChildren` does nothing.
    pub fn adjust_children(&mut self, id: WidgetId) {
        if self.node(id).list.is_none() || self.list(id).grid_pos.is_some() {
            return;
        }
        let list = self.list(id);
        let (spacing, item_spacing, collapse) = (list.top_bottom_spacing, list.item_spacing, list.collapse_hidden_children);
        let mut height = spacing;
        for child in self.node(id).children.clone() {
            self.node_mut(child).bounds.y = height;
            if !collapse || self.is_visible(child) {
                height += self.node(child).bounds.h + item_spacing;
            }
        }
        self.list(id).content_height = height + spacing - item_spacing;
    }

    pub(crate) fn list_changed(&mut self, id: WidgetId) {
        if self.node(id).list.is_some() {
            self.adjust_children(id);
            self.scroll(id, 0, false);
        }
    }

    pub(crate) fn list_cleared(&mut self, id: WidgetId) {
        if self.node(id).list.is_some() {
            self.list(id).content_height = 0;
            self.scroll(id, 0, false);
        }
    }

    /// Port of `SetListOffset`.
    pub fn set_list_offset(&mut self, id: WidgetId, value: f32, smooth: bool) {
        let list = self.list(id);
        list.target_offset = value;
        if !smooth {
            let changed = list.current_offset != value;
            list.current_offset = value;
            self.update_list_origin(id);
            if changed {
                self.reset_tooltips();
            }
        }
    }

    fn update_list_origin(&mut self, id: WidgetId) {
        let list = self.list(id);
        let offset = (list.origin_x, list.current_offset as i32);
        self.node_mut(id).child_offset = offset;
    }

    /// Port of `Scroll`.
    pub fn scroll(&mut self, id: WidgetId, amount: i32, smooth: bool) {
        let speed = self.ui_scroll_speed;
        let height = self.node(id).bounds.h;
        let list = self.list(id);
        let target = list.target_offset + amount as f32 * speed;
        let target = target.max((height - list.content_height) as f32).min(0.0);
        self.set_list_offset(id, target, smooth);
    }

    /// Port of `ScrollToBottom`.
    pub fn scroll_to_bottom(&mut self, id: WidgetId, smooth: bool) {
        let height = self.node(id).bounds.h;
        let list = self.list(id);
        let distance = height - list.content_height;
        let value = if list.align_bottom { distance } else { distance.min(0) };
        self.set_list_offset(id, value as f32, smooth);
    }

    /// Port of `ScrollToTop`.
    pub fn scroll_to_top(&mut self, id: WidgetId, smooth: bool) {
        let height = self.node(id).bounds.h;
        let list = self.list(id);
        let value = if list.align_bottom {
            (height - list.content_height).max(0)
        } else {
            0
        };
        self.set_list_offset(id, value as f32, smooth);
    }

    /// Port of `ScrolledToBottom`.
    pub fn scrolled_to_bottom(&mut self, id: WidgetId) -> bool {
        let height = self.node(id).bounds.h;
        let list = self.list(id);
        list.target_offset == (height - list.content_height).min(0) as f32 || list.content_height <= height
    }

    /// Port of the private `ScrollToItem`.
    fn scroll_to_widget(&mut self, id: WidgetId, item: WidgetId, smooth: bool) {
        let bounds = self.node(item).bounds;
        let panel_height = self.node(id).bounds.h;
        let list = self.list(id);
        let mut new_offset = None;
        if bounds.y as f32 + list.current_offset < 0.0 {
            new_offset = Some(list.item_spacing - bounds.y);
        }
        if bounds.bottom() as f32 + list.current_offset > panel_height as f32 {
            new_offset = Some(panel_height - bounds.bottom() - list.item_spacing);
        }
        if let Some(offset) = new_offset {
            self.set_list_offset(id, offset as f32, smooth);
        }
    }

    fn find_scroll_item(&self, id: WidgetId, test: impl Fn(&mut ScrollItemWidget) -> bool) -> Option<WidgetId> {
        self.node(id).children.iter().copied().find(|&c| {
            let kind = self.kind(c);
            let mut kind = kind.borrow_mut();
            kind.as_any().downcast_mut::<ScrollItemWidget>().is_some_and(&test)
        })
    }

    /// Port of `ScrollToItem(string)`.
    pub fn scroll_to_item(&mut self, id: WidgetId, key: &str, smooth: bool) {
        if let Some(item) = self.find_scroll_item(id, |s| s.item_key.as_deref() == Some(key)) {
            self.scroll_to_widget(id, item, smooth);
        }
    }

    /// Port of `ScrollToSelectedItem`.
    pub fn scroll_to_selected_item(&mut self, id: WidgetId) {
        if let Some(item) = self.find_scroll_item(id, |s| s.is_selected.as_ref().is_some_and(|f| f())) {
            self.scroll_to_widget(id, item, false);
        }
    }

    /// Port of `UpdateSmoothScrolling`.
    fn update_smooth_scrolling(&mut self, id: WidgetId, speed: f32) {
        let now = self.run_time();
        let list = self.list(id);
        if list.last_smooth_scroll_time == 0 {
            list.last_smooth_scroll_time = now;
            return;
        }
        let diff = list.target_offset - list.current_offset;
        if diff.abs() > 1.0 {
            let dt = now - list.last_smooth_scroll_time;
            list.current_offset += diff * speed.clamp(0.1, 1.0) * dt as f32 / 40.0;
            list.last_smooth_scroll_time = now;
            self.update_list_origin(id);
            self.reset_tooltips();
        } else {
            list.last_smooth_scroll_time = now;
            let target = list.target_offset;
            self.set_list_offset(id, target, false);
        }
    }
}

impl WidgetKind for ScrollPanelWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        let int = |current: i32| value.parse().unwrap_or(current);
        match key {
            "ScrollbarWidth" => self.scrollbar_width = int(self.scrollbar_width),
            "BorderWidth" => self.border_width = int(self.border_width),
            "TopBottomSpacing" => self.top_bottom_spacing = int(self.top_bottom_spacing),
            "ItemSpacing" => self.item_spacing = int(self.item_spacing),
            "ButtonDepth" => self.button_depth = int(self.button_depth),
            "Background" => self.background = value.to_string(),
            "ScrollBarBackground" => self.scrollbar_background = value.to_string(),
            "Button" => self.button = value.to_string(),
            "Decorations" => self.decorations = value.to_string(),
            "MinimumThumbSize" => self.minimum_thumb_size = int(self.minimum_thumb_size),
            "Align" => self.align_bottom = value == "Bottom",
            "ScrollBar" => {
                self.scrollbar = match value {
                    "Left" => ScrollBar::Left,
                    "Hidden" => ScrollBar::Hidden,
                    _ => ScrollBar::Right,
                }
            }
            "CollapseHiddenChildren" => self.collapse_hidden_children = parse_bool(value),
            "SmoothScrollSpeed" => self.smooth_scroll_speed = value.parse().unwrap_or(self.smooth_scroll_speed),
            "ClickSound" | "ClickDisabledSound" => {}
            _ => return false,
        }
        true
    }

    fn initialize(&mut self, ui: &mut Ui, id: WidgetId) {
        ui.node_mut(id).list = Some(ListState {
            top_bottom_spacing: self.top_bottom_spacing,
            item_spacing: self.item_spacing,
            collapse_hidden_children: self.collapse_hidden_children,
            origin_x: if self.scrollbar == ScrollBar::Left {
                self.scrollbar_width
            } else {
                0
            },
            align_bottom: self.align_bottom,
            ..ListState::default()
        });
    }

    fn event_bounds(&self) -> EventBounds {
        EventBounds::Own
    }

    fn draw_outer(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) -> bool {
        ui.update_smooth_scrolling(id, self.smooth_scroll_speed);
        let rb = ui.render_bounds(id);
        let width = self.scrollbar_width;
        let scrollbar_height = rb.h - 2 * width;
        let list = ui.node(id).list.clone().expect("a scroll panel");
        let mut thumb_height = 0;
        let mut thumb_origin = rb.y + width;
        if list.content_height > rb.h {
            thumb_height = self.minimum_thumb_size.max(scrollbar_height * rb.h / list.content_height);
            thumb_origin += ((scrollbar_height - thumb_height) as f32 * list.current_offset / (rb.h - list.content_height) as f32) as i32;
        }
        let bar_x = if self.scrollbar == ScrollBar::Left {
            rb.x
        } else {
            rb.right() - width
        };
        let background_rect = match self.scrollbar {
            ScrollBar::Left => Rect::new(rb.x + width, rb.y, rb.w + 1, rb.h),
            ScrollBar::Right => Rect::new(rb.x, rb.y, rb.w - width + 1, rb.h),
            ScrollBar::Hidden => Rect::new(rb.x, rb.y, rb.w + 1, rb.h),
        };
        dc.draw_panel(&self.background, background_rect);
        if self.scrollbar != ScrollBar::Hidden {
            self.up_button_rect = Rect::new(bar_x, rb.y, width, width);
            self.down_button_rect = Rect::new(bar_x, rb.bottom() - width, width, width);
            self.thumb_rect = Rect::new(bar_x, thumb_origin, width, thumb_height);
            let scrollbar_rect = Rect::new(bar_x, rb.y + width - 1, width, scrollbar_height + 2);
            let (mx, my) = ui.last_mouse;
            let over = ui.mouse_over == Some(id);
            let up_hover = over && self.up_button_rect.contains(mx, my);
            let down_hover = over && self.down_button_rect.contains(mx, my);
            let thumb_hover = over && self.thumb_rect.contains(mx, my);
            self.up_disabled = thumb_height == 0 || list.current_offset >= 0.0;
            self.down_disabled = thumb_height == 0 || list.current_offset <= (rb.h - list.content_height) as f32;
            dc.draw_panel(&self.scrollbar_background, scrollbar_rect);
            draw_background(
                dc,
                &self.button,
                self.up_button_rect,
                self.up_disabled,
                self.up_pressed,
                up_hover,
                false,
            );
            draw_background(
                dc,
                &self.button,
                self.down_button_rect,
                self.down_disabled,
                self.down_pressed,
                down_hover,
                false,
            );
            if thumb_height > 0 {
                let focused_hover = ui.has_mouse_focus(id) && thumb_hover;
                draw_background(dc, &self.button, self.thumb_rect, false, focused_hover, thumb_hover, false);
            }
            let arrow_offset = |pressed: bool, disabled: bool, depth: i32| if !pressed || disabled { 4 } else { 4 + depth };
            let up_offset = arrow_offset(self.up_pressed, self.up_disabled, self.button_depth);
            let down_offset = arrow_offset(self.down_pressed, self.down_disabled, self.button_depth);
            let up_state = (self.up_disabled, self.up_pressed, up_hover, false, false);
            if let Some(s) = stateful_image(dc, &self.decorations, "up", up_state) {
                dc.draw_sprite(
                    s,
                    (self.up_button_rect.x + up_offset) as f32,
                    (self.up_button_rect.y + up_offset) as f32,
                );
            }
            let down_state = (self.down_disabled, self.down_pressed, down_hover, false, false);
            if let Some(s) = stateful_image(dc, &self.decorations, "down", down_state) {
                dc.draw_sprite(
                    s,
                    (self.down_button_rect.x + down_offset) as f32,
                    (self.down_button_rect.y + down_offset) as f32,
                );
            }
        }
        let b = self.border_width;
        let draw_bounds = background_rect.inflate(-b, -b, -b, -b);
        dc.push_clip(draw_bounds);
        let (cx, cy) = ui.child_origin(id);
        let relative = Rect::new(draw_bounds.x - cx, draw_bounds.y - cy, draw_bounds.w, draw_bounds.h);
        for child in ui.node(id).children.clone() {
            if ui.node(child).bounds.intersects(relative) {
                ui.draw_outer(child, dc);
            }
        }
        dc.pop_clip();
        true
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        if self.up_pressed {
            ui.scroll(id, 1, false);
        }
        if self.down_pressed {
            ui.scroll(id, -1, false);
        }
    }

    fn yield_mouse_focus(&mut self, ui: &mut Ui, id: WidgetId, _mi: Option<&MouseInput>) -> bool {
        self.up_pressed = false;
        self.down_pressed = false;
        self.thumb_pressed = false;
        if ui.mouse_focus == Some(id) {
            ui.mouse_focus = None;
        }
        true
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        if mi.event == MouseEvent::Scroll {
            ui.scroll(id, mi.scroll_delta, true);
            return true;
        }
        if mi.button != MouseButton::Left {
            return false;
        }
        if mi.event == MouseEvent::Down && !ui.take_mouse_focus(id, mi) {
            return false;
        }
        if !ui.has_mouse_focus(id) {
            return false;
        }
        if mi.event == MouseEvent::Up {
            return self.yield_mouse_focus(ui, id, Some(mi));
        }
        if self.thumb_pressed && mi.event == MouseEvent::Move {
            let rb = ui.render_bounds(id);
            let content = ui.content_height(id);
            let scrollbar_height = rb.h - 2 * self.scrollbar_width;
            let thumb_height = if content == 0 {
                0
            } else {
                self.minimum_thumb_size
                    .max((scrollbar_height as f32 * (rb.h as f32 / content as f32).min(1.0)) as i32)
            };
            let old_offset = ui.node(id).list.as_ref().map_or(0.0, |l| l.current_offset);
            let dragged = ((self.last_mouse_y - mi.y) as f32 * (content - rb.h) as f32 / (scrollbar_height - thumb_height) as f32) as i32;
            let new_offset = (old_offset + dragged as f32).max((rb.h - content) as f32).min(0.0);
            ui.set_list_offset(id, new_offset, false);
            if old_offset != new_offset {
                self.last_mouse_y = mi.y;
            }
        } else {
            self.up_pressed = self.up_button_rect.contains(mi.x, mi.y);
            self.down_pressed = self.down_button_rect.contains(mi.x, mi.y);
            self.thumb_pressed = self.thumb_rect.contains(mi.x, mi.y);
            if self.thumb_pressed {
                self.last_mouse_y = mi.y;
            }
        }
        self.up_pressed || self.down_pressed || self.thumb_pressed
    }
}
