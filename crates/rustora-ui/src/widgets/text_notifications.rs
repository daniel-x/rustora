// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `TextNotificationsDisplayWidget.cs`.

use std::collections::HashMap;

use super::impl_as_any;
use crate::draw::{DrawContext, Rect};
use crate::logic::lobby::setup_text_notification;
use crate::net::{TextNotification, TextNotificationPool};
use crate::widget::{EventBounds, Ui, WidgetArgs, WidgetId, WidgetKind};

#[derive(Clone)]
pub struct TextNotificationsDisplayWidget {
    display_duration_ms: i64,
    item_spacing: i32,
    bottom_spacing: i32,
    log_length: usize,
    hide_overflow: bool,
    chat_template: String,
    system_template: String,
    mission_template: String,
    feedback_template: String,
    transients_template: String,
    templates: HashMap<TextNotificationPool, WidgetId>,
    expirations: Vec<i64>,
}

impl Default for TextNotificationsDisplayWidget {
    fn default() -> Self {
        TextNotificationsDisplayWidget {
            display_duration_ms: 0,
            item_spacing: 4,
            bottom_spacing: 0,
            log_length: 8,
            hide_overflow: true,
            chat_template: "CHAT_LINE_TEMPLATE".to_string(),
            system_template: "SYSTEM_LINE_TEMPLATE".to_string(),
            mission_template: "CHAT_LINE_TEMPLATE".to_string(),
            feedback_template: "TRANSIENT_LINE_TEMPLATE".to_string(),
            transients_template: "TRANSIENT_LINE_TEMPLATE".to_string(),
            templates: HashMap::new(),
            expirations: Vec::new(),
        }
    }
}

impl TextNotificationsDisplayWidget {
    /// `AddNotification`.
    pub fn add_notification(ui: &mut Ui, id: WidgetId, notification: &TextNotification) {
        let (template, item_spacing, bottom_spacing, duration, log_length) = {
            let w = ui.get::<TextNotificationsDisplayWidget>(id);
            let Some(&template) = w.templates.get(&notification.pool) else {
                return;
            };
            (template, w.item_spacing, w.bottom_spacing, w.display_duration_ms, w.log_length)
        };
        let line = ui.clone_widget(template);
        let bounds = ui.node(id).bounds;
        setup_text_notification(ui, line, notification, bounds.w, false);
        let height = ui.node(line).bounds.h;
        let children = ui.node(id).children.clone();
        if let Some(&last) = children.last() {
            for &c in &children {
                ui.node_mut(c).bounds.y -= height + item_spacing;
            }
            ui.node_mut(line).bounds.y = ui.node(last).bounds.bottom() + item_spacing;
        } else {
            ui.node_mut(line).bounds.y = bounds.bottom() - height - bottom_spacing;
        }
        ui.add_child(id, line);
        let expiration = ui.run_time() + duration;
        ui.get::<TextNotificationsDisplayWidget>(id).expirations.push(expiration);
        while ui.node(id).children.len() > log_length {
            Self::remove_oldest(ui, id);
        }
    }

    /// `RemoveMostRecentNotification`.
    pub fn remove_most_recent(ui: &mut Ui, id: WidgetId) {
        let children = ui.node(id).children.clone();
        let Some(&last) = children.last() else {
            return;
        };
        let height = ui.node(last).bounds.h;
        let spacing = ui.get::<TextNotificationsDisplayWidget>(id).item_spacing;
        ui.remove_child(id, last);
        ui.get::<TextNotificationsDisplayWidget>(id).expirations.pop();
        for &c in &children[..children.len() - 1] {
            ui.node_mut(c).bounds.y += height + spacing;
        }
    }

    /// `RemoveNotification`.
    fn remove_oldest(ui: &mut Ui, id: WidgetId) {
        if let Some(&first) = ui.node(id).children.first() {
            ui.remove_child(id, first);
            ui.get::<TextNotificationsDisplayWidget>(id).expirations.remove(0);
        }
    }
}

impl WidgetKind for TextNotificationsDisplayWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "DisplayDurationMs" => self.display_duration_ms = value.parse().unwrap_or(0),
            "ItemSpacing" => self.item_spacing = value.parse().unwrap_or(4),
            "BottomSpacing" => self.bottom_spacing = value.parse().unwrap_or(0),
            "LogLength" => self.log_length = value.parse().unwrap_or(8),
            "HideOverflow" => self.hide_overflow = super::parse_bool(value),
            "ChatTemplate" => self.chat_template = value.to_string(),
            "SystemTemplate" => self.system_template = value.to_string(),
            "MissionTemplate" => self.mission_template = value.to_string(),
            "FeedbackTemplate" => self.feedback_template = value.to_string(),
            "TransientsTemplate" => self.transients_template = value.to_string(),
            _ => return false,
        }
        true
    }

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        let args = WidgetArgs::default();
        for (pool, name) in [
            (TextNotificationPool::Chat, &self.chat_template),
            (TextNotificationPool::System, &self.system_template),
            (TextNotificationPool::Mission, &self.mission_template),
            (TextNotificationPool::Feedback, &self.feedback_template),
            (TextNotificationPool::Transients, &self.transients_template),
        ] {
            self.templates.insert(pool, ui.load_widget(name, None, &args));
        }
    }

    fn event_bounds(&self) -> EventBounds {
        EventBounds::Never
    }

    fn draw_outer(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) -> bool {
        let children = ui.node(id).children.clone();
        let Some(&last) = children.last() else {
            return true;
        };
        let bounds = ui.node(id).bounds;
        let last_overflows = bounds.h < ui.node(last).bounds.h;
        if last_overflows {
            if self.hide_overflow {
                let line_height = self.templates.get(&TextNotificationPool::Chat).map_or(0, |&t| {
                    let text = ui.child(t, "TEXT");
                    let font = ui.label(text).font.clone();
                    dc.measure(&font, "").1
                });
                let visible = (bounds.h - self.bottom_spacing) / line_height.max(1) * line_height;
                let (x, y) = ui.render_origin(id);
                dc.push_clip(Rect::new(x, y + bounds.h - visible, bounds.w, visible));
                ui.draw_outer(last, dc);
                dc.pop_clip();
            } else {
                ui.draw_outer(last, dc);
            }
            return true;
        }
        for &c in children.iter().rev() {
            let child = ui.node(c).bounds;
            if !self.hide_overflow || (child.y >= 0 && child.bottom() <= bounds.h + bounds.y) {
                ui.draw_outer(c, dc);
            }
        }
        true
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        if self.display_duration_ms == 0 {
            return;
        }
        while !self.expirations.is_empty() && ui.run_time() >= self.expirations[0] {
            self.expirations.remove(0);
            if let Some(&first) = ui.node(id).children.first() {
                ui.remove_child(id, first);
            }
        }
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        None
    }
}
