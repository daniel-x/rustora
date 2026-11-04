// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use super::button::draw_stateful_panel;
use super::{ButtonWidget, impl_as_any, parse_bool};
use crate::assets::Assets;
use crate::draw::DrawContext;
use crate::input::{KeyInput, MouseInput};
use crate::widget::{Action, Getter, Ui, WidgetId, WidgetKind};

/// Port of `ScrollItemWidget`. A new item stays hidden until [`ScrollItemWidget::setup`] copies it.
#[derive(Clone)]
pub struct ScrollItemWidget {
    pub button: ButtonWidget,
    pub background: String,
    pub enable_child_mouse_over: bool,
    pub item_key: Option<String>,
    pub is_selected: Option<Getter<bool>>,
}

impl ScrollItemWidget {
    pub fn new(assets: &Assets) -> ScrollItemWidget {
        let mut button = ButtonWidget::new(assets);
        button.visual_height = 0;
        ScrollItemWidget {
            button,
            background: "scrollitem".into(),
            enable_child_mouse_over: false,
            item_key: None,
            is_selected: None,
        }
    }

    /// Port of `ScrollItemWidget.Setup`.
    pub fn setup(ui: &mut Ui, template: WidgetId, is_selected: Getter<bool>, on_click: Action) -> WidgetId {
        let item = ui.clone_widget(template);
        ui.set_visible_fn(item, || true);
        let kind = ui.kind(item);
        let mut kind = kind.borrow_mut();
        let widget = kind.as_any().downcast_mut::<ScrollItemWidget>().expect("a scroll item");
        widget.is_selected = Some(is_selected);
        widget.button.on_click = Some(on_click);
        item
    }

    /// Port of `Setup` with a key and a double click action.
    pub fn setup_with_key(
        ui: &mut Ui,
        key: &str,
        template: WidgetId,
        is_selected: Getter<bool>,
        on_click: Action,
        on_double_click: Action,
    ) -> WidgetId {
        let item = Self::setup(ui, template, is_selected, on_click);
        let mut widget = ui.get::<ScrollItemWidget>(item);
        widget.item_key = Some(key.to_string());
        widget.button.on_double_click = Some(on_double_click);
        drop(widget);
        item
    }
}

impl WidgetKind for ScrollItemWidget {
    impl_as_any!();

    fn as_button(&mut self) -> Option<&mut ButtonWidget> {
        Some(&mut self.button)
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "Background" => self.background = value.to_string(),
            "EnableChildMouseOver" => self.enable_child_mouse_over = parse_bool(value),
            _ => return self.button.set_field_base(key, value),
        }
        true
    }

    fn initialize(&mut self, ui: &mut Ui, id: WidgetId) {
        ui.set_visible_fn(id, || false);
        ui.node_mut(id).ignore_child_mouse_over = !self.enable_child_mouse_over;
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        if self.background.is_empty() {
            return;
        }
        let node = ui.node(id);
        let mut hover = ui.mouse_over == Some(id);
        if !node.ignore_child_mouse_over && !hover {
            hover = node.children.iter().any(|&c| ui.mouse_over == Some(c));
        }
        let selected = self.is_selected.as_ref().is_some_and(|f| f());
        let highlighted = self.button.is_highlighted.as_ref().map_or(self.button.highlighted, |f| f());
        let state = (
            self.button.disabled_now(),
            self.button.depressed,
            hover,
            false,
            selected || highlighted,
        );
        draw_stateful_panel(dc, &self.background, ui.render_bounds(id), state);
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        self.button.handle_mouse_with_offset(ui, id, mi)
    }

    fn handle_key(&mut self, ui: &mut Ui, id: WidgetId, e: &KeyInput) -> bool {
        self.button.handle_key(ui, id, e)
    }

    fn yield_mouse_focus(&mut self, ui: &mut Ui, id: WidgetId, mi: Option<&MouseInput>) -> bool {
        self.button.yield_mouse_focus(ui, id, mi)
    }

    fn mouse_entered(&mut self, ui: &mut Ui, id: WidgetId) {
        self.button.show_tooltip(ui, id);
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.button.hide_tooltip(ui);
    }

    fn cursor(&self, default: &str) -> Option<String> {
        self.button.cursor(default)
    }
}
