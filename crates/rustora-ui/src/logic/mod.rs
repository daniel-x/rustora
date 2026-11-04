// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of the logic classes of `OpenRA.Mods.Common/Widgets/Logic`.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_core::miniyaml::MiniYamlNode;

use crate::loader::cell;
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::LabelWidget;

pub mod main_menu;

/// The application state that logic classes read.
#[derive(Default)]
pub struct Services {
    pub mod_version: String,
}

fn services(ui: &Ui) -> Rc<Services> {
    ui.services.clone().downcast::<Services>().expect("logic services")
}

/// A logic class without per frame state.
pub struct NoLogic;
impl ChromeLogic for NoLogic {}

/// Port of `ButtonTooltipLogic`. The hotkey label stays hidden until hotkeys are ported.
fn button_tooltip(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let label = ui.child(widget, "LABEL");
    let font = ui.get::<LabelWidget>(label).font.clone();
    let text: String = args.value("text").unwrap_or_default();
    let label_width = ui.measure(&font, &text).0;
    let shown = text.clone();
    ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || shown.clone()));
    ui.node_mut(label).bounds.w = label_width;
    let label_x = ui.node(label).bounds.x;
    ui.node_mut(widget).bounds.w = 2 * label_x + label_width;

    let desc: String = args.value("desc").unwrap_or_default();
    if !desc.is_empty() {
        let template = ui.child(widget, "DESC");
        let parent = ui.node(template).parent.expect("description parent");
        ui.node_mut(parent).children.retain(|&c| c != template);
        let desc_font = ui.get::<LabelWidget>(template).font.clone();
        let template_bounds = ui.node(template).bounds;
        let mut desc_width = 0;
        let mut desc_offset = template_bounds.y;
        for line in desc.split('\n') {
            desc_width = desc_width.max(ui.measure(&desc_font, line).0);
            let line_label = ui.clone_widget(template);
            let line = line.to_string();
            ui.get::<LabelWidget>(line_label).get_text = Some(Rc::new(move || line.clone()));
            ui.node_mut(line_label).bounds.y = desc_offset;
            ui.add_child(widget, line_label);
            desc_offset += template_bounds.h;
        }
        ui.dispose_detached(template);
        let bounds = &mut ui.node_mut(widget).bounds;
        bounds.w = bounds.w.max(template_bounds.x * 2 + desc_width);
        bounds.h += desc_offset - template_bounds.y + template_bounds.x;
    }
    cell(NoLogic)
}

/// Port of `ObjectCreator.CreateObject<ChromeLogic>`.
pub fn create(
    name: &str,
    ui: &mut Ui,
    id: WidgetId,
    args: &WidgetArgs,
    _logic_args: &[MiniYamlNode],
) -> Option<Rc<RefCell<dyn ChromeLogic>>> {
    Some(match name {
        "MainMenuLogic" => main_menu::create(ui, id, args),
        "VersionLabelLogic" => {
            let version = services(ui).mod_version.clone();
            ui.get::<LabelWidget>(id).get_text = Some(Rc::new(move || version.clone()));
            cell(NoLogic)
        }
        "ButtonTooltipLogic" => button_tooltip(ui, id, args),
        "PerfDebugLogic" => {
            for name in ["GRAPH_BG", "PERF_TEXT"] {
                let widget = ui.child(id, name);
                ui.set_visible_fn(widget, || false);
            }
            cell(NoLogic)
        }
        _ => return None,
    })
}
