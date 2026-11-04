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
