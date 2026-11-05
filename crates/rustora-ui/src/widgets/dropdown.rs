// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::rc::Rc;

use super::button::stateful_image;
use super::label::parse_align;
use super::{ButtonWidget, ScrollItemWidget, TextAlign, impl_as_any};
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect};
use crate::input::{KeyInput, MouseEvent, MouseInput};
use crate::loader::cell;
use crate::widget::{Action, Ui, WidgetArgs, WidgetId, WidgetKind};

/// Port of `MaskWidget`: takes all clicks while a drop down is open.
#[derive(Clone, Default)]
pub struct MaskWidget {
    pub on_mouse_down: Vec<Rc<dyn Fn(&mut Ui, &MouseInput)>>,
}

impl WidgetKind for MaskWidget {
    impl_as_any!();

    fn handle_mouse(&mut self, ui: &mut Ui, _id: WidgetId, mi: &MouseInput) -> bool {
        if mi.event == MouseEvent::Move {
            return false;
        }
        if mi.event == MouseEvent::Down {
            for action in self.on_mouse_down.clone() {
                let mi = *mi;
                ui.defer(move |ui| action(ui, &mi));
            }
        }
        true
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        None
    }
}

/// Port of `DropDownButtonWidget`.
#[derive(Clone)]
pub struct DropDownButtonWidget {
    pub button: ButtonWidget,
    pub decorations: String,
    pub decoration_marker: String,
    pub separators: String,
    pub separator_image: String,
    pub panel_align: TextAlign,
    pub panel_root: Option<String>,
    panel: Option<(WidgetId, WidgetId, WidgetId)>,
}

impl DropDownButtonWidget {
    pub fn new(assets: &Assets) -> DropDownButtonWidget {
        DropDownButtonWidget {
            button: ButtonWidget::new(assets),
            decorations: "dropdown-decorations".into(),
            decoration_marker: "marker".into(),
            separators: "dropdown-separators".into(),
            separator_image: "separator".into(),
            panel_align: TextAlign::Left,
            panel_root: None,
            panel: None,
        }
    }

    fn close(&mut self, ui: &mut Ui) {
        if let Some((panel, mask, root)) = self.panel.take() {
            if ui.exists(root) {
                ui.remove_child(root, mask);
                ui.remove_child(root, panel);
            }
            ui.reset_tooltips();
        }
    }

    /// Port of `RemovePanel`.
    pub fn remove_panel(ui: &mut Ui, id: WidgetId) {
        if !ui.exists(id) {
            return;
        }
        let kind = ui.kind(id);
        let mut kind = kind.borrow_mut();
        kind.as_any()
            .downcast_mut::<DropDownButtonWidget>()
            .expect("a drop down button")
            .close(ui);
    }

    /// Port of `AttachPanel`.
    pub fn attach_panel(ui: &mut Ui, id: WidgetId, panel: WidgetId, on_cancel: Option<Action>) {
        let (panel_root, align) = {
            let widget = ui.get::<DropDownButtonWidget>(id);
            assert!(widget.panel.is_none(), "Attempted to attach a panel to an open dropdown");
            (widget.panel_root.clone(), widget.panel_align)
        };
        let mut mask_kind = MaskWidget::default();
        mask_kind.on_mouse_down.push(Rc::new(move |ui, _| {
            let click_sound = ui.assets.borrow().metrics.string("ClickSound");
            crate::logic::play_ui_sound(ui, &click_sound);
            Self::remove_panel(ui, id);
        }));
        if let Some(on_cancel) = on_cancel {
            mask_kind.on_mouse_down.push(Rc::new(move |ui, _| on_cancel(ui)));
        }
        let mask = ui.alloc(cell(mask_kind));
        ui.node_mut(mask).bounds = Rect::new(0, 0, ui.resolution.0, ui.resolution.1);
        let root = panel_root.map_or(ui.root, |name| ui.child(ui.root, &name));
        ui.add_child(root, mask);

        let origin = ui.render_origin(id);
        let root_origin = ui.render_origin(root);
        let bounds = ui.node(id).bounds;
        let old = ui.node(panel).bounds;
        let mut x = origin.0 - root_origin.0;
        match align {
            TextAlign::Right => x += bounds.w - old.w,
            TextAlign::Center => x += (bounds.w - old.w) / 2,
            TextAlign::Left => {}
        }
        let mut y = origin.1 + bounds.h - root_origin.1;
        if y + old.h > ui.resolution.1 {
            y -= bounds.h + old.h;
        }
        ui.node_mut(panel).bounds = Rect::new(x, y, old.w, old.h);
        ui.add_child(root, panel);
        ui.get::<DropDownButtonWidget>(id).panel = Some((panel, mask, root));
        if ui.node(panel).list.is_some() {
            ui.scroll_to_selected_item(panel);
        }
    }

