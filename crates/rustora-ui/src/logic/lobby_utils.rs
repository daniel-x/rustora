// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `LobbyUtils.cs` and the tooltips of the lobby. A client is captured by value: the lobby runs these setup
//! functions again after each change of the session, as C# does.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_client::map_cache::{MapPreview, MapStatus};
use rustora_client::settings::{Settings, sanitized_player_name};
use rustora_core::fieldloader::Color;
use rustora_net::session::{Client, ConnectionQuality, Session, Slot};

use crate::fluent::MessageArg;
use crate::input::{MouseButton, MouseInput};
use crate::loader::cell;
use crate::logic::{NoLogic, message, message_with, services};
use crate::net::{LobbyFaction, LobbyRules, Net, is_ready};
use crate::widget::{Action, ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::{
    CheckboxWidget, ClientTooltipRegionWidget, ColorBlockWidget, DropDownButtonWidget, ImageWidget, LabelWidget, MapPreviewWidget,
    ScrollItemWidget, TextFieldWidget,
};

pub type Factions = Rc<IndexMap<String, LobbyFaction>>;

fn client_name(client: &Client) -> String {
    client.name.clone().unwrap_or_default()
}

fn faction_of(client: &Client) -> String {
    client.faction.clone().unwrap_or_default()
}

fn team_text(team: i32) -> String {
    if team == 0 { "-".to_string() } else { team.to_string() }
}

fn spawn_text(spawn: i32) -> String {
    if spawn == 0 {
        "-".to_string()
    } else {
        char::from_u32('A' as u32 - 1 + spawn as u32).map_or_else(String::new, String::from)
    }
}

fn hide_child(ui: &mut Ui, parent: WidgetId, name: &str) {
    if let Some(widget) = ui.find(parent, name) {
        ui.set_visible_fn(widget, || false);
    }
}

fn set_label_text(ui: &Ui, id: WidgetId, text: String) {
    ui.label(id).get_text = Some(Rc::new(move || text.clone()));
}

fn local_ready(net: &Net) -> impl Fn() -> bool + 'static {
    let net = net.clone();
    move || net.borrow().local_is_ready()
}

/// A drop down item of `SlotDropDownOption`, `DropDownOption` and the like.
#[derive(Clone)]
struct DropDownOption {
    title: String,
    is_selected: Rc<dyn Fn() -> bool>,
    on_click: Action,
}

fn option_item(ui: &mut Ui, option: DropDownOption, template: WidgetId) -> WidgetId {
    let item = ScrollItemWidget::setup(ui, template, option.is_selected, option.on_click);
    let label = ui.child(item, "LABEL");
    set_label_text(ui, label, option.title);
    item
}

fn command_option(net: &Net, title: String, command: String, is_selected: impl Fn() -> bool + 'static) -> DropDownOption {
    let net = net.clone();
    DropDownOption {
        title,
        is_selected: Rc::new(is_selected),
        on_click: Rc::new(move |_| net.borrow_mut().command(&command)),
    }
}

/// `ShowSlotDropDown`.
fn show_slot_dropdown(ui: &mut Ui, dropdown: WidgetId, slot: &Slot, client: &Option<Client>, net: &Net, rules: &LobbyRules) {
    let closed = slot.closed;
    let empty = client.is_none();
    let reference = &slot.player_reference;
    let mut groups = vec![(
        message(ui, "options-lobby-slot.slot"),
        vec![
            command_option(
                net,
                message(ui, "options-lobby-slot.open"),
                format!("slot_open {reference}"),
                move || !closed && empty,
            ),
            command_option(
                net,
                message(ui, "options-lobby-slot.closed"),
                format!("slot_close {reference}"),
                move || closed,
            ),
        ],
    )];
    let mut bots = Vec::new();
    if slot.allow_bots {
        let controller = net.borrow().session.clients.iter().find(|c| c.is_admin).map_or(0, |c| c.index);
        for (bot_type, name) in &rules.bots {
            let current = client.as_ref().and_then(|c| c.bot.clone());
            let this_type = bot_type.clone();
            bots.push(command_option(
                net,
                message(ui, name),
                format!("slot_bot {reference} {controller} {bot_type}"),
                move || current.as_ref() == Some(&this_type),
            ));
        }
    }
    let title = if bots.is_empty() {
        "options-lobby-slot.bots-disabled"
    } else {
        "options-lobby-slot.bots"
    };
    groups.push((message(ui, title), bots));
    DropDownButtonWidget::show_grouped_dropdown(ui, dropdown, "LABEL_DROPDOWN_TEMPLATE", 180, groups, option_item);
}

fn show_int_dropdown(
    ui: &mut Ui,
    dropdown: WidgetId,
    template: &str,
    options: Vec<i32>,
    current: i32,
    text: impl Fn(i32) -> String + 'static,
    on_click: impl Fn(i32) + 'static,
) {
    let on_click = Rc::new(on_click);
    DropDownButtonWidget::show_dropdown(ui, dropdown, template, 150, options, move |ui, value, item_template| {
        let on_click = on_click.clone();
        let item = ScrollItemWidget::setup(
            ui,
            item_template,
            Rc::new(move || current == value),
            Rc::new(move |_| on_click(value)),
        );
        let label = ui.child(item, "LABEL");
        set_label_text(ui, label, text(value));
        item
    });
}

/// `SplitOnFirstToken` with empty strings in place of `null`.
pub fn split_on_first_token(input: &str) -> (String, String) {
    match input.find('\n') {
        Some(split) if split > 0 => (input[..split].to_string(), input[split + 1..].to_string()),
        _ => (input.to_string(), String::new()),
    }
}

fn faction_tooltip(ui: &Ui, faction: Option<&LobbyFaction>) -> (String, String) {
    let description = faction
        .and_then(|f| f.description.as_ref())
        .map_or_else(String::new, |d| message(ui, d));
    split_on_first_token(&description)
}

