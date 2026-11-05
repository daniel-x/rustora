// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use rustora_core::fieldloader::Color;

use super::button::stateful_image_name;
use super::{impl_as_any, parse_bool};
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::input::{KeyEvent, KeyInput, MouseEvent, MouseInput};
use crate::widget::{Action, Getter, KeyCallback, Ui, WidgetId, WidgetKind};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextFieldType {
    General,
    Filename,
    Integer,
}

/// `Path.GetInvalidFileNameChars` differs by system, as in .NET.
fn is_invalid_filename_char(c: char) -> bool {
    if cfg!(windows) {
        (c as u32) < 32 || matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/')
    } else {
        c == '\0' || c == '/'
    }
}

/// Port of `TextFieldWidget` and, with `password` set, `PasswordFieldWidget`.
#[derive(Clone)]
pub struct TextFieldWidget {
    text: Vec<char>,
    pub password: bool,
    pub max_length: usize,
    pub visual_height: i32,
    pub left_margin: i32,
    pub right_margin: i32,
    pub background: String,
    field_type: TextFieldType,
    pub on_enter_key: Option<KeyCallback>,
    pub on_tab_key: Option<KeyCallback>,
    pub on_esc_key: Option<KeyCallback>,
    pub on_alt_key: Option<KeyCallback>,
    pub on_lose_focus: Option<Action>,
    pub on_text_edited: Option<Action>,
    pub cursor_position: usize,
    pub is_valid: Option<Getter<bool>>,
    pub disabled: bool,
    pub is_disabled: Option<Getter<bool>>,
    pub font: String,
    pub text_color: Color,
    pub text_color_disabled: Color,
    pub text_color_invalid: Color,
    pub text_color_highlight: Color,
    selection: Option<(usize, usize)>,
    mouse_selection_active: bool,
    blink_cycle: i32,
    show_cursor: bool,
    was_disabled: bool,
}

