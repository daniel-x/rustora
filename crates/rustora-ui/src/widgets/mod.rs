// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of the widget types of `OpenRA.Mods.Common/Widgets`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::assets::Assets;
use crate::loader::cell;
use crate::widget::WidgetKind;

mod background;
mod button;
mod container;
mod image;
mod label;

pub use background::BackgroundWidget;
pub use button::ButtonWidget;
pub use container::ContainerWidget;
pub use image::ImageWidget;
pub use label::{LabelWidget, TextAlign, TextVAlign};

pub fn container() -> Rc<RefCell<dyn WidgetKind>> {
    cell(ContainerWidget::default())
}

/// Port of `ObjectCreator.CreateObject<Widget>`.
pub fn create(type_name: &str, assets: &Assets) -> Option<Rc<RefCell<dyn WidgetKind>>> {
    Some(match type_name {
        "Container" | "LogicKeyListener" | "LogicTicker" => container(),
        "Background" => cell(BackgroundWidget::default()),
        "Label" => cell(LabelWidget::new(assets)),
        "Button" => cell(ButtonWidget::new(assets)),
        "Image" => cell(ImageWidget::default()),
        _ => return None,
    })
}

pub(crate) fn parse_bool(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
}

macro_rules! impl_as_any {
    () => {
        fn as_any(&mut self) -> &mut dyn std::any::Any {
            self
        }
    };
}
pub(crate) use impl_as_any;