/// `ShowFactionDropDown`.
fn show_faction_dropdown(ui: &mut Ui, dropdown: WidgetId, client: &Client, net: &Net, factions: &Factions) {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for (id, faction) in factions.iter().filter(|(_, f)| f.selectable) {
        let side = faction.side.as_ref().map_or_else(String::new, |s| message(ui, s));
        match groups.iter_mut().find(|(g, _)| *g == side) {
            Some((_, members)) => members.push(id.clone()),
            None => groups.push((side, vec![id.clone()])),
        }
    }
    let (current, index, net, factions) = (faction_of(client), client.index, net.clone(), factions.clone());
    DropDownButtonWidget::show_grouped_dropdown(ui, dropdown, "FACTION_DROPDOWN_TEMPLATE", 154, groups, move |ui, id, template| {
        let (selected, this) = (current.clone(), id.clone());
        let (n, command) = (net.clone(), format!("faction {index} {id}"));
        let item = ScrollItemWidget::setup(
            ui,
            template,
            Rc::new(move || selected == this),
            Rc::new(move |_| n.borrow_mut().command(&command)),
        );
        let faction = factions.get(&id);
        let label = ui.child(item, "LABEL");
        let name = faction.map_or_else(String::new, |f| message(ui, &f.name));
        let (width, font) = (ui.node(label).bounds.w, ui.label(label).font.clone());
        let truncated = ui.truncate_text(&font, &name, width);
        set_label_text(ui, label, truncated);
        let flag = ui.child(item, "FLAG");
        {
            let mut image = ui.get::<ImageWidget>(flag);
            image.get_image_collection = Some(Rc::new(|| "flags".to_string()));
            let flag_name = id.clone();
            image.get_image_name = Some(Rc::new(move || flag_name.clone()));
        }
        let (text, desc) = faction_tooltip(ui, faction);
        let mut button = ui.button(item);
        button.get_tooltip_text = Some(Rc::new(move || text.clone()));
        button.get_tooltip_desc = Some(Rc::new(move || desc.clone()));
        drop(button);
        item
    });
}

/// `SelectSpawnPoint`.
pub fn select_spawn_point(ui: &mut Ui, net: &Net, preview_widget: WidgetId, preview: &MapPreview, mi: &MouseInput) {
    if net.borrow().local_is_ready() {
        return;
    }
    let selected = determine_selected_spawn(ui, preview_widget, preview, mi);
    match mi.button {
        MouseButton::Left => {
            let target = {
                let state = net.borrow();
                let (local, host) = (state.local_client_id, state.is_host());
                state
                    .session
                    .clients
                    .iter()
                    .filter(|c| c.index == local || (host && c.is_bot()))
                    .find(|c| ((selected == 0) ^ (c.spawn_point == 0)) && !c.is_observer())
                    .map_or(local, |c| c.index)
            };
            set_spawn_point(net, target, selected);
        }
        MouseButton::Right => {
            let allowed = {
                let state = net.borrow();
                let owner = state.session.clients.iter().find(|c| c.spawn_point == selected).map(|c| c.index);
                state.is_host() || owner == Some(state.local_client_id)
            };
            if allowed {
                net.borrow_mut().command(&format!("clear_spawn {selected}"));
            }
        }
        _ => {}
    }
}

fn determine_selected_spawn(ui: &mut Ui, preview_widget: WidgetId, preview: &MapPreview, mi: &MouseInput) -> i32 {
    let spawn_size = ui.spawn_image_size();
    let widget = ui.get::<MapPreviewWidget>(preview_widget);
    preview
        .spawn_points
        .iter()
        .enumerate()
        .find(|(_, spawn)| {
            let location = widget.convert_to_preview(preview, **spawn);
            let dx = (location.0 - mi.x) as f32 / spawn_size.0 as f32 * 2.0;
            let dy = (location.1 - mi.y) as f32 / spawn_size.1 as f32 * 2.0;
            dx * dx + dy * dy <= 1.0
        })
        .map_or(0, |(i, _)| i as i32 + 1)
}

/// `SetSpawnPoint`.
fn set_spawn_point(net: &Net, client_index: i32, spawn: i32) {
    let mut state = net.borrow_mut();
    let owned = state.session.clients.iter().any(|c| c.spawn_point == spawn) || state.session.disabled_spawn_points.contains(&spawn);
    if spawn == 0 || !owned {
        state.command(&format!("spawn {client_index} {spawn}"));
    }
}

/// `InsufficientEnabledSpawnPoints`.
pub fn insufficient_enabled_spawn_points(map: &MapPreview, lobby: &Session) -> bool {
    let spawn_points = map.spawn_points.len() as i32;
    if spawn_points == 0 {
        return false;
    }
    let available = (1..=spawn_points).filter(|s| !lobby.disabled_spawn_points.contains(s)).count();
    available < lobby.clients.iter().filter(|c| !c.is_observer()).count()
}

fn latency_color(client: Option<&Client>) -> Color {
    match client.map(|c| c.connection_quality) {
        Some(ConnectionQuality::Good) => Color::from_argb(255, 50, 205, 50),
        Some(ConnectionQuality::Moderate) => Color::from_argb(255, 255, 165, 0),
        Some(ConnectionQuality::Poor) => Color::from_argb(255, 255, 0, 0),
        _ => Color::from_argb(255, 128, 128, 128),
    }
}

fn latency_description(client: Option<&Client>) -> &'static str {
    match client.map(|c| c.connection_quality) {
        Some(ConnectionQuality::Good) => "Good",
        Some(ConnectionQuality::Moderate) => "Moderate",
        Some(ConnectionQuality::Poor) => "Poor",
        _ => "Unknown",
    }
}

