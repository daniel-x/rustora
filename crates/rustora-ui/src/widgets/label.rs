// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::rc::Rc;

use rustora_core::fieldloader::Color;

use super::{TooltipContainerWidget, impl_as_any, parse_bool};
use crate::assets::Assets;
use crate::draw::{DrawContext, TextStyle};
use crate::widget::{Getter, Ui, WidgetArgs, WidgetId, WidgetKind};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextVAlign {
    Top,
    Middle,
    Bottom,
}

pub(crate) fn parse_align(value: &str) -> TextAlign {
    match value {
        "Center" => TextAlign::Center,
        "Right" => TextAlign::Right,
        _ => TextAlign::Left,
    }
}

/// Port of `LabelWidget`.
#[derive(Clone)]
pub struct LabelWidget {
    pub text: Option<String>,
    pub resolved: String,
    pub align: TextAlign,
    pub valign: TextVAlign,
    pub font: String,
    pub text_color: Color,
    pub contrast: bool,
    pub shadow: bool,
    pub contrast_dark: Color,
    pub contrast_light: Color,
    pub contrast_radius: i32,
    pub word_wrap: bool,
    pub get_text: Option<Getter<String>>,
    pub get_color: Option<Getter<Color>>,
}

impl LabelWidget {
    /// Port of `IncreaseHeightToFitCurrentText`.
    pub fn increase_height_to_fit_current_text(ui: &mut Ui, id: WidgetId) {
        let width = ui.node(id).bounds.w;
        let (get_text, key, font, word_wrap) = {
            let label = ui.label(id);
            (label.get_text.clone(), label.text.clone(), label.font.clone(), label.word_wrap)
        };
        let text = match get_text {
            Some(f) => f(),
            None => ui.assets.borrow().messages.get(key.as_deref().unwrap_or_default()),
        };
        let line = if word_wrap {
            ui.assets.borrow_mut().wrap_text(&font, &text, width)
        } else {
            text
        };
        let height = ui.measure(&font, &line).1;
        let bounds = &mut ui.node_mut(id).bounds;
        bounds.h = bounds.h.max(height);
    }

    pub fn new(assets: &Assets) -> LabelWidget {
        let m = &assets.metrics;
        LabelWidget {
            text: None,
            resolved: String::new(),
            align: TextAlign::Left,
            valign: TextVAlign::Middle,
            font: m.string("TextFont"),
            text_color: m.color("TextColor"),
            contrast: m.bool("TextContrast"),
            shadow: m.bool("TextShadow"),
            contrast_dark: m.color("TextContrastColorDark"),
            contrast_light: m.color("TextContrastColorLight"),
            contrast_radius: m.int("TextContrastRadius"),
            word_wrap: false,
            get_text: None,
            get_color: None,
        }
    }
}

impl WidgetKind for LabelWidget {
    impl_as_any!();

