// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `RegisteredProfileTooltipLogic.cs` and `PlayerProfileBadgesLogic.cs`.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_net::session::Client;

use super::{NoLogic, message};
use crate::loader::cell;
use crate::player_database::{self, PlayerProfile};
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::BadgeWidget;

fn client(args: &WidgetArgs) -> Option<Client> {
    args.value::<Option<Client>>("client").flatten()
}

/// The text that a label shows.
fn label_text(ui: &Ui, label: WidgetId) -> String {
    let l = ui.label(label);
    match &l.get_text {
        Some(f) => f(),
        None => ui.assets.borrow().messages.get(l.text.as_deref().unwrap_or_default()),
    }
}

fn text_width(ui: &mut Ui, label: WidgetId, text: &str) -> i32 {
    let font = ui.label(label).font.clone();
    ui.measure(&font, text).0
}

fn set_text(ui: &mut Ui, label: WidgetId, text: String) {
    ui.label(label).get_text = Some(Rc::new(move || text.clone()));
}

/// `RegisteredProfileTooltipLogic`: shows a loading message until the profile download finishes.
pub fn registered(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(client) = client(args) else {
        return cell(NoLogic);
    };
    let header = ui.child(widget, "HEADER");
    let badges = ui.child(widget, "BADGES_CONTAINER");
    let profile_header = ui.child(header, "PROFILE_HEADER");
    let message_header = ui.child(header, "MESSAGE_HEADER");
    let message_label = ui.child(message_header, "MESSAGE");
    let loaded = Rc::new(std::cell::Cell::new(false));
    let l = loaded.clone();
    ui.set_visible_fn(profile_header, move || l.get());
    let l = loaded.clone();
    ui.set_visible_fn(message_header, move || !l.get());
    let max_width = ui.node(widget).bounds.w;
    let tooltip = RegisteredTooltip {
        widget,
        header,
        badges,
        profile_header,
        message_header,
        message_label,
        client,
        loaded,
        done: false,
        max_width,
    };
    tooltip.show_message(ui, "label-loading-player-profile");
    let message_height = ui.node(message_header).bounds.h;
    ui.node_mut(header).bounds.h += message_height;
    ui.node_mut(widget).bounds.h = ui.node(header).bounds.h;
    ui.node_mut(badges).visible = false;
    let mut tooltip = tooltip;
    tooltip.poll(ui);
    cell(tooltip)
}

struct RegisteredTooltip {
    widget: WidgetId,
    header: WidgetId,
    badges: WidgetId,
    profile_header: WidgetId,
    message_header: WidgetId,
    message_label: WidgetId,
    client: Client,
    loaded: Rc<std::cell::Cell<bool>>,
    done: bool,
    max_width: i32,
}

impl RegisteredTooltip {
    fn show_message(&self, ui: &mut Ui, key: &str) {
        let text = message(ui, key);
        let width = text_width(ui, self.message_label, &text) + 2 * ui.node(self.message_label).bounds.x;
        set_text(ui, self.message_label, text);
        ui.node_mut(self.header).bounds.w = width;
        ui.node_mut(self.widget).bounds.w = width;
    }

    fn poll(&mut self, ui: &mut Ui) {
        if self.done {
            return;
        }
        let Some(result) = self.client.fingerprint.as_deref().and_then(player_database::profile) else {
            return;
        };
        self.done = true;
        match result {
            Some(profile) => self.show_profile(ui, &profile),
            None => self.show_message(ui, "label-loading-player-profile-failed"),
        }
    }