/// `SetupLatencyWidget`.
pub fn setup_latency_widget(ui: &mut Ui, parent: WidgetId, client: Option<&Client>) {
    let visible = client.is_some_and(|c| !c.is_bot());
    if let Some(block) = ui.find(parent, "LATENCY") {
        ui.set_visible_fn(block, move || visible);
        if visible {
            let color = latency_color(client);
            let widget = ui.child(block, "LATENCY_COLOR");
            ui.get::<ColorBlockWidget>(widget).get_color = Some(Rc::new(move || color));
        }
    }
    let tooltip = ui.child(parent, "LATENCY_REGION");
    ui.set_visible_fn(tooltip, move || visible);
    if visible {
        ui.get::<ClientTooltipRegionWidget>(tooltip).client = client.cloned();
    }
}

/// `SetupProfileWidget`.
pub(crate) fn setup_profile_widget(ui: &mut Ui, parent: WidgetId, client: &Client) {
    if let Some(profile) = ui.find(parent, "PROFILE") {
        let name = if client.is_bot() {
            "bot".to_string()
        } else {
            let role = if client.is_admin { "admin-" } else { "player-" };
            let kind = if client.fingerprint.is_some() { "registered" } else { "anonymous" };
            format!("{role}{kind}")
        };
        ui.get::<ImageWidget>(profile).get_image_name = Some(Rc::new(move || name.clone()));
        ui.set_visible_fn(profile, || true);
    }
    if let Some(tooltip) = ui.find(parent, "PROFILE_TOOLTIP") {
        {
            let mut region = ui.get::<ClientTooltipRegionWidget>(tooltip);
            if client.fingerprint.is_some() {
                region.template = "REGISTERED_PLAYER_TOOLTIP".to_string();
            }
            if client.is_bot() {
                region.template = "BOT_TOOLTIP".to_string();
            }
            region.client = Some(client.clone());
        }
        ui.set_visible_fn(tooltip, || true);
    }
}

/// Asks the server to rename the local player to the sanitized `name` and saves it as the player name; returns
/// the sanitized name.
pub fn change_player_name(net: &Net, settings: &RefCell<Settings>, name: &str) -> String {
    let clean = sanitized_player_name(name);
    net.borrow_mut().command(&format!("name {clean}"));
    let mut settings = settings.borrow_mut();
    settings.player.name = clean.clone();
    if let Err(e) = settings.save() {
        log::warn!("Cannot save the settings: {e}");
    }
    clean
}

/// `SetupEditableNameWidget`.
pub fn setup_editable_name_widget(ui: &mut Ui, parent: WidgetId, client: &Client, net: &Net) {
    let name = ui.child(parent, "NAME");
    ui.set_visible_fn(name, || true);
    let current = client_name(client);
    let settings = services(ui).settings.clone();
    let esc_pressed = Rc::new(Cell::new(false));
    {
        let mut field = ui.get::<TextFieldWidget>(name);
        let ready = local_ready(net);
        field.is_disabled = Some(Rc::new(ready));
        field.set_text(&current);
        let (esc, n, original) = (esc_pressed.clone(), net.clone(), current.clone());
        field.on_lose_focus = Some(Rc::new(move |ui| {
            if esc.replace(false) {
                return;
            }
            let text = ui.get::<TextFieldWidget>(name).text().trim().to_string();
            if text.is_empty() {
                ui.get::<TextFieldWidget>(name).set_text(&original);
            } else if text != original {
                let clean = change_player_name(&n, &settings, &text);
                ui.get::<TextFieldWidget>(name).set_text(&clean);
            }
        }));
        field.on_enter_key = Some(Rc::new(move |ui, _| {
            ui.yield_keyboard_focus(name);
            true
        }));
        let original = current.clone();
        field.on_esc_key = Some(Rc::new(move |ui, _| {
            ui.get::<TextFieldWidget>(name).set_text(&original);
            esc_pressed.set(true);
            ui.yield_keyboard_focus(name);
            true
        }));
    }
    setup_profile_widget(ui, name, client);
    hide_child(ui, parent, "SLOT_OPTIONS");
}

fn bot_or_client_name(ui: &Ui, client: &Client) -> String {
    let name = client_name(client);
    if !client.is_bot() {
        return name;
    }
    ui.assets
        .borrow()
        .messages
        .try_get(&name, &[])
        .unwrap_or_else(|| message(ui, "label-bot-player"))
}

/// `SetupNameWidget`.
pub fn setup_name_widget(ui: &mut Ui, parent: WidgetId, client: &Client) {
    let label = ui.child(parent, "NAME");
    ui.set_visible_fn(label, || true);
    let (width, font) = (ui.node(label).bounds.w, ui.label(label).font.clone());
    let name = bot_or_client_name(ui, client);
    let truncated = ui.truncate_text(&font, &name, width);
    set_label_text(ui, label, truncated);
    setup_profile_widget(ui, parent, client);
}

fn truncated_for_dropdown(ui: &Ui, dropdown: WidgetId, text: &str) -> String {
    let bounds = ui.node(dropdown).bounds;
    let (left, right, font) = {
        let button = ui.button(dropdown);
        (button.left_margin, button.right_margin, button.font.clone())
    };
    ui.truncate_text(&font, text, bounds.w - bounds.h - left - right)
}

/// `SetupEditableSlotWidget`.
pub fn setup_editable_slot_widget(ui: &mut Ui, parent: WidgetId, slot: &Slot, client: Option<&Client>, net: &Net, rules: &Rc<LobbyRules>) {
    let dropdown = ui.child(parent, "SLOT_OPTIONS");
    ui.set_visible_fn(dropdown, || true);
    let name = match client {
        Some(c) => bot_or_client_name(ui, c),
        None if slot.closed => message(ui, "options-lobby-slot.closed"),
        None => message(ui, "options-lobby-slot.open"),
    };
    let text = truncated_for_dropdown(ui, dropdown, &name);
    let (slot, client, n, rules) = (slot.clone(), client.cloned(), net.clone(), rules.clone());
    let mut button = ui.button(dropdown);
    button.is_disabled = Some(Rc::new(local_ready(net)));
    button.get_text = Some(Rc::new(move || text.clone()));
    button.on_mouse_down = Some(Rc::new(move |ui, _| show_slot_dropdown(ui, dropdown, &slot, &client, &n, &rules)));
    drop(button);
    hide_child(ui, parent, "NAME");
}

