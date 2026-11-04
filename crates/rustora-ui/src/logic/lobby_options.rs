// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `LobbyOptionsLogic.cs`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::loader::cell;
use crate::logic::lobby_utils::split_on_first_token;
use crate::logic::{message, services};
use crate::net::{LobbyOption, LobbyRules, Net};
use crate::widget::{ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::{CheckboxWidget, DropDownButtonWidget, LabelWidget, ScrollItemWidget};

/// The `getRules` argument: the rules of the current map.
pub type GetRules = Getter<Option<(String, Rc<LobbyRules>)>>;

struct LobbyOptionsLogic {
    panel: WidgetId,
    container: WidgetId,
    checkbox_row: WidgetId,
    dropdown_row: WidgetId,
    y_margin: i32,
    net: Net,
    get_rules: GetRules,
    configuration_disabled: Getter<bool>,
    current_uid: Option<String>,
}

impl LobbyOptionsLogic {
    fn set_tooltip(ui: &Ui, id: WidgetId, option: &LobbyOption) {
        if let Some(description) = &option.description {
            let (text, desc) = split_on_first_token(description);
            let mut button = ui.button(id);
            button.get_tooltip_text = Some(Rc::new(move || text.clone()));
            button.get_tooltip_desc = Some(Rc::new(move || desc.clone()));
        }
    }

    fn add_checkbox(&self, ui: &mut Ui, checkbox: WidgetId, option: &LobbyOption) {
        let name = option.name.clone();
        ui.button(checkbox).get_text = Some(Rc::new(move || name.clone()));
        Self::set_tooltip(ui, checkbox, option);
        ui.set_visible_fn(checkbox, || true);
        let (net, id) = (self.net.clone(), option.id.clone());
        ui.get::<CheckboxWidget>(checkbox).is_checked = Some(Rc::new(move || {
            net.borrow()
                .session
                .global_settings
                .lobby_options
                .get(&id)
                .is_some_and(|o| o.is_enabled())
        }));
        let (net, id, disabled) = (self.net.clone(), option.id.clone(), self.configuration_disabled.clone());
        ui.button(checkbox).is_disabled = Some(Rc::new(move || {
            disabled()
                || net
                    .borrow()
                    .session
                    .global_settings
                    .lobby_options
                    .get(&id)
                    .is_some_and(|o| o.is_locked)
        }));
        let (net, id) = (self.net.clone(), option.id.clone());
        ui.button(checkbox).on_click = Some(Rc::new(move |_| {
            let mut net = net.borrow_mut();
            let enabled = net.session.global_settings.lobby_options.get(&id).is_some_and(|o| o.is_enabled());
            let state = if enabled { "False" } else { "True" };
            net.command(&format!("option {id} {state}"));
        }));
    }

    fn add_dropdown(&self, ui: &mut Ui, dropdown: WidgetId, row: WidgetId, option: &LobbyOption) {
        let not_available = message(ui, "label-not-available");
        let (net, id, values) = (self.net.clone(), option.id.clone(), option.values.clone());
        ui.button(dropdown).get_text = Some(Rc::new(move || {
            let net = net.borrow();
            net.session
                .global_settings
                .lobby_options
                .get(&id)
                .and_then(|o| o.value.as_ref())
                .and_then(|v| values.get(v))
                .unwrap_or(&not_available)
                .clone()
        }));
        Self::set_tooltip(ui, dropdown, option);
        ui.set_visible_fn(dropdown, || true);
        let (net, id, disabled) = (self.net.clone(), option.id.clone(), self.configuration_disabled.clone());
        ui.button(dropdown).is_disabled = Some(Rc::new(move || {
            disabled()
                || net
                    .borrow()
                    .session
                    .global_settings
                    .lobby_options
                    .get(&id)
                    .is_some_and(|o| o.is_locked)
        }));
        let (net, id, values) = (self.net.clone(), option.id.clone(), option.values.clone());
        ui.button(dropdown).on_mouse_down = Some(Rc::new(move |ui, _| {
            let (net, id) = (net.clone(), id.clone());
            let height = values.len() as i32 * 30;
            DropDownButtonWidget::show_dropdown(
                ui,
                dropdown,
                "LABEL_DROPDOWN_TEMPLATE",
                height,
                values.clone(),
                move |ui, (key, value), template| {
                    let (n, i, k) = (net.clone(), id.clone(), key.clone());
                    let is_selected = Rc::new(move || {
                        n.borrow()
                            .session
                            .global_settings
                            .lobby_options
                            .get(&i)
                            .and_then(|o| o.value.clone())
                            == Some(k.clone())
                    });
                    let (n, i) = (net.clone(), id.clone());
                    let on_click = Rc::new(move |_: &mut Ui| n.borrow_mut().command(&format!("option {i} {key}")));
                    let item = ScrollItemWidget::setup(ui, template, is_selected, on_click);
                    let label = ui.child(item, "LABEL");
                    ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || value.clone()));
                    item
                },
            );
        }));
        let dropdown_id = ui.node(dropdown).id.clone().unwrap_or_default();
        if let Some(label) = ui.find(row, &format!("{dropdown_id}_DESC")) {
            let text = format!("{}:", option.name);
            ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || text.clone()));
            ui.set_visible_fn(label, || true);
        }
    }

    /// Port of `RebuildOptions`.
    fn rebuild(&self, ui: &mut Ui, rules: &LobbyRules) {
        ui.remove_children(self.container);
        ui.node_mut(self.container).bounds.h = 0;
        let mut options: Vec<&LobbyOption> = rules.options.iter().filter(|o| o.is_visible).collect();
        options.sort_by_key(|o| o.display_order);

        let mut columns: VecDeque<WidgetId> = VecDeque::new();
        let mut row = self.container;
        for option in options.iter().filter(|o| o.is_boolean) {
            if columns.is_empty() {
                (row, columns) = self.new_row(ui, self.checkbox_row);
            }
            let checkbox = columns.pop_front().expect("a free column");
            self.add_checkbox(ui, checkbox, option);
        }
        columns.clear();
        for option in options.iter().filter(|o| !o.is_boolean) {
            if columns.is_empty() {
                (row, columns) = self.new_row(ui, self.dropdown_row);
            }
            let dropdown = columns.pop_front().expect("a free column");
            self.add_dropdown(ui, dropdown, row, option);
        }
        let height = ui.node(self.container).bounds.h;
        ui.list(self.panel).content_height = self.y_margin + height;
        ui.node_mut(self.container).bounds.y = self.y_margin;
        ui.scroll_to_top(self.panel, false);
    }

    /// Adds a copy of a row template. The columns are the checkboxes or drop downs among its children.
    fn new_row(&self, ui: &mut Ui, template: WidgetId) -> (WidgetId, VecDeque<WidgetId>) {
        let row = ui.clone_widget(template);
        let height = ui.node(self.container).bounds.h;
        ui.node_mut(row).bounds.y = height;
        ui.node_mut(self.container).bounds.h += ui.node(row).bounds.h;
        let columns = ui
            .node(row)
            .children
            .iter()
            .copied()
            .filter(|&c| {
                let id = ui.node(c).id.clone().unwrap_or_default();
                !id.ends_with("_DESC") && ui.kind(c).borrow_mut().as_button().is_some()
            })
            .collect();
        ui.add_child(self.container, row);
        (row, columns)
    }
}

