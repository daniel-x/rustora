// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `ShowColorDropDown` of `ColorPickerManager.cs`. The chooser panel follows in the next step.

use std::rc::Rc;

use rustora_core::fieldloader::Color;

use crate::widget::{Ui, WidgetId};

/// The `onExit` argument.
pub type OnColorChosen = Rc<dyn Fn(&mut Ui, Color)>;

pub fn show_color_dropdown(_ui: &mut Ui, _dropdown: WidgetId, _initial: Color, _on_exit: OnColorChosen) {}