/// `SetupSlotWidget`.
pub fn setup_slot_widget(ui: &mut Ui, parent: WidgetId, slot: &Slot, client: Option<&Client>) {
    let name = ui.child(parent, "NAME");
    ui.set_visible_fn(name, || true);
    let text = match client {
        Some(c) => client_name(c),
        None if slot.closed => message(ui, "options-lobby-slot.closed"),
        None => message(ui, "options-lobby-slot.open"),
    };
    set_label_text(ui, name, text);
    hide_child(ui, parent, "SLOT_OPTIONS");
}

/// `ShowPlayerActionDropDown`.
fn show_player_action_dropdown(
    ui: &mut Ui,
    dropdown: WidgetId,
    client: &Client,
    net: &Net,
    lobby: WidgetId,
    before: Action,
    after: Action,
) {
    let index = client.index;
    let client_name = client_name(client);
    let no_selection: Rc<dyn Fn() -> bool> = Rc::new(|| false);
    let (n, a) = (net.clone(), after.clone());
    let kick: Action = Rc::new(move |ui| {
        before(ui);
        let root = ui.child(lobby, "TOP_PANELS_ROOT");
        let (n, a2) = (n.clone(), a.clone());
        let ok_pressed: Rc<dyn Fn(&mut Ui, bool)> = Rc::new(move |ui, temp_ban| {
            n.borrow_mut()
                .command(&format!("kick {index} {}", if temp_ban { "True" } else { "False" }));
            a2(ui);
        });
        let args = WidgetArgs::default()
            .with_value("clientName", client_name.clone())
            .with_value("okPressed", ok_pressed)
            .with_action("cancelPressed", a.clone());
        ui.load_widget("KICK_CLIENT_DIALOG", Some(root), &args);
    });
    let mut options = vec![DropDownOption {
        title: "Kick".to_string(),
        is_selected: no_selection.clone(),
        on_click: kick,
    }];
    let (independent_server, allow_spectators) = {
        let state = net.borrow();
        (
            state.session.global_settings.independent_server,
            state.session.global_settings.allow_spectators,
        )
    };
    if independent_server {
        options.push(command_option(
            net,
            "Transfer Admin".to_string(),
            format!("make_admin {index}"),
            || false,
        ));
    }
    if !client.is_observer() && allow_spectators {
        options.push(command_option(
            net,
            "Move to Spectator".to_string(),
            format!("make_spectator {index}"),
            || false,
        ));
    }
    DropDownButtonWidget::show_dropdown(ui, dropdown, "PLAYERACTION_DROPDOWN_TEMPLATE", 167, options, option_item);
}

/// `SetupPlayerActionWidget`.
pub fn setup_player_action_widget(
    ui: &mut Ui,
    parent: WidgetId,
    client: &Client,
    net: &Net,
    lobby: WidgetId,
    before: Action,
    after: Action,
) {
    let dropdown = ui.child(parent, "PLAYER_ACTION");
    let (n, index) = (net.clone(), client.index);
    ui.set_visible_fn(dropdown, move || {
        let state = n.borrow();
        state.is_host() && index != state.local_client_id
    });
    let text = truncated_for_dropdown(ui, dropdown, &client_name(client));
    let (c, n) = (client.clone(), net.clone());
    let mut button = ui.button(dropdown);
    button.is_disabled = Some(Rc::new(local_ready(net)));
    button.get_text = Some(Rc::new(move || text.clone()));
    button.on_mouse_down = Some(Rc::new(move |ui, _| {
        show_player_action_dropdown(ui, dropdown, &c, &n, lobby, before.clone(), after.clone());
    }));
    drop(button);
    setup_profile_widget(ui, dropdown, client);
    hide_child(ui, parent, "NAME");
}

/// `SetupKickSpectatorsWidget`.
pub fn setup_kick_spectators_widget(
    ui: &mut Ui,
    parent: WidgetId,
    net: &Net,
    lobby: WidgetId,
    before: Action,
    after: Action,
    skirmish: bool,
) {
    let checkbox = ui.child(parent, "TOGGLE_SPECTATORS");
    let n = net.clone();
    ui.get::<CheckboxWidget>(checkbox).is_checked = Some(Rc::new(move || n.borrow().session.global_settings.allow_spectators));
    let n = net.clone();
    ui.set_visible_fn(checkbox, move || n.borrow().local_client().is_some_and(|c| c.is_admin) && !skirmish);
    let n = net.clone();
    let mut button = ui.button(checkbox);
    button.is_disabled = Some(Rc::new(|| false));
    button.on_click = Some(Rc::new(move |ui| {
        before(ui);
        let toggle = |n: &Net| {
            let allow = n.borrow().session.global_settings.allow_spectators;
            n.borrow_mut()
                .command(&format!("allow_spectators {}", if allow { "False" } else { "True" }));
        };
        let spectators = n.borrow().session.clients.iter().filter(|c| c.is_observer()).count();
        if spectators == 0 {
            toggle(&n);
            after(ui);
            return;
        }
        let (n2, a2) = (n.clone(), after.clone());
        let ok_pressed: Action = Rc::new(move |ui| {
            toggle(&n2);
            let kicks: Vec<String> = n2
                .borrow()
                .session
                .clients
                .iter()
                .filter(|c| c.is_observer() && !c.is_admin)
                .map(|c| format!("kick {} {}", c.index, client_name(c)))
                .collect();
            for kick in kicks {
                n2.borrow_mut().command(&kick);
            }
            a2(ui);
        });
        let root = ui.child(lobby, "TOP_PANELS_ROOT");
        let args = WidgetArgs::default()
            .with_value("clientCount", spectators as i32)
            .with_action("okPressed", ok_pressed)
            .with_action("cancelPressed", after.clone());
        ui.load_widget("KICK_SPECTATORS_DIALOG", Some(root), &args);
    }));
    drop(button);
}