    /// Port of `ShowDropDown` with a flat list of options.
    pub fn show_dropdown<T>(
        ui: &mut Ui,
        id: WidgetId,
        template: &str,
        max_height: i32,
        options: impl IntoIterator<Item = T>,
        setup_item: impl Fn(&mut Ui, T, WidgetId) -> WidgetId,
    ) {
        let mut args = WidgetArgs::default();
        args.substitutions.insert("DROPDOWN_WIDTH".into(), ui.node(id).bounds.w);
        let panel = ui.load_widget(template, None, &args);
        let item_template = ui.child(panel, "TEMPLATE");
        let template_copy = ui.clone_widget(item_template);
        ui.remove_children(panel);
        for option in options {
            Self::add_option(ui, id, panel, option, template_copy, &setup_item);
        }
        ui.dispose_detached(template_copy);
        ui.node_mut(panel).bounds.h = max_height.min(ui.content_height(panel));
        Self::attach_panel(ui, id, panel, None);
    }

    /// Port of `ShowDropDown` with options in named groups.
    pub fn show_grouped_dropdown<T>(
        ui: &mut Ui,
        id: WidgetId,
        template: &str,
        max_height: i32,
        groups: impl IntoIterator<Item = (String, Vec<T>)>,
        setup_item: impl Fn(&mut Ui, T, WidgetId) -> WidgetId,
    ) {
        let mut args = WidgetArgs::default();
        args.substitutions.insert("DROPDOWN_WIDTH".into(), ui.node(id).bounds.w);
        let panel = ui.load_widget(template, None, &args);
        let header_template = ui.find(panel, "HEADER").map(|h| ui.clone_widget(h));
        let item_template = ui.child(panel, "TEMPLATE");
        let template_copy = ui.clone_widget(item_template);
        ui.remove_children(panel);
        for (group, options) in groups {
            if let (false, Some(header_template)) = (group.is_empty(), header_template) {
                let header = ScrollItemWidget::setup(ui, header_template, Rc::new(|| false), Rc::new(|_| {}));
                let label = ui.child(header, "LABEL");
                ui.get::<super::LabelWidget>(label).get_text = Some(Rc::new(move || group.clone()));
                ui.add_child(panel, header);
            }
            for option in options {
                Self::add_option(ui, id, panel, option, template_copy, &setup_item);
            }
        }
        ui.dispose_detached(template_copy);
        if let Some(h) = header_template {
            ui.dispose_detached(h);
        }
        ui.node_mut(panel).bounds.h = max_height.min(ui.content_height(panel));
        Self::attach_panel(ui, id, panel, None);
    }

    fn add_option<T>(
        ui: &mut Ui,
        id: WidgetId,
        panel: WidgetId,
        option: T,
        template: WidgetId,
        setup_item: &impl Fn(&mut Ui, T, WidgetId) -> WidgetId,
    ) {
        let item = setup_item(ui, option, template);
        let on_click = ui.button(item).on_click.clone();
        ui.button(item).on_click = Some(Rc::new(move |ui| {
            if let Some(on_click) = &on_click {
                on_click(ui);
            }
            Self::remove_panel(ui, id);
        }));
        ui.add_child(panel, item);
    }
}

impl WidgetKind for DropDownButtonWidget {
    impl_as_any!();

    fn as_button(&mut self) -> Option<&mut ButtonWidget> {
        Some(&mut self.button)
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "Decorations" => self.decorations = value.to_string(),
            "DecorationMarker" => self.decoration_marker = value.to_string(),
            "Separators" => self.separators = value.to_string(),
            "SeparatorImage" => self.separator_image = value.to_string(),
            "PanelAlign" => self.panel_align = parse_align(value),
            "PanelRoot" => self.panel_root = Some(value.to_string()),
            _ => return self.button.set_field_base(key, value),
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let bounds = ui.node(id).bounds;
        self.button.draw_button(ui, id, dc, bounds.w - bounds.h);
        let depressed = self.button.depressed;
        let state_offset = if depressed { self.button.visual_height } else { 0 } as f32;
        let rb = ui.render_bounds(id);
        let disabled = self.button.disabled_now();
        let hover = ui.mouse_over == Some(id) || ui.node(id).children.iter().any(|&c| ui.mouse_over == Some(c));
        let highlighted = self.button.is_highlighted.as_ref().map_or(self.button.highlighted, |f| f());
        let state = (disabled, depressed, hover, false, highlighted);
        if let Some(arrow) = stateful_image(dc, &self.decorations, &self.decoration_marker, state) {
            let x = rb.right() - (rb.h + arrow.w) / 2;
            let y = rb.y + (rb.h - arrow.h) / 2;
            dc.draw_sprite(arrow, state_offset + x as f32, state_offset + y as f32);
        }
        if let Some(separator) = stateful_image(dc, &self.separators, &self.separator_image, state) {
            let x = rb.right() - rb.h + 4 - 3;
            let y = rb.y + (rb.h - separator.h) / 2;
            dc.draw_sprite(separator, state_offset + x as f32, state_offset + y as f32);
        }
    }

    fn hidden(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.close(ui);
    }

    fn removed(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.close(ui);
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
