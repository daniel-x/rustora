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
mod checkbox;
mod color_block;
mod color_mixer;
mod container;
pub mod control_groups;
mod dropdown;
mod image;
mod label;
mod map_preview;
pub mod menu_button;
pub mod production;
pub mod radar;
mod scroll_item;
mod scroll_panel;
mod slider;
pub mod support_powers;
mod text_field;
mod tooltip;
mod viewport_controller;
pub mod world_interaction;

pub use background::BackgroundWidget;
pub use button::{ButtonWidget, draw_background};
pub use checkbox::CheckboxWidget;
pub use color_block::{ClientTooltipRegionWidget, ColorBlockWidget};
pub use color_mixer::{ColorMixerWidget, HueSliderWidget};
pub use container::ContainerWidget;
pub use dropdown::{DropDownButtonWidget, MaskWidget};
pub use image::ImageWidget;
pub use label::{LabelWidget, LabelWithTooltipWidget, TextAlign, TextVAlign};
pub use map_preview::{MapPreviewWidget, SpawnOccupant};
pub use scroll_item::ScrollItemWidget;
pub use scroll_panel::ScrollPanelWidget;
pub use slider::SliderWidget;
pub use text_field::{TextFieldType, TextFieldWidget};
pub use tooltip::TooltipContainerWidget;
pub use viewport_controller::ViewportControllerWidget;

pub fn container() -> Rc<RefCell<dyn WidgetKind>> {
    cell(ContainerWidget::default())
}

/// Port of `ObjectCreator.CreateObject<Widget>`.
pub fn create(type_name: &str, assets: &Assets) -> Option<Rc<RefCell<dyn WidgetKind>>> {
    Some(match type_name {
        "Container" | "LogicKeyListener" => container(),
        "LogicTicker" => cell(production::LogicTickerWidget::default()),
        "ProductionTypeButton" => cell(production::ProductionTypeButtonWidget::new(assets)),
        "SupportPowers" => cell(support_powers::SupportPowersWidget::default()),
        "ControlGroups" => cell(control_groups::ControlGroupsWidget::default()),
        "MenuButton" => cell(menu_button::MenuButtonWidget::new(assets)),
        "Radar" => cell(radar::RadarWidget::default()),
        "ProductionPalette" => cell(production::ProductionPaletteWidget::new(assets)),
        "Background" => cell(BackgroundWidget::default()),
        "Label" | "LabelForInput" => cell(LabelWidget::new(assets)),
        "ColorBlock" => cell(ColorBlockWidget::default()),
        "ClientTooltipRegion" => cell(ClientTooltipRegionWidget::default()),
        "LabelWithTooltip" => cell(LabelWithTooltipWidget::new(assets)),
        "MapPreview" => cell(MapPreviewWidget::new(assets)),
        "Button" => cell(ButtonWidget::new(assets)),
        "Image" => cell(ImageWidget::default()),
        "ScrollPanel" => cell(ScrollPanelWidget::new(assets)),
        "ScrollItem" => cell(ScrollItemWidget::new(assets)),
        "DropDownButton" => cell(DropDownButtonWidget::new(assets)),
        "Checkbox" => cell(CheckboxWidget::new(assets)),
        "ViewportController" => cell(ViewportControllerWidget::default()),
        "WorldInteractionController" => cell(world_interaction::WorldInteractionControllerWidget::new(assets)),
        "Slider" => cell(SliderWidget::default()),
        "HueSlider" => cell(HueSliderWidget::default()),
        "ColorMixer" => cell(ColorMixerWidget::default()),
        "TextField" => cell(TextFieldWidget::new(assets, false)),
        "PasswordField" => cell(TextFieldWidget::new(assets, true)),
        "TooltipContainer" => cell(TooltipContainerWidget::default()),
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

        fn clone_kind(&self) -> std::rc::Rc<std::cell::RefCell<dyn crate::widget::WidgetKind>> {
            std::rc::Rc::new(std::cell::RefCell::new(self.clone()))
        }
    };
}
pub(crate) use impl_as_any;

#[cfg(test)]
mod tests;