/// `SetupEditableColorWidget`. The color chooser opens with `crate::logic::color_picker`.
pub fn setup_editable_color_widget(ui: &mut Ui, parent: WidgetId, slot: Option<&Slot>, client: &Client, net: &Net) {
    let dropdown = ui.child(parent, "COLOR");
    let locked = slot.is_some_and(|s| s.lock_color);
    let ready = local_ready(net);
    let (index, color, n) = (client.index, client.color, net.clone());
    let settings = services(ui).settings.clone();
    let mut button = ui.button(dropdown);
    button.is_disabled = Some(Rc::new(move || locked || ready()));
    button.on_mouse_down = Some(Rc::new(move |ui, _| {
        let (n, settings) = (n.clone(), settings.clone());
        let on_exit: Rc<dyn Fn(&mut Ui, Color)> = Rc::new(move |_, chosen| {
            let local = n.borrow().local_client_id == index;
            if local {
                let mut settings = settings.borrow_mut();
                settings.player.color = chosen;
                if let Err(e) = settings.save() {
                    log::warn!("Cannot save the settings: {e}");
                }
            }
            n.borrow_mut()
                .command(&format!("color {index} {}", rustora_net::session::fmt_color(chosen)));
        });
        crate::logic::color_picker::show_color_dropdown(ui, dropdown, color, on_exit);
    }));
    drop(button);
    setup_color_widget(ui, dropdown, client);
}

/// `SetupColorWidget`.
pub fn setup_color_widget(ui: &mut Ui, parent: WidgetId, client: &Client) {
    let block = ui.child(parent, "COLORBLOCK");
    let color = client.color;
    ui.get::<ColorBlockWidget>(block).get_color = Some(Rc::new(move || color));
}

/// `SetupEditableFactionWidget`.
pub fn setup_editable_faction_widget(ui: &mut Ui, parent: WidgetId, slot: &Slot, client: &Client, net: &Net, factions: &Factions) {
    let dropdown = ui.child(parent, "FACTION");
    let locked = slot.lock_faction;
    let ready = local_ready(net);
    let (text, desc) = faction_tooltip(ui, factions.get(&faction_of(client)));
    let (c, n, f) = (client.clone(), net.clone(), factions.clone());
    let mut button = ui.button(dropdown);
    button.is_disabled = Some(Rc::new(move || locked || ready()));
    button.on_mouse_down = Some(Rc::new(move |ui, _| show_faction_dropdown(ui, dropdown, &c, &n, &f)));
    button.get_tooltip_text = Some(Rc::new(move || text.clone()));
    button.get_tooltip_desc = Some(Rc::new(move || desc.clone()));
    drop(button);
    setup_faction_widget(ui, dropdown, client, factions);
}

/// `SetupFactionWidget`.
pub fn setup_faction_widget(ui: &mut Ui, parent: WidgetId, client: &Client, factions: &Factions) {
    let name_label = ui.child(parent, "FACTIONNAME");
    let name = factions.get(&faction_of(client)).map_or_else(String::new, |f| message(ui, &f.name));
    let (width, font) = (ui.node(name_label).bounds.w, ui.label(name_label).font.clone());
    let truncated = ui.truncate_text(&font, &name, width);
    set_label_text(ui, name_label, truncated);
    let flag = ui.child(parent, "FACTIONFLAG");
    let faction = faction_of(client);
    let mut image = ui.get::<ImageWidget>(flag);
    image.get_image_name = Some(Rc::new(move || faction.clone()));
    image.get_image_collection = Some(Rc::new(|| "flags".to_string()));
}

/// `SetupEditableTeamWidget`.
pub fn setup_editable_team_widget(ui: &mut Ui, parent: WidgetId, slot: &Slot, client: &Client, net: &Net, map: &MapPreview) {
    let dropdown = ui.child(parent, "TEAM_DROPDOWN");
    ui.set_visible_fn(dropdown, || true);
    let locked = slot.lock_team;
    let ready = local_ready(net);
    let (index, team, n, count) = (client.index, client.team, net.clone(), map.player_count);
    let mut button = ui.button(dropdown);
    button.is_disabled = Some(Rc::new(move || locked || ready()));
    button.get_text = Some(Rc::new(move || team_text(team)));
    button.on_mouse_down = Some(Rc::new(move |ui, _| {
        let n = n.clone();
        show_int_dropdown(
            ui,
            dropdown,
            "TEAM_DROPDOWN_TEMPLATE",
            (0..=count).collect(),
            team,
            team_text,
            move |value| {
                n.borrow_mut().command(&format!("team {index} {value}"));
            },
        );
    }));
    drop(button);
    hide_child(ui, parent, "TEAM");
}

/// `SetupTeamWidget`.
pub fn setup_team_widget(ui: &mut Ui, parent: WidgetId, client: &Client) {
    let team = ui.child(parent, "TEAM");
    ui.set_visible_fn(team, || true);
    set_label_text(ui, team, team_text(client.team));
    hide_child(ui, parent, "TEAM_DROPDOWN");
}

