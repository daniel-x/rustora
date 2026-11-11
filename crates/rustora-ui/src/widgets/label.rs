// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::rc::Rc;

use rustora_core::fieldloader::Color;

use super::{TooltipContainerWidget, impl_as_any, parse_bool};
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::input::{MouseButton, MouseEvent, MouseInput};
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
    /// `LabelWithHighlightWidget.HighlightColor`: text between `<` and `>` is drawn in this color.
    pub highlight_color: Option<Color>,
    /// `LabelForInputWidget.For`: the sibling input whose disabled state selects `text_disabled_color`.
    pub for_input: Option<String>,
    pub text_disabled_color: Color,
    /// Links in the text are underlined in this color and open in the browser on a click.
    pub link_color: Option<Color>,
    /// The screen rectangles of the links of the last draw, with their addresses.
    pub(crate) link_hits: Vec<(Rect, String)>,
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
            highlight_color: None,
            for_input: None,
            text_disabled_color: m.color("TextDisabledColor"),
            link_color: None,
            link_hits: Vec::new(),
        }
    }
}

impl LabelWidget {
    /// Draws the text line by line, with the links underlined in `link_color`, and keeps where the links are.
    fn draw_with_links(&mut self, dc: &mut DrawContext, text: &str, origin: (i32, i32), color: Color, link_color: Color, style: TextStyle) {
        self.link_hits.clear();
        let line_height = dc.measure(&self.font, "").1;
        let top = origin.1 + dc.font_top_offset(&self.font);
        for (row, line) in text.split('\n').enumerate() {
            let y = origin.1 + row as i32 * line_height;
            let mut x = origin.0;
            for (part, link) in crate::links::link_components(line) {
                let width = dc.measure(&self.font, part).0;
                match link {
                    Some(address) => {
                        dc.draw_text(&self.font, part, x, y, link_color, style);
                        let line_top = top + row as i32 * line_height;
                        dc.fill_rect_with_color(Rect::new(x, line_top + line_height - 2, width, 1), link_color);
                        self.link_hits.push((Rect::new(x, line_top, width, line_height), address));
                    }
                    None => dc.draw_text(&self.font, part, x, y, color, style),
                }
                x += width;
            }
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
            "HighlightColor" if self.highlight_color.is_some() => self.highlight_color = Color::parse(value),
            "For" if self.for_input.is_some() => self.for_input = Some(value.to_string()),
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
        let mut color = self.get_color.as_ref().map_or(self.text_color, |f| f());
        if let Some(name) = self.for_input.as_deref().filter(|n| !n.is_empty()) {
            let input = ui.node(id).parent.and_then(|p| ui.find(p, name));
            if input.is_some_and(|i| ui.kind(i).borrow_mut().input_disabled() == Some(true)) {
                color = self.text_disabled_color;
            }
        }
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
        match (self.link_color, self.highlight_color) {
            (Some(link_color), _) => self.draw_with_links(dc, &text, (x, y), color, link_color, style),
            (None, Some(highlight)) => {
                for (part, highlighted) in highlight_components(&text) {
                    dc.draw_text(&self.font, part, x, y, if highlighted { highlight } else { color }, style);
                    x += dc.measure(&self.font, part).0;
                }
            }
            (None, None) => dc.draw_text(&self.font, &text, x, y, color, style),
        }
    }

    fn handle_mouse(&mut self, ui: &mut Ui, _id: WidgetId, mi: &MouseInput) -> bool {
        if mi.event != MouseEvent::Down || mi.button != MouseButton::Left {
            return false;
        }
        let Some((_, address)) = self.link_hits.iter().find(|(r, _)| r.contains(mi.x, mi.y)) else {
            return false;
        };
        ui.requests.push(crate::widget::AppRequest::OpenUrl(address.clone()));
        true
    }

    fn cursor_at_point(&self, default: &str, x: i32, y: i32) -> Option<String> {
        if self.link_hits.iter().any(|(r, _)| r.contains(x, y)) {
            Some(crate::cursor::LINK_CURSOR.to_string())
        } else {
            self.cursor(default)
        }
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        None
    }
}

/// Port of `LabelWithHighlightWidget.MakeComponents`: splits the text into parts and marks the parts between `<` and
/// `>`.
fn highlight_components(text: &str) -> Vec<(&str, bool)> {
    let mut components = Vec::new();
    for mut line in text.split("\\n") {
        while !line.is_empty() {
            match (line.find('<'), line.find('>')) {
                (Some(start), Some(end)) if start > 0 && end > start => {
                    components.push((&line[..start], false));
                    components.push((&line[start + 1..end], true));
                    line = &line[end + 1..];
                }
                _ => {
                    components.push((line, false));
                    break;
                }
            }
        }
    }
    components
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