impl TextFieldWidget {
    pub fn new(assets: &Assets, password: bool) -> TextFieldWidget {
        let m = &assets.metrics;
        TextFieldWidget {
            text: Vec::new(),
            password,
            max_length: 0,
            visual_height: 1,
            left_margin: 5,
            right_margin: 5,
            background: "textfield".into(),
            field_type: TextFieldType::General,
            on_enter_key: None,
            on_tab_key: None,
            on_esc_key: None,
            on_alt_key: None,
            on_lose_focus: None,
            on_text_edited: None,
            cursor_position: 0,
            is_valid: None,
            disabled: false,
            is_disabled: None,
            font: m.string("TextfieldFont"),
            text_color: m.color("TextfieldColor"),
            text_color_disabled: m.color("TextfieldColorDisabled"),
            text_color_invalid: m.color("TextfieldColorInvalid"),
            text_color_highlight: m.color("TextfieldColorHighlight"),
            selection: None,
            mouse_selection_active: false,
            blink_cycle: 10,
            show_cursor: true,
            was_disabled: false,
        }
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    /// The `Text` setter.
    pub fn set_text(&mut self, value: &str) {
        self.text = self.remove_invalid_characters(value);
        self.cursor_position = self.cursor_position.min(self.text.len());
        self.selection = None;
    }

    /// The `Type` setter.
    pub fn set_type(&mut self, field_type: TextFieldType) {
        self.field_type = field_type;
        let text = self.text();
        self.text = self.remove_invalid_characters(&text);
        self.cursor_position = self.cursor_position.min(self.text.len());
    }

    fn remove_invalid_characters(&self, input: &str) -> Vec<char> {
        input
            .chars()
            .filter(|&c| match self.field_type {
                TextFieldType::Filename => !is_invalid_filename_char(c),
                TextFieldType::Integer => c.is_numeric(),
                TextFieldType::General => true,
            })
            .collect()
    }

    fn apparent(&self, end: usize) -> String {
        if self.password {
            "*".repeat(end)
        } else {
            self.text[..end].iter().collect()
        }
    }

    pub fn disabled_now(&self) -> bool {
        self.is_disabled.as_ref().map_or(self.disabled, |f| f())
    }

    fn reset_blink_cycle(&mut self) {
        self.blink_cycle = 10;
        self.show_cursor = true;
    }

    fn edited(&self, ui: &mut Ui) {
        if let Some(action) = self.on_text_edited.clone() {
            ui.defer(move |ui| action(ui));
        }
    }

    /// Port of `ClosestCursorPosition`.
    fn closest_cursor_position(&self, ui: &Ui, id: WidgetId, x: i32) -> usize {
        let text_width = ui.measure(&self.font, &self.apparent(self.text.len())).0;
        let bounds = ui.node(id).bounds;
        let usable = bounds.w - self.left_margin - self.right_margin;
        let mut start = ui.render_origin(id).0 + self.left_margin;
        if text_width > usable && ui.has_keyboard_focus(id) {
            start += usable - text_width;
        }
        let mut closest = 0;
        let mut min_value = i32::MAX;
        for i in 0..=self.text.len() {
            let distance = (start + ui.measure(&self.font, &self.apparent(i)).0 - x).abs();
            if distance > min_value {
                break;
            }
            min_value = distance;
            closest = i;
        }
        closest
    }

    /// Port of `GetPrevWhitespaceIndex`.
    fn prev_whitespace_index(&self) -> usize {
        let mut end = self.cursor_position;
        while end > 0 && self.text[end - 1].is_whitespace() {
            end -= 1;
        }
        self.text[..end].iter().rposition(|&c| c == ' ').map_or(0, |p| p + 1)
    }

    /// Port of `GetNextWhitespaceIndex`.
    fn next_whitespace_index(&self) -> usize {
        let rest = &self.text[self.cursor_position..];
        let trimmed = rest.iter().take_while(|c| c.is_whitespace()).count();
        match rest[trimmed..].iter().position(|&c| c == ' ') {
            Some(next) => self.cursor_position + trimmed + next,
            None => self.text.len(),
        }
    }

    fn handle_selection_update(&mut self, previous: usize, new: usize) {
        let start = self.selection.map_or(previous, |s| s.0);
        self.selection = (start != new).then_some((start, new));
    }

    fn selected_range(&self) -> Option<(usize, usize)> {
        self.selection.map(|(a, b)| (a.min(b), a.max(b)))
    }

    fn remove_selected_text(&mut self, ui: &mut Ui) {
        if let Some((low, high)) = self.selected_range() {
            self.text.drain(low..high);
            self.selection = None;
            self.cursor_position = low;
            self.edited(ui);
        }
    }

    fn copy_selection(&self, ui: &Ui) {
        if let Some((low, high)) = self.selected_range() {
            let text: String = self.text[low..high].iter().collect();
            ui.clipboard.set(&text);
        }
    }

    /// Whether the modifier that moves by word on this system is held.
    fn word_modifier(e: &KeyInput) -> bool {
        if cfg!(target_os = "macos") {
            e.modifiers.alt
        } else {
            e.modifiers.ctrl
        }
    }

    /// Whether the modifier of clipboard commands on this system is held.
    fn command_modifier(e: &KeyInput) -> bool {
        if cfg!(target_os = "macos") {
            e.modifiers.meta
        } else {
            e.modifiers.ctrl
        }
    }

    /// Port of `HandleTextInput`.
    pub fn insert_text(&mut self, ui: &mut Ui, id: WidgetId, input: &str) -> bool {
        if !ui.has_keyboard_focus(id) || self.disabled_now() {
            return false;
        }
        let input = self.remove_invalid_characters(input);
        if input.is_empty() {
            return true;
        }
        self.remove_selected_text(ui);
        if self.max_length > 0 && self.text.len() >= self.max_length {
            return true;
        }
        let length = if self.max_length > 0 {
            input.len().min(self.max_length - self.text.len())
        } else {
            input.len()
        };
        let at = self.cursor_position;
        self.text.splice(at..at, input[..length].iter().copied());
        self.cursor_position += length;
        self.selection = None;
        self.edited(ui);
        true
    }
}

impl WidgetKind for TextFieldWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        let color = |current: Color| Color::parse(value).unwrap_or(current);
        match key {
            "Text" => self.set_text(value),
            "MaxLength" => self.max_length = value.parse().unwrap_or(self.max_length),
            "VisualHeight" => self.visual_height = value.parse().unwrap_or(self.visual_height),
            "LeftMargin" => self.left_margin = value.parse().unwrap_or(self.left_margin),
            "RightMargin" => self.right_margin = value.parse().unwrap_or(self.right_margin),
            "Background" => self.background = value.to_string(),
            "Type" => self.set_type(match value {
                "Filename" => TextFieldType::Filename,
                "Integer" => TextFieldType::Integer,
                _ => TextFieldType::General,
            }),
            "Font" => self.font = value.to_string(),
            "TextColor" => self.text_color = color(self.text_color),
            "TextColorDisabled" => self.text_color_disabled = color(self.text_color_disabled),
            "TextColorInvalid" => self.text_color_invalid = color(self.text_color_invalid),
            "TextColorHighlight" => self.text_color_highlight = color(self.text_color_highlight),
            "Disabled" => self.disabled = parse_bool(value),
            _ => return false,
        }
        true
    }

    fn yield_keyboard_focus(&mut self, ui: &mut Ui, id: WidgetId) -> bool {
        if let Some(action) = self.on_lose_focus.clone() {
            ui.defer(move |ui| action(ui));
        }
        if ui.keyboard_focus == Some(id) {
            ui.keyboard_focus = None;
        }
        true
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        if self.disabled_now() {
            return false;
        }
        if self.mouse_selection_active {
            if mi.event == MouseEvent::Up {
                self.mouse_selection_active = false;
                return true;
            }
            if mi.event != MouseEvent::Move {
                return false;
            }
        } else if mi.event != MouseEvent::Down {
            return false;
        }
        if !ui.render_bounds(id).contains(mi.x, mi.y) || !ui.take_keyboard_focus(id) {
            return false;
        }
        self.mouse_selection_active = true;
        self.reset_blink_cycle();
        let previous = self.cursor_position;
        self.cursor_position = self.closest_cursor_position(ui, id, mi.x);
        if mi.modifiers.shift || mi.event == MouseEvent::Move {
            self.handle_selection_update(previous, self.cursor_position);
        } else {
            self.selection = None;
        }
        true
    }

    fn key_callback(&mut self, ui: &Ui, id: WidgetId, e: &KeyInput) -> Option<KeyCallback> {
        if self.disabled_now() || e.event == KeyEvent::Up || !ui.has_keyboard_focus(id) {
            return None;
        }
        match e.key.as_str() {
            "RETURN" | "KP_ENTER" => self.on_enter_key.clone(),
            "TAB" => self.on_tab_key.clone(),
            "ESCAPE" => {
                self.selection = None;
                self.on_esc_key.clone()
            }
            "LALT" => self.on_alt_key.clone(),
            _ => None,
        }
    }

    fn handle_key(&mut self, ui: &mut Ui, id: WidgetId, e: &KeyInput) -> bool {
        if self.disabled_now() || e.event == KeyEvent::Up || !ui.has_keyboard_focus(id) {
            return false;
        }
        let is_mac = cfg!(target_os = "macos");
        let shift = e.modifiers.shift;
        let meta = e.modifiers.meta;
        let word = Self::word_modifier(e);
        let command = Self::command_modifier(e);
        let length = self.text.len();
        match e.key.as_str() {
            "LEFT" => {
                self.reset_blink_cycle();
                if self.cursor_position > 0 {
                    let previous = self.cursor_position;
                    self.cursor_position = if word {
                        self.prev_whitespace_index()
                    } else if is_mac && meta {
                        0
                    } else {
                        previous - 1
                    };
                    if shift {
                        self.handle_selection_update(previous, self.cursor_position);
                    } else {
                        self.selection = None;
                    }
                }
            }
            "RIGHT" => {
                self.reset_blink_cycle();
                if self.cursor_position < length {
                    let previous = self.cursor_position;
                    self.cursor_position = if word {
                        self.next_whitespace_index()
                    } else if is_mac && meta {
                        length
                    } else {
                        previous + 1
                    };
                    if shift {
                        self.handle_selection_update(previous, self.cursor_position);
                    } else {
                        self.selection = None;
                    }
                }
            }
            "HOME" => {
                self.reset_blink_cycle();
                if shift {
                    self.handle_selection_update(self.cursor_position, 0);
                } else {
                    self.selection = None;
                }
                self.cursor_position = 0;
            }
            "END" => {
                self.reset_blink_cycle();
                if shift {
                    self.handle_selection_update(self.cursor_position, length);
                } else {
                    self.selection = None;
                }
                self.cursor_position = length;
            }
            "D" => {
                if e.modifiers.ctrl && self.cursor_position < length {
                    self.text.remove(self.cursor_position);
                    self.edited(ui);
                }
            }
            "K" => {
                self.reset_blink_cycle();
                if e.modifiers.ctrl && self.cursor_position < length {
                    self.text.truncate(self.cursor_position);
                    self.edited(ui);
                }
            }
            "U" => {
                self.reset_blink_cycle();
                if !is_mac && e.modifiers.ctrl && self.cursor_position > 0 {
                    self.text.drain(..self.cursor_position);
                    self.cursor_position = 0;
                    self.selection = None;
                    self.edited(ui);
                }
            }
            "X" => {
                self.reset_blink_cycle();
                if command && self.selection.is_some() {
                    self.copy_selection(ui);
                    self.remove_selected_text(ui);
                }
            }
            "C" => {
                self.reset_blink_cycle();
                if command {
                    self.copy_selection(ui);
                }
            }
            "DELETE" => {
                self.reset_blink_cycle();
                if self.selection.is_some() {
                    self.remove_selected_text(ui);
                } else if self.cursor_position < length {
                    if word {
                        let next = self.next_whitespace_index();
                        self.text.drain(self.cursor_position..next);
                    } else if is_mac && meta {
                        self.text.truncate(self.cursor_position);
                    } else {
                        self.text.remove(self.cursor_position);
                    }
                    self.edited(ui);
                }
            }
            "BACKSPACE" => {
                self.reset_blink_cycle();
                if self.selection.is_some() {
                    self.remove_selected_text(ui);
                } else if self.cursor_position > 0 {
                    if word {
                        let previous = self.prev_whitespace_index();
                        self.text.drain(previous..self.cursor_position);
                        self.cursor_position = previous;
                    } else if is_mac && meta {
                        self.text.drain(..self.cursor_position);
                        self.cursor_position = 0;
                    } else {
                        self.cursor_position -= 1;
                        self.text.remove(self.cursor_position);
                    }
                    self.edited(ui);
                }
            }
            "V" => {
                self.reset_blink_cycle();
                self.remove_selected_text(ui);
                if command {
                    let clipboard = ui.clipboard.get();
                    let first_line = match clipboard.find('\n') {
                        Some(nl) if nl > 0 => &clipboard[..nl],
                        _ => clipboard.as_str(),
                    };
                    let trimmed = first_line.trim();
                    if !trimmed.is_empty() {
                        self.insert_text(ui, id, trimmed);
                    }
                }
            }
            "A" if command => {
                self.selection = None;
                self.handle_selection_update(0, length);
            }
            _ => {}
        }
        true
    }

    fn handle_text(&mut self, ui: &mut Ui, id: WidgetId, text: &str) -> bool {
        self.insert_text(ui, id, text)
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        let disabled = self.disabled_now();
        if disabled != self.was_disabled {
            self.was_disabled = disabled;
            if disabled && ui.keyboard_focus == Some(id) {
                self.yield_keyboard_focus(ui, id);
            }
        }
        self.blink_cycle -= 1;
        if self.blink_cycle <= 0 {
            self.blink_cycle = 20;
            self.show_cursor = !self.show_cursor;
        }
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let text = self.apparent(self.text.len());
        let (x, y) = ui.render_origin(id);
        let bounds = ui.node(id).bounds;
        let (text_w, text_h) = dc.measure(&self.font, &text);
        let cursor_x = dc.measure(&self.font, &self.apparent(self.cursor_position)).0;
        let disabled = self.disabled_now();
        let hover = ui.mouse_over == Some(id) || ui.node(id).children.iter().any(|&c| ui.mouse_over == Some(c));
        let focus = ui.has_keyboard_focus(id);
        let state = stateful_image_name(&self.background, disabled, false, hover, focus);
        dc.draw_panel(&state, Rect::new(x, y, bounds.w, bounds.h));

        let vertical_margin = (bounds.h - text_h) / 2 - self.visual_height;
        let mut text_x = x + self.left_margin;
        let text_y = y + vertical_margin;
        let usable = bounds.w - self.left_margin - self.right_margin;
        let overflowing = text_w > usable;
        if overflowing {
            if focus {
                text_x += usable - text_w;
            }
            dc.push_clip(Rect::new(x + self.left_margin, y, usable, bounds.bottom()));
        }
        if let Some((low, high)) = self.selected_range() {
            let start = dc.measure(&self.font, &self.apparent(low)).0;
            let end = dc.measure(&self.font, &self.apparent(high)).0;
            let rect = Rect::new(text_x + start, text_y, end - start, bounds.h - vertical_margin * 2);
            dc.fill_rect_with_color(rect, self.text_color_highlight);
        }
        let color = if disabled {
            self.text_color_disabled
        } else if self.is_valid.as_ref().is_none_or(|f| f()) {
            self.text_color
        } else {
            self.text_color_invalid
        };
        dc.draw_text(&self.font, &text, text_x, text_y, color, TextStyle::Plain);
        if self.show_cursor && focus {
            dc.draw_text(&self.font, "|", text_x + cursor_x - 2, text_y, self.text_color, TextStyle::Plain);
        }
        if overflowing {
            dc.pop_clip();
        }
    }
}