/// `SetupEditableHandicapWidget`.
pub fn setup_editable_handicap_widget(ui: &mut Ui, parent: WidgetId, slot: &Slot, client: &Client, net: &Net) {
    let dropdown = ui.child(parent, "HANDICAP_DROPDOWN");
    ui.set_visible_fn(dropdown, || true);
    let locked = slot.lock_handicap;
    let ready = local_ready(net);
    let (index, handicap, n) = (client.index, client.handicap, net.clone());
    let mut button = ui.button(dropdown);
    button.is_disabled = Some(Rc::new(move || locked || ready()));
    button.get_text = Some(Rc::new(move || format!("{handicap}%")));
    button.on_mouse_down = Some(Rc::new(move |ui, _| {
        let n = n.clone();
        let options = (0..20).map(|i| 5 * i).collect();
        show_int_dropdown(
            ui,
            dropdown,
            "TEAM_DROPDOWN_TEMPLATE",
            options,
            handicap,
            |v| format!("{v}%"),
            move |value| {
                n.borrow_mut().command(&format!("handicap {index} {value}"));
            },
        );
    }));
    drop(button);
    hide_child(ui, parent, "HANDICAP");
}

/// `SetupHandicapWidget`.
pub fn setup_handicap_widget(ui: &mut Ui, parent: WidgetId, client: &Client) {
    let label = ui.child(parent, "HANDICAP");
    ui.set_visible_fn(label, || true);
    set_label_text(ui, label, format!("{}%", client.handicap));
    hide_child(ui, parent, "HANDICAP_DROPDOWN");
}

/// `SetupEditableSpawnWidget`.
pub fn setup_editable_spawn_widget(ui: &mut Ui, parent: WidgetId, slot: &Slot, client: &Client, net: &Net, map: &MapPreview) {
    let dropdown = ui.child(parent, "SPAWN_DROPDOWN");
    ui.set_visible_fn(dropdown, || true);
    let locked = slot.lock_spawn;
    let ready = local_ready(net);
    let (index, spawn, n, spawns) = (client.index, client.spawn_point, net.clone(), map.spawn_points.len() as i32);
    let mut button = ui.button(dropdown);
    button.is_disabled = Some(Rc::new(move || locked || ready()));
    button.get_text = Some(Rc::new(move || spawn_text(spawn)));
    button.on_mouse_down = Some(Rc::new(move |ui, _| {
        let options: Vec<i32> = {
            let state = n.borrow();
            (0..=spawns)
                .filter(|&s| {
                    let taken = s != 0 && state.session.clients.iter().any(|c| c.index != index && c.spawn_point == s);
                    !taken && !state.session.disabled_spawn_points.contains(&s)
                })
                .collect()
        };
        let n2 = n.clone();
        show_int_dropdown(ui, dropdown, "SPAWN_DROPDOWN_TEMPLATE", options, spawn, spawn_text, move |value| {
            set_spawn_point(&n2, index, value);
        });
    }));
    drop(button);
    hide_child(ui, parent, "SPAWN");
}

/// `SetupSpawnWidget`.
pub fn setup_spawn_widget(ui: &mut Ui, parent: WidgetId, client: &Client) {
    let spawn = ui.child(parent, "SPAWN");
    ui.set_visible_fn(spawn, || true);
    set_label_text(ui, spawn, spawn_text(client.spawn_point));
    hide_child(ui, parent, "SPAWN_DROPDOWN");
}

/// `SetupEditableReadyWidget`.
pub fn setup_editable_ready_widget(ui: &mut Ui, parent: WidgetId, client: &Client, net: &Net, map: &MapPreview, is_enabled: bool) {
    let status = ui.child(parent, "STATUS_CHECKBOX");
    ui.set_visible_fn(status, || true);
    let is_bot = client.is_bot();
    let available = map.status == MapStatus::Available;
    ui.button(status).is_disabled = Some(Rc::new(move || is_bot || !available || !is_enabled));
    if is_bot {
        ui.get::<CheckboxWidget>(status).is_checked = Some(Rc::new(|| true));
        return;
    }
    let predicted = Rc::new(Cell::new(is_ready(client)));
    let shown = predicted.clone();
    ui.get::<CheckboxWidget>(status).is_checked = Some(Rc::new(move || shown.get()));
    let n = net.clone();
    ui.button(status).on_click = Some(Rc::new(move |_| {
        let state = if predicted.get() { "NotReady" } else { "Ready" };
        n.borrow_mut().command(&format!("state {state}"));
        predicted.set(!predicted.get());
    }));
}

/// `SetupReadyWidget`.
pub fn setup_ready_widget(ui: &mut Ui, parent: WidgetId, client: &Client) {
    let image = ui.child(parent, "STATUS_IMAGE");
    let shown = is_ready(client) || client.is_bot();
    ui.set_visible_fn(image, move || shown);
}

/// `HideReadyWidgets`.
pub fn hide_ready_widgets(ui: &mut Ui, parent: WidgetId) {
    hide_child(ui, parent, "STATUS_CHECKBOX");
    hide_child(ui, parent, "STATUS_IMAGE");
}

/// Port of `LatencyTooltipLogic`.
struct LatencyTooltipLogic {
    widget: WidgetId,
    latency: WidgetId,
    right_margin: i32,
}

impl ChromeLogic for LatencyTooltipLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let (font, text) = {
            let label = ui.get::<LabelWidget>(self.latency);
            (label.font.clone(), label.get_text.as_ref().map_or_else(String::new, |f| f()))
        };
        let x = ui.node(self.latency).bounds.x;
        ui.node_mut(self.widget).bounds.w = x + ui.measure(&font, &text).0 + self.right_margin;
    }
}