    fn as_label(&mut self) -> Option<&mut LabelWidget> {
        Some(self)
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "Text" => self.text = Some(value.to_string()),
            "Align" => self.align = parse_align(value),
            "VAlign" => {
                self.valign = match value {
                    "Top" => TextVAlign::Top,
                    "Bottom" => TextVAlign::Bottom,
                    _ => TextVAlign::Middle,
                }
            }
            "Font" => self.font = value.to_string(),
            "TextColor" => self.text_color = Color::parse(value).unwrap_or(self.text_color),
            "Contrast" => self.contrast = parse_bool(value),
            "Shadow" => self.shadow = parse_bool(value),
            "ContrastColorDark" => self.contrast_dark = Color::parse(value).unwrap_or(self.contrast_dark),
            "ContrastColorLight" => self.contrast_light = Color::parse(value).unwrap_or(self.contrast_light),
            "ContrastRadius" => self.contrast_radius = value.parse().unwrap_or(self.contrast_radius),
            "WordWrap" => self.word_wrap = parse_bool(value),
            _ => return false,
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let text = match &self.get_text {
            Some(f) => f(),
            None => {
                if self.resolved.is_empty()
                    && let Some(key) = self.text.as_deref().filter(|k| !k.is_empty())
                {
                    self.resolved = dc.assets.messages.get(key);
                }
                self.resolved.clone()
            }
        };
        let bounds = ui.node(id).bounds;
        let text = if self.word_wrap {
            dc.wrap_text(&self.font, &text, bounds.w)
        } else {
            text
        };
        let (tw, th) = dc.measure(&self.font, &text);
        let offset = dc.font_top_offset(&self.font);
        let (mut x, mut y) = ui.render_origin(id);
        match self.valign {
            TextVAlign::Top => y -= offset,
            TextVAlign::Middle => y += (bounds.h - th - offset) / 2,
            TextVAlign::Bottom => y += bounds.h - th,
        }
        match self.align {
            TextAlign::Left => {}
            TextAlign::Center => x += (bounds.w - tw) / 2,
            TextAlign::Right => x += bounds.w - tw,
        }
        let color = self.get_color.as_ref().map_or(self.text_color, |f| f());
        let style = if self.contrast {
            TextStyle::Contrast {
                dark: self.contrast_dark,
                light: self.contrast_light,
                radius: self.contrast_radius,
            }
        } else if self.shadow {
            TextStyle::Shadow {
                dark: self.contrast_dark,
                light: self.contrast_light,
                offset: 1,
            }
        } else {
            TextStyle::Plain
        };
        dc.draw_text(&self.font, &text, x, y, color, style);
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        None
    }
}

/// Port of `LabelWithTooltipWidget`.
#[derive(Clone)]
pub struct LabelWithTooltipWidget {
    pub label: LabelWidget,
    pub tooltip_template: String,
    pub tooltip_container: Option<String>,
    pub get_tooltip_text: Option<Getter<String>>,
}

impl LabelWithTooltipWidget {
    pub fn new(assets: &Assets) -> LabelWithTooltipWidget {
        LabelWithTooltipWidget {
            label: LabelWidget::new(assets),
            tooltip_template: String::new(),
            tooltip_container: None,
            get_tooltip_text: Some(Rc::new(String::new)),
        }
    }

    /// Port of `WidgetUtils.TruncateLabelToTooltip`.
    pub fn truncate_to_tooltip(ui: &mut Ui, id: WidgetId, text: &str) {
        let width = ui.node(id).bounds.w;
        let font = ui.label(id).font.clone();
        let truncated = ui.truncate_text(&font, text, width);
        let mut widget = ui.get::<LabelWithTooltipWidget>(id);
        let tooltip = (truncated != text).then(|| text.to_string());
        widget.label.get_text = Some(Rc::new(move || truncated.clone()));
        widget.get_tooltip_text = tooltip.map(|t| -> Getter<String> { Rc::new(move || t.clone()) });
    }
}

impl WidgetKind for LabelWithTooltipWidget {
    impl_as_any!();

    fn as_label(&mut self) -> Option<&mut LabelWidget> {
        Some(&mut self.label)
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "TooltipTemplate" => self.tooltip_template = value.to_string(),
            "TooltipContainer" => self.tooltip_container = Some(value.to_string()),
            _ => return self.label.set_field(key, value),
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        self.label.draw(ui, id, dc);
    }

    fn ignores_mouse_over(&self) -> bool {
        self.label.ignores_mouse_over()
    }

    fn cursor(&self, default: &str) -> Option<String> {
        self.label.cursor(default)
    }

    fn mouse_entered(&mut self, ui: &mut Ui, _id: WidgetId) {
        let Some(container) = self.tooltip_container.as_deref() else {
            return;
        };
        if let Some(text) = &self.get_tooltip_text {
            let container = ui.child(ui.root, container);
            let args = WidgetArgs::default().with_value("getText", text.clone());
            TooltipContainerWidget::set_tooltip(ui, container, &self.tooltip_template, args);
        }
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        if let Some(container) = self.tooltip_container.as_deref().and_then(|c| ui.find(ui.root, c)) {
            TooltipContainerWidget::remove_tooltip(ui, container);
        }
    }
}