impl ChromeLogic for LobbyOptionsLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let Some((uid, rules)) = (self.get_rules)() else {
            return;
        };
        if self.current_uid.as_deref() == Some(&uid) {
            return;
        }
        self.current_uid = Some(uid);
        self.rebuild(ui, &rules);
    }

    fn disposed(&mut self, ui: &mut Ui) {
        ui.dispose_detached(self.checkbox_row);
        ui.dispose_detached(self.dropdown_row);
    }
}

pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let container = ui.child(widget, "LOBBY_OPTIONS");
    let y_margin = ui.node(container).bounds.y;
    let checkbox_template = ui.child(container, "CHECKBOX_ROW_TEMPLATE");
    let dropdown_template = ui.child(container, "DROPDOWN_ROW_TEMPLATE");
    let checkbox_row = ui.clone_widget(checkbox_template);
    let dropdown_row = ui.clone_widget(dropdown_template);
    cell(LobbyOptionsLogic {
        panel: widget,
        container,
        checkbox_row,
        dropdown_row,
        y_margin,
        net: services(ui).net.clone(),
        get_rules: args.value::<GetRules>("getRules").unwrap_or_else(|| Rc::new(|| None)),
        configuration_disabled: args
            .value::<Getter<bool>>("configurationDisabled")
            .unwrap_or_else(|| Rc::new(|| true)),
        current_uid: None,
    })
}