pub fn latency_tooltip(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let client = args.value::<Option<Client>>("client").flatten();
    let prefix = ui.child(widget, "LATENCY_PREFIX");
    let latency = ui.child(widget, "LATENCY");
    let (prefix_font, prefix_text) = {
        let mut label = ui.get::<LabelWidget>(prefix);
        let text = label.text.clone().unwrap_or_default();
        (label.font.clone(), {
            let assets = ui.assets.borrow();
            let resolved = assets.messages.get(&text);
            label.resolved = resolved.clone();
            resolved
        })
    };
    let x = ui.node(prefix).bounds.x + ui.measure(&prefix_font, &format!("{prefix_text} ")).0;
    ui.node_mut(latency).bounds.x = x;
    let right_margin = ui.node(widget).bounds.w;
    let description = latency_description(client.as_ref());
    let color = latency_color(client.as_ref());
    {
        let mut label = ui.get::<LabelWidget>(latency);
        label.get_text = Some(Rc::new(move || description.to_string()));
        label.get_color = Some(Rc::new(move || color));
    }
    let mut logic = LatencyTooltipLogic {
        widget,
        latency,
        right_margin,
    };
    logic.tick(ui);
    cell(logic)
}

/// Port of `SpawnSelectorTooltipLogic`.
struct SpawnTooltipLogic {
    widget: WidgetId,
    preview: WidgetId,
    label: WidgetId,
    flag: WidgetId,
    team: WidgetId,
    single_height: i32,
    double_height: i32,
    label_margin: i32,
    show_unoccupied: bool,
    show: Rc<Cell<bool>>,
    label_text: Rc<RefCell<String>>,
    team_text: Rc<RefCell<String>>,
    faction: Rc<RefCell<Option<String>>>,
    player_team: Rc<Cell<i32>>,
}

impl ChromeLogic for SpawnTooltipLogic {
    fn tick(&mut self, ui: &mut Ui) {
        if !ui.exists(self.preview) {
            return;
        }
        let (index, occupant, disabled) = {
            let preview = ui.get::<MapPreviewWidget>(self.preview);
            let index = preview.tooltip_spawn_index;
            let occupant = preview.spawn_occupants.as_ref().and_then(|f| f().get(&index).cloned());
            let disabled = preview.disabled_spawn_points.as_ref().is_some_and(|f| f().contains(&index));
            (index, occupant, disabled)
        };
        self.show.set(index != -1);
        let mut team_width = 0;
        match occupant {
            Some(occupant) => {
                *self.label_text.borrow_mut() = occupant.player_name;
                *self.faction.borrow_mut() = occupant.faction;
                self.player_team.set(occupant.team);
                ui.node_mut(self.widget).bounds.h = if occupant.team > 0 {
                    self.double_height
                } else {
                    self.single_height
                };
                let team_text = if occupant.team > 0 {
                    message_with(ui, "label-team-name", &[("team", MessageArg::Number(occupant.team as i64))])
                } else {
                    String::new()
                };
                let font = ui.get::<LabelWidget>(self.team).font.clone();
                team_width = ui.measure(&font, &team_text).0;
                *self.team_text.borrow_mut() = team_text;
            }
            None => {
                if !self.show_unoccupied {
                    self.show.set(false);
                    return;
                }
                let key = if disabled {
                    "label-disabled-spawn"
                } else {
                    "label-available-spawn"
                };
                *self.label_text.borrow_mut() = message(ui, key);
                *self.faction.borrow_mut() = None;
                self.player_team.set(0);
                ui.node_mut(self.widget).bounds.h = self.single_height;
            }
        }
        let flag_right = ui.node(self.flag).bounds.right();
        let x = if self.faction.borrow().is_some() {
            flag_right + self.label_margin
        } else {
            self.label_margin
        };
        let font = ui.get::<LabelWidget>(self.label).font.clone();
        let label_width = ui.measure(&font, &self.label_text.borrow()).0;
        let label_bounds = &mut ui.node_mut(self.label).bounds;
        label_bounds.x = x;
        label_bounds.w = label_width;
        let width = (team_width + 2 * self.label_margin).max(x + label_width + self.label_margin);
        ui.node_mut(self.widget).bounds.w = width;
        ui.node_mut(self.team).bounds.w = width;
    }
}

pub fn spawn_tooltip(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let preview: WidgetId = args.value("preview").expect("a map preview");
    let show_unoccupied = args.value("showUnoccupiedSpawnpoints").unwrap_or(true);
    let label = ui.child(widget, "LABEL");
    let flag = ui.child(widget, "FLAG");
    let team = ui.child(widget, "TEAM");
    let single = ui.child(widget, "SINGLE_HEIGHT");
    let double = ui.child(widget, "DOUBLE_HEIGHT");
    let show = Rc::new(Cell::new(false));
    let label_text: Rc<RefCell<String>> = Rc::default();
    let faction: Rc<RefCell<Option<String>>> = Rc::default();
    let player_team = Rc::new(Cell::new(0));
    let s = show.clone();
    ui.set_visible_fn(widget, move || s.get());
    let t = label_text.clone();
    ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || t.borrow().clone()));
    let f = faction.clone();
    ui.set_visible_fn(flag, move || f.borrow().is_some());
    let f = faction.clone();
    {
        let mut image = ui.get::<ImageWidget>(flag);
        image.get_image_collection = Some(Rc::new(|| "flags".to_string()));
        image.get_image_name = Some(Rc::new(move || f.borrow().clone().unwrap_or_default()));
    }
    let team_text: Rc<RefCell<String>> = Rc::default();
    let shown_team = team_text.clone();
    ui.get::<LabelWidget>(team).get_text = Some(Rc::new(move || shown_team.borrow().clone()));
    let p = player_team.clone();
    ui.set_visible_fn(team, move || p.get() > 0);
    let mut logic = SpawnTooltipLogic {
        widget,
        preview,
        label,
        flag,
        team,
        single_height: ui.node(single).bounds.h,
        double_height: ui.node(double).bounds.h,
        label_margin: ui.node(widget).bounds.w,
        show_unoccupied,
        show,
        label_text,
        team_text,
        faction,
        player_team,
    };
    logic.tick(ui);
    cell(logic)
}