    fn show_profile(&self, ui: &mut Ui, profile: &PlayerProfile) {
        let name = ui.child(self.profile_header, "PROFILE_NAME");
        let rank = ui.child(self.profile_header, "PROFILE_RANK");
        let admin = ui.child(self.profile_header, "GAME_ADMIN");
        let admin_label = ui.child(admin, "LABEL");
        let header_offset = ui.node(self.profile_header).bounds.h - ui.node(self.message_header).bounds.h;
        set_text(ui, name, profile.profile_name.clone());
        set_text(ui, rank, profile.profile_rank.clone());
        let mut width = text_width(ui, name, &profile.profile_name) + 2 * ui.node(name).bounds.x;
        width = width.max(text_width(ui, rank, &profile.profile_rank) + 2 * ui.node(rank).bounds.x);
        ui.node_mut(self.header).bounds.h += header_offset;
        let header_height = ui.node(self.header).bounds.h;
        ui.node_mut(self.badges).bounds.y += header_height;
        if self.client.is_admin {
            let text = label_text(ui, admin_label);
            width = width.max(text_width(ui, admin_label, &text) + 2 * ui.node(admin_label).bounds.x);
            ui.set_visible_fn(admin, || true);
            let admin_height = ui.node(admin_label).bounds.h;
            ui.node_mut(self.profile_header).bounds.h += admin_height;
            ui.node_mut(self.header).bounds.h += admin_height;
            ui.node_mut(self.badges).bounds.y += admin_height;
        }
        let max_width = self.max_width;
        if !profile.badges.is_empty() {
            let shared = Rc::new(std::cell::Cell::new(width));
            let s = shared.clone();
            let negotiate_width: Rc<dyn Fn(i32) -> i32> = Rc::new(move |badge_width| {
                s.set(badge_width.max(s.get()).min(max_width));
                s.get()
            });
            let args = WidgetArgs::default()
                .with_value("profile", profile.clone())
                .with_value("negotiateWidth", negotiate_width);
            let inserted = ui.load_widget("PLAYER_PROFILE_BADGES_INSERT", Some(self.badges), &args);
            width = shared.get();
            let height = ui.node(inserted).bounds.h;
            if height > 0 {
                ui.node_mut(self.badges).bounds.h = height;
                ui.node_mut(self.badges).visible = true;
            }
        }
        width = width.min(max_width);
        ui.node_mut(self.header).bounds.w = width;
        ui.node_mut(self.widget).bounds.w = width;
        ui.node_mut(self.badges).bounds.w = width;
        let badges_height = if ui.node(self.badges).visible { ui.node(self.badges).bounds.h } else { 0 };
        ui.node_mut(self.widget).bounds.h = ui.node(self.header).bounds.h + badges_height;
        if let Some(separator) = ui.find(self.badges, "SEPARATOR") {
            let x = ui.node(separator).bounds.x;
            ui.node_mut(separator).bounds.w = width - 2 * x;
        }
        self.loaded.set(true);
    }
}

impl ChromeLogic for RegisteredTooltip {
    fn tick(&mut self, ui: &mut Ui) {
        self.poll(ui);
    }
}

/// `PlayerProfileBadgesLogic`.
pub fn badges(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(profile) = args.value::<PlayerProfile>("profile") else {
        return cell(NoLogic);
    };
    let negotiate_width = args.value::<Rc<dyn Fn(i32) -> i32>>("negotiateWidth").unwrap_or_else(|| Rc::new(|w| w));
    let show = !profile.badges.is_empty();
    ui.set_visible_fn(widget, move || show);
    let template = ui.child(widget, "BADGE_TEMPLATE");
    ui.node_mut(widget).children.retain(|&c| c != template);
    let template_icon = ui.child(template, "ICON");
    let template_label = ui.child(template, "LABEL");
    let max_label_width = profile
        .badges
        .iter()
        .map(|b| text_width(ui, template_label, &b.label))
        .max()
        .unwrap_or(0);
    let icon_right = ui.node(template_icon).bounds.right();
    let width = negotiate_width(2 * ui.node(template_label).bounds.x - icon_right + max_label_width);
    ui.node_mut(widget).bounds.w = width;
    let template_bounds = ui.node(template).bounds;
    let mut offset = template_bounds.y + 3;
    for badge in &profile.badges {
        let b = ui.clone_widget(template);
        let icon = ui.child(b, "ICON");
        ui.get::<BadgeWidget>(icon).badge = Some(badge.clone());
        let label = ui.child(b, "LABEL");
        let font = ui.label(label).font.clone();
        let label_width = width - ui.node(label).bounds.x - ui.node(icon).bounds.x;
        let text = ui.truncate_text(&font, &badge.label, label_width);
        set_text(ui, label, text);
        ui.node_mut(b).bounds.y = offset;
        ui.add_child(widget, b);
        offset += template_bounds.h;
    }
    if offset > template_bounds.y {
        offset += 5;
    }
    ui.node_mut(widget).bounds.h = offset;
    ui.dispose_detached(template);
    cell(NoLogic)
}