/// Port of `KickClientLogic`.
pub fn kick_client(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let name: String = args.value("clientName").unwrap_or_default();
    let text = message_with(ui, "dialog-kick-client.prompt", &[("player", MessageArg::Text(&name))]);
    let title = ui.child(widget, "TITLE");
    set_label_text(ui, title, text);
    let temp_ban = Rc::new(Cell::new(false));
    let checkbox = ui.child(widget, "PREVENT_REJOINING_CHECKBOX");
    let t = temp_ban.clone();
    ui.get::<CheckboxWidget>(checkbox).is_checked = Some(Rc::new(move || t.get()));
    let t = temp_ban.clone();
    ui.button(checkbox).on_click = Some(Rc::new(move |_| t.set(!t.get())));
    let ok_pressed: Option<Rc<dyn Fn(&mut Ui, bool)>> = args.value("okPressed");
    let ok = ui.child(widget, "OK_BUTTON");
    ui.button(ok).on_click = Some(Rc::new(move |ui| {
        let parent = ui.node(widget).parent.expect("dialog parent");
        ui.remove_child(parent, widget);
        if let Some(ok_pressed) = &ok_pressed {
            ok_pressed(ui, temp_ban.get());
        }
    }));
    close_with_action(ui, widget, "CANCEL_BUTTON", args.action("cancelPressed"));
    cell(NoLogic)
}

fn close_with_action(ui: &mut Ui, widget: WidgetId, button: &str, action: Action) {
    let id = ui.child(widget, button);
    ui.button(id).on_click = Some(Rc::new(move |ui| {
        let parent = ui.node(widget).parent.expect("dialog parent");
        ui.remove_child(parent, widget);
        action(ui);
    }));
}

/// Port of `KickSpectatorsLogic`.
pub fn kick_spectators(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let count: i32 = args.value("clientCount").unwrap_or(0);
    let text = message_with(ui, "dialog-kick-spectators.prompt", &[("count", MessageArg::Number(count as i64))]);
    let title = ui.child(widget, "TEXT");
    set_label_text(ui, title, text);
    close_with_action(ui, widget, "OK_BUTTON", args.action("okPressed"));
    close_with_action(ui, widget, "CANCEL_BUTTON", args.action("cancelPressed"));
    cell(NoLogic)
}

/// Spawn occupants of the preview tooltip: `new SpawnOccupant(client)`.
pub fn spawn_occupants(ui: &Ui, session: &Session) -> HashMap<i32, crate::widgets::SpawnOccupant> {
    session
        .clients
        .iter()
        .filter(|c| c.spawn_point != 0)
        .map(|c| {
            (
                c.spawn_point,
                crate::widgets::SpawnOccupant {
                    color: c.color,
                    player_name: bot_or_client_name(ui, c),
                    team: c.team,
                    faction: c.faction.clone(),
                    spawn_point: c.spawn_point,
                },
            )
        })
        .collect()
}

/// The text of `label` as `LabelWidget.GetText` returns it: the getter if set, else the resolved `Text` key.
fn label_text(ui: &Ui, label: WidgetId) -> String {
    let (get_text, key) = {
        let l = ui.label(label);
        (l.get_text.clone(), l.text.clone())
    };
    match get_text {
        Some(get) => get(),
        None => key.map(|k| message(ui, &k)).unwrap_or_default(),
    }
}

/// Sets the width of `widget` to fit the text of `label` with the label's left margin on both sides.
fn fit_width_to_label(ui: &mut Ui, widget: WidgetId, label: WidgetId) {
    let font = ui.label(label).font.clone();
    let width = ui.measure(&font, &label_text(ui, label)).0 + 2 * ui.node(label).bounds.x;
    ui.node_mut(widget).bounds.w = width;
}

/// Port of `AnonymousProfileTooltipLogic`.
pub fn anonymous_profile_tooltip(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let client = args.value::<Option<Client>>("client").flatten().unwrap_or_default();
    let name = ui.child(widget, "NAME");
    fit_width_to_label(ui, widget, name);
    let location_label = ui.child(widget, "LOCATION");
    let ip_label = ui.child(widget, "IP");
    let admin = ui.child(widget, "GAME_ADMIN");
    let location_height = ui.node(location_label).bounds.h;
    if let Some(location) = client.location.as_deref() {
        let font = ui.label(location_label).font.clone();
        let width = ui.node(widget).bounds.w - 2 * ui.node(location_label).bounds.x;
        let location = ui.truncate_text(&font, location, width);
        ui.set_visible_fn(location_label, || true);
        set_label_text(ui, location_label, location);
        ui.node_mut(widget).bounds.h += location_height;
        ui.node_mut(ip_label).bounds.y += location_height;
        ui.node_mut(admin).bounds.y += location_height;
    }
    if let Some(ip) = client.anonymized_ip_address.clone() {
        ui.set_visible_fn(ip_label, || true);
        set_label_text(ui, ip_label, ip);
        ui.node_mut(widget).bounds.h += ui.node(ip_label).bounds.h;
        ui.node_mut(admin).bounds.y += location_height;
    }
    if client.is_admin {
        ui.set_visible_fn(admin, || true);
        ui.node_mut(widget).bounds.h += ui.node(admin).bounds.h;
    }
    cell(NoLogic)
}

/// Port of `BotTooltipLogic`.
pub fn bot_tooltip(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let client = args.value::<Option<Client>>("client").flatten().unwrap_or_default();
    let name = ui.child(widget, "NAME");
    let controller = services(ui)
        .net
        .borrow()
        .session
        .client_with_index(client.bot_controller_client_index)
        .map(client_name);
    if let Some(controller) = controller {
        let text = message_with(ui, "label-bot-managed-by-tooltip", &[("name", MessageArg::Text(&controller))]);
        set_label_text(ui, name, text);
    }
    fit_width_to_label(ui, widget, name);
    cell(NoLogic)
}
