// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `LobbyLogic.cs`. The `Game.LobbyInfoChanged` event becomes a check of `NetState::revision` in each tick.
//! Not ported: sounds, the Discord status, the music and server list tabs, and map downloads.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustora_client::map_cache::{MapClassification, MapPreview, MapStatus, MapVisibility};
use rustora_core::miniyaml::MiniYamlNode;
use rustora_net::session::{Client, Slot};

use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::logic::lobby_options::GetRules;
use crate::logic::lobby_utils::{self as utils, Factions};
use crate::logic::map_chooser::OnSelectMap;
use crate::logic::map_preview::{GetMap, OnPreviewMouseDown};
use crate::logic::{connection, cosmetic_random, message, message_with, play_ui_sound, services};
use crate::net::{ConnectionState, LobbyRules, Net, TextNotification, TextNotificationPool, is_ready};
use crate::widget::{Action, ChromeLogic, Getter, KeyCallback, Ui, WidgetArgs, WidgetId};
use crate::widgets::{DropDownButtonWidget, LabelWidget, ScrollItemWidget, SpawnOccupant, TextFieldWidget};

/// `UnitOrders.ChatMessageMaxLength`.
const CHAT_MESSAGE_MAX_LENGTH: usize = 2500;
const MAP_STATUS_PLAYABLE: i32 = 2;

/// `items.Random(Game.CosmeticRandom)`.
fn pick<T: Clone>(items: &[T]) -> Option<T> {
    (!items.is_empty()).then(|| items[cosmetic_random(items.len())].clone())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelType {
    Players,
    Options,
    Music,
    Servers,
    Kick,
    ForceStart,
}

struct Lobby {
    widget: WidgetId,
    net: Net,
    skirmish_mode: bool,
    on_start: Action,
    on_exit: Action,
    panel: Rc<Cell<PanelType>>,
    map: Rc<RefCell<Option<Rc<MapPreview>>>>,
    map_status: Rc<Cell<i32>>,
    rules: Rc<RefCell<Option<(String, Rc<LobbyRules>)>>>,
    factions: RefCell<Factions>,
    spawn_occupants: Rc<RefCell<HashMap<i32, SpawnOccupant>>>,
    players: WidgetId,
    template_editable_player: WidgetId,
    template_noneditable_player: WidgetId,
    template_empty: WidgetId,
    template_editable_spectator: WidgetId,
    template_noneditable_spectator: WidgetId,
    template_new_spectator: WidgetId,
    chat_panel: WidgetId,
    chat_text_field: WidgetId,
    chat_templates: HashMap<TextNotificationPool, WidgetId>,
    sounds: Rc<LobbySounds>,
    chat_enabled: Cell<bool>,
    disable_team_chat: Rc<Cell<bool>>,
    team_chat: Rc<Cell<bool>>,
    insufficient_player_spawns: Rc<Cell<bool>>,
    tab_names: Rc<RefCell<Vec<String>>>,
    reset_options_enabled: Rc<Cell<bool>>,
    revision: Cell<u64>,
    notifications_seen: Cell<usize>,
    connection_lost: Cell<bool>,
}

impl Lobby {
    fn map_is_playable(&self) -> bool {
        self.map_status.get() & MAP_STATUS_PLAYABLE == MAP_STATUS_PLAYABLE
    }

    fn panel_is(&self, kind: PanelType) -> impl Fn() -> bool + 'static {
        let panel = self.panel.clone();
        move || panel.get() == kind
    }

    fn set_panel(&self, kind: PanelType) -> Action {
        let panel = self.panel.clone();
        Rc::new(move |_| panel.set(kind))
    }

    /// `UpdateCurrentMap`.
    fn update_current_map(&self, ui: &mut Ui) {
        let (status, uid) = {
            let net = self.net.borrow();
            (net.session.global_settings.map_status, net.session.global_settings.map.clone())
        };
        self.map_status.set(status);
        let Some(uid) = uid else {
            return;
        };
        let service = services(ui);
        let map = service.maps.borrow_mut().entry(&uid);
        let previous = self.map.borrow().clone();
        if previous.as_ref().is_some_and(|m| Rc::ptr_eq(m, &map)) {
            return;
        }
        let uid_changed = previous.as_ref().is_none_or(|m| m.uid != uid);
        let became_available = map.status == MapStatus::Available && previous.is_none_or(|m| m.status != MapStatus::Available);
        *self.map.borrow_mut() = Some(map.clone());
        if uid_changed {
            if map.status == MapStatus::Available {
                self.net.borrow_mut().command("state NotReady");
            } else if map.status != MapStatus::DownloadAvailable && service.settings.borrow().game.allow_downloading {
                service.map_repository.query(&service.maps, [uid.as_str()]);
            }
        }
        if !uid_changed && !became_available {
            return;
        }
        let lookup = service.lobby_rules.borrow().clone();
        if let Some(rules) = lookup.and_then(|f| f(&uid)) {
            *self.factions.borrow_mut() = Rc::new(rules.factions.clone());
            *self.rules.borrow_mut() = Some((uid, rules));
        }
    }

    /// `UpdateSpawnOccupants`.
    fn update_spawn_occupants(&self, ui: &Ui) {
        *self.spawn_occupants.borrow_mut() = utils::spawn_occupants(ui, &self.net.borrow().session);
    }

    /// `UpdateOptions`.
    fn update_options(&self) {
        let rules = self.rules.borrow();
        let Some((_, rules)) = rules.as_ref() else {
            return;
        };
        let net = self.net.borrow();
        let options = &net.session.global_settings.lobby_options;
        let differs = rules
            .options
            .iter()
            .filter(|o| o.is_visible)
            .any(|o| options.get(&o.id).and_then(|s| s.value.as_ref()) != Some(&o.default_value));
        self.reset_options_enabled.set(differs);
    }

    /// `UpdatePlayerList`.
    fn update_player_list(self: &Rc<Self>, ui: &mut Ui) {
        let (session, local_id, is_host) = {
            let net = self.net.borrow();
            if net.local_client().is_none() {
                return;
            }
            (net.session.clone(), net.local_client_id, net.is_host())
        };
        let local = session.client_with_index(local_id).expect("a local client").clone();
        let disable = if local.team == 0 && !local.is_observer() {
            true
        } else if local.is_observer() {
            !session.clients.iter().any(|c| c.index != local_id && c.is_observer())
        } else {
            !session
                .clients
                .iter()
                .any(|c| c.index != local_id && !c.is_bot() && c.team == local.team)
        };
        self.disable_team_chat.set(disable);
        if disable {
            self.team_chat.set(false);
        }
        let map = self.map.borrow().clone();
        self.insufficient_player_spawns
            .set(map.as_ref().is_some_and(|m| utils::insufficient_enabled_spawn_points(m, &session)));

        let mut idx = 0;
        if let Some(map) = &map {
            for (key, slot) in &session.slots {
                self.setup_slot_row(ui, idx, key, slot, session.client_in_slot(key), local_id, is_host, map);
                idx += 1;
            }
            for client in session.clients.iter().filter(|c| c.slot.is_none()) {
                self.setup_spectator_row(ui, idx, client, local_id, is_host);
                idx += 1;
            }
        }
        if local.slot.is_some() {
            let spec = self.reuse_or_clone(ui, idx, self.template_new_spectator);
            let (before, after) = (self.set_panel(PanelType::Kick), self.set_panel(PanelType::Players));
            utils::setup_kick_spectators_widget(ui, spec, &self.net, self.widget, before, after, self.skirmish_mode);
            let button = ui.child(spec, "SPECTATE");
            let net = self.net.clone();
            ui.button(button).on_click = Some(Rc::new(move |_| net.borrow_mut().command("spectate")));
            let net = self.net.clone();
            ui.button(button).is_disabled = Some(Rc::new(move || net.borrow().local_is_ready()));
            let net = self.net.clone();
            ui.set_visible_fn(button, move || {
                let net = net.borrow();
                net.session.global_settings.allow_spectators || net.local_client().is_some_and(|c| c.is_admin)
            });
            self.place_row(ui, idx, spec);
            idx += 1;
        }
        while ui.node(self.players).children.len() > idx {
            let last = ui.node(self.players).children[idx];
            ui.remove_child(self.players, last);
        }
        *self.tab_names.borrow_mut() = session
            .clients
            .iter()
            .filter(|c| !c.is_bot())
            .filter_map(|c| c.name.clone())
            .collect();
    }

    /// The row at `idx` if it came from `template`, else a new copy of `template`.
    fn reuse_or_clone(&self, ui: &mut Ui, idx: usize, template: WidgetId) -> WidgetId {
        let template_id = ui.node(template).id.clone();
        match ui.node(self.players).children.get(idx).copied() {
            Some(row) if ui.node(row).id == template_id => row,
            _ => ui.clone_widget(template),
        }
    }

    fn place_row(&self, ui: &mut Ui, idx: usize, row: WidgetId) {
        ui.set_visible_fn(row, || true);
        match ui.node(self.players).children.get(idx).copied() {
            None => ui.add_child(self.players, row),
            Some(old) if old != row => ui.replace_child(self.players, old, row),
            Some(_) => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setup_slot_row(
        self: &Rc<Self>,
        ui: &mut Ui,
        idx: usize,
        key: &str,
        slot: &Slot,
        client: Option<&Client>,
        local_id: i32,
        is_host: bool,
        map: &Rc<MapPreview>,
    ) {
        let net = &self.net;
        let rules = self.rules.borrow().as_ref().map_or_else(Rc::default, |(_, r)| r.clone());
        let factions: Factions = self.factions.borrow().clone();
        let row = match client {
            None => {
                let row = self.reuse_or_clone(ui, idx, self.template_empty);
                if is_host {
                    utils::setup_editable_slot_widget(ui, row, slot, None, net, &rules);
                } else {
                    utils::setup_slot_widget(ui, row, slot, None);
                }
                let join = ui.child(row, "JOIN");
                let closed = slot.closed;
                ui.set_visible_fn(join, move || !closed);
                let n = net.clone();
                ui.button(join).is_disabled = Some(Rc::new(move || n.borrow().local_is_ready()));
                let (n, command) = (net.clone(), format!("slot {key}"));
                ui.button(join).on_click = Some(Rc::new(move |_| n.borrow_mut().command(&command)));
                row
            }
            Some(client) if client.index == local_id || (client.is_bot() && is_host) => {
                let row = self.reuse_or_clone(ui, idx, self.template_editable_player);
                utils::setup_latency_widget(ui, row, Some(client));
                if client.is_bot() {
                    utils::setup_editable_slot_widget(ui, row, slot, Some(client), net, &rules);
                } else {
                    utils::setup_editable_name_widget(ui, row, client, net);
                }
                utils::setup_editable_color_widget(ui, row, Some(slot), client, net);
                utils::setup_editable_faction_widget(ui, row, slot, client, net, &factions);
                utils::setup_editable_team_widget(ui, row, slot, client, net, map);
                utils::setup_editable_handicap_widget(ui, row, slot, client, net);
                utils::setup_editable_spawn_widget(ui, row, slot, client, net, map);
                utils::setup_editable_ready_widget(ui, row, client, net, map, self.map_is_playable());
                row
            }
            Some(client) => {
                let row = self.reuse_or_clone(ui, idx, self.template_noneditable_player);
                utils::setup_latency_widget(ui, row, Some(client));
                utils::setup_color_widget(ui, row, client);
                utils::setup_faction_widget(ui, row, client, &factions);
                if is_host {
                    utils::setup_editable_team_widget(ui, row, slot, client, net, map);
                    utils::setup_editable_handicap_widget(ui, row, slot, client, net);
                    utils::setup_editable_spawn_widget(ui, row, slot, client, net, map);
                    let (before, after) = (self.set_panel(PanelType::Kick), self.set_panel(PanelType::Players));
                    utils::setup_player_action_widget(ui, row, client, net, self.widget, before, after);
                } else {
                    utils::setup_name_widget(ui, row, client);
                    utils::setup_team_widget(ui, row, client);
                    utils::setup_handicap_widget(ui, row, client);
                    utils::setup_spawn_widget(ui, row, client);
                }
                utils::setup_ready_widget(ui, row, client);
                row
            }
        };
        self.place_row(ui, idx, row);
    }

    fn setup_spectator_row(self: &Rc<Self>, ui: &mut Ui, idx: usize, client: &Client, local_id: i32, is_host: bool) {
        let net = &self.net;
        let row = if client.index == local_id {
            let row = self.reuse_or_clone(ui, idx, self.template_editable_spectator);
            utils::setup_editable_name_widget(ui, row, client, net);
            if client.is_admin {
                let map = self.map.borrow().clone();
                if let Some(map) = map {
                    utils::setup_editable_ready_widget(ui, row, client, net, &map, self.map_is_playable());
                }
            } else {
                utils::hide_ready_widgets(ui, row);
            }
            row
        } else {
            let row = self.reuse_or_clone(ui, idx, self.template_noneditable_spectator);
            if is_host {
                let (before, after) = (self.set_panel(PanelType::Kick), self.set_panel(PanelType::Players));
                utils::setup_player_action_widget(ui, row, client, net, self.widget, before, after);
            } else {
                utils::setup_name_widget(ui, row, client);
            }
            if client.is_admin {
                utils::setup_ready_widget(ui, row, client);
            } else {
                utils::hide_ready_widgets(ui, row);
            }
            row
        };
        utils::setup_latency_widget(ui, row, Some(client));
        self.place_row(ui, idx, row);
    }

    /// `Handle(TextNotification)`.
    fn add_chat_line(&self, ui: &mut Ui, notification: &TextNotification) {
        let Some(&template) = self.chat_templates.get(&notification.pool) else {
            return;
        };
        let line = ui.clone_widget(template);
        let bounds = ui.node(self.chat_panel).bounds;
        let scrollbar = ui.get::<crate::widgets::ScrollPanelWidget>(self.chat_panel).scrollbar_width;
        setup_text_notification(ui, line, notification, bounds.w - scrollbar, true);
        let at_bottom = ui.scrolled_to_bottom(self.chat_panel);
        ui.add_child(self.chat_panel, line);
        if at_bottom {
            ui.scroll_to_bottom(self.chat_panel, true);
        }
        let sound = match notification.pool {
            TextNotificationPool::Chat => &self.sounds.chat_line,
            TextNotificationPool::System => &self.sounds.option_changed,
            TextNotificationPool::Join => &self.sounds.player_joined,
            TextNotificationPool::Leave => &self.sounds.player_left,
            _ => return,
        };
        play_ui_sound(ui, sound);
    }

    /// `ConnectionStateChanged`.
    fn check_connection(&self, ui: &mut Ui) {
        let (state, password, target) = {
            let net = self.net.borrow();
            (net.connection_state, net.password.clone(), net.target.clone())
        };
        if state != ConnectionState::NotConnected || self.connection_lost.replace(true) {
            return;
        }
        let (on_exit, on_start) = (self.on_exit.clone(), self.on_start.clone());
        ui.run_after_tick(move |ui| {
            ui.close_window();
            let (exit, start) = (on_exit.clone(), on_start.clone());
            let on_connect: Action = Rc::new(move |ui| open_lobby(ui, exit.clone(), start.clone(), false));
            let (connect_action, exit, target) = (on_connect, on_exit.clone(), target.clone());
            let on_retry: connection::OnRetry = Rc::new(move |ui, pass| {
                connection::connect(ui, target.clone(), &pass, connect_action.clone(), exit.clone());
            });
            connection::open_connection_failed(ui, &password, on_exit.clone(), on_retry);
        });
    }
}

/// Opens the lobby window.
pub fn open_lobby(ui: &mut Ui, on_exit: Action, on_start: Action, skirmish_mode: bool) {
    let args = WidgetArgs::default()
        .with_action("onExit", on_exit)
        .with_action("onStart", on_start)
        .with_value("skirmishMode", skirmish_mode);
    let window = ui.open_window("SERVER_LOBBY", args);
    if !skirmish_mode {
        super::general_chat::place_dialog(ui, window);
    }
}

/// Port of `WidgetUtils.SetupTextNotification`.
pub fn setup_text_notification(ui: &mut Ui, widget: WidgetId, notification: &TextNotification, box_width: i32, with_timestamp: bool) {
    let time_label = ui.find(widget, "TIME");
    let prefix_label = ui.find(widget, "PREFIX");
    let text_label = ui.child(widget, "TEXT");
    let text_font = ui.label(text_label).font.clone();
    let widget_x = ui.node(widget).bounds.x;
    let mut text_width = box_width - widget_x - ui.node(text_label).bounds.x;
    let has_prefix = notification.prefix.as_ref().is_some_and(|p| !p.is_empty()) && prefix_label.is_some();
    let mut time_offset = 0;
    if let (true, Some(time)) = (with_timestamp, time_label) {
        let text = format!("{:02}:{:02}", notification.time.0, notification.time.1);
        let bounds = ui.node(time).bounds;
        time_offset = bounds.w + bounds.x;
        ui.label(time).get_text = Some(Rc::new(move || text.clone()));
        text_width -= time_offset;
        ui.node_mut(text_label).bounds.x += time_offset;
        if let (true, Some(prefix)) = (has_prefix, prefix_label) {
            ui.node_mut(prefix).bounds.x += time_offset;
        }
    }
    if let (true, Some(prefix_id)) = (has_prefix, prefix_label) {
        let prefix = format!("{}:", notification.prefix.as_deref().unwrap_or_default());
        let font = ui.label(prefix_id).font.clone();
        let size = ui.measure(&font, &prefix);
        let offset = size.0 + ui.node(prefix_id).bounds.x;
        {
            let mut label = ui.label(prefix_id);
            let color = notification.prefix_color.unwrap_or(label.text_color);
            label.get_color = Some(Rc::new(move || color));
            label.get_text = Some(Rc::new(move || prefix.clone()));
        }
        ui.node_mut(prefix_id).bounds.w = size.0;
        text_width -= offset;
        ui.node_mut(text_label).bounds.x += offset - time_offset;
    }
    {
        let mut label = ui.label(text_label);
        let color = notification.text_color.unwrap_or(label.text_color);
        label.get_color = Some(Rc::new(move || color));
    }
    ui.node_mut(text_label).bounds.w = text_width;
    let text = ui.assets.borrow_mut().wrap_text(&text_font, &notification.text, text_width);
    let dh = ui.measure(&text_font, &text).1 - ui.node(text_label).bounds.h;
    ui.label(text_label).get_text = Some(Rc::new(move || text.clone()));
    if dh > 0 {
        ui.node_mut(text_label).bounds.h += dh;
        ui.node_mut(widget).bounds.h += dh;
    }
    ui.node_mut(widget).bounds.w = box_width - widget_x;
}

/// `TabCompletionLogic.Complete`.
pub(crate) fn complete_name(text: &str, names: &[String]) -> String {
    let start = text.rfind(' ').map_or(0, |i| i + 1);
    let word = &text[start..];
    if word.is_empty() {
        return text.to_string();
    }
    let lowered = word.to_lowercase();
    match names.iter().find(|n| n.to_lowercase().starts_with(&lowered)) {
        Some(name) if start == 0 => format!("{name}: "),
        Some(name) => format!("{}{name}", &text[..start]),
        None => text.to_string(),
    }
}

impl ChromeLogic for LobbyHandle {
    fn tick(&mut self, ui: &mut Ui) {
        let lobby = &self.0;
        let revision = lobby.net.borrow().revision;
        let lobby_changed = lobby.revision.replace(revision) != revision;
        lobby.update_current_map(ui);
        if lobby_changed {
            lobby.update_player_list(ui);
            lobby.update_spawn_occupants(ui);
            lobby.update_options();
        }
        lobby.check_connection(ui);

        let notifications = {
            let net = lobby.net.borrow();
            net.notifications[lobby.notifications_seen.get().min(net.notifications.len())..].to_vec()
        };
        lobby.notifications_seen.set(lobby.notifications_seen.get() + notifications.len());
        for notification in &notifications {
            lobby.add_chat_line(ui, notification);
        }

        if lobby.panel.get() == PanelType::Options && !lobby.map_is_playable() {
            lobby.panel.set(PanelType::Players);
        }
        let until = lobby.net.borrow().chat_disabled_until;
        let now = ui.run_time();
        let was_enabled = lobby.chat_enabled.replace(now >= until && until != i64::MAX);
        if lobby.chat_enabled.get() && !was_enabled {
            ui.get::<TextFieldWidget>(lobby.chat_text_field).set_text("");
            if ui.keyboard_focus.is_none() {
                ui.take_keyboard_focus(lobby.chat_text_field);
            }
        } else if !lobby.chat_enabled.get() {
            let text = if until == i64::MAX {
                message(ui, "label-chat-disabled")
            } else {
                let seconds = (until - now + 999) / 1000;
                message_with(ui, "label-chat-availability", &[("seconds", MessageArg::Number(seconds))])
            };
            ui.get::<TextFieldWidget>(lobby.chat_text_field).set_text(&text);
        }
    }

    fn disposed(&mut self, ui: &mut Ui) {
        let lobby = &self.0;
        lobby.net.borrow_mut().before_game_start = None;
        for template in [
            lobby.template_editable_player,
            lobby.template_noneditable_player,
            lobby.template_empty,
            lobby.template_editable_spectator,
            lobby.template_noneditable_spectator,
            lobby.template_new_spectator,
        ] {
            ui.dispose_detached(template);
        }
        for &template in lobby.chat_templates.values() {
            ui.dispose_detached(template);
        }
    }
}

struct LobbyHandle(Rc<Lobby>);

/// The lobby notification sounds: chrome metrics, overridden by the logic arguments of the same name.
struct LobbySounds {
    chat_line: String,
    player_joined: String,
    player_left: String,
    option_changed: String,
}

impl LobbySounds {
    fn new(ui: &Ui, logic_args: &[MiniYamlNode]) -> LobbySounds {
        let get = |key: &str| {
            logic_args
                .iter()
                .find(|n| n.key == key)
                .map(|n| n.value.value_str().to_string())
                .unwrap_or_else(|| ui.assets.borrow().metrics.string(key))
        };
        LobbySounds {
            chat_line: get("ChatLineSound"),
            player_joined: get("PlayerJoinedSound"),
            player_left: get("PlayerLeftSound"),
            option_changed: get("LobbyOptionChangedSound"),
        }
    }
}

pub(crate) fn pool_from_name(name: &str) -> Option<TextNotificationPool> {
    match name {
        "Chat" => Some(TextNotificationPool::Chat),
        "Join" => Some(TextNotificationPool::Join),
        "Leave" => Some(TextNotificationPool::Leave),
        "System" => Some(TextNotificationPool::System),
        "Mission" => Some(TextNotificationPool::Mission),
        "Feedback" => Some(TextNotificationPool::Feedback),
        _ => None,
    }
}

fn take_template(ui: &mut Ui, players: WidgetId, name: &str) -> WidgetId {
    let template = ui.child(players, name);
    let copy = ui.clone_widget(template);
    ui.remove_child(players, template);
    copy
}

/// The drop down of `SLOTS_DROPDOWNBUTTON`.
fn show_slots_dropdown(lobby: &Rc<Lobby>, ui: &mut Ui, dropdown: WidgetId) {
    #[derive(Clone)]
    struct Item {
        title: String,
        action: Action,
    }
    let net = lobby.net.clone();
    let session = net.borrow().session.clone();
    let bot_types: Vec<String> = lobby
        .rules
        .borrow()
        .as_ref()
        .map_or_else(Vec::new, |(_, r)| r.bots.iter().map(|(t, _)| t.clone()).collect());
    let controller = session.clients.iter().find(|c| c.is_admin).map_or(0, |c| c.index);
    let mut groups: Vec<(String, Vec<Item>)> = Vec::new();
    if session.slots.values().any(|s| s.allow_bots) {
        let n = net.clone();
        let mut bots = vec![Item {
            title: message(ui, "options-slot-admin.add-bots"),
            action: Rc::new(move |_| {
                let session = n.borrow().session.clone();
                for (key, slot) in &session.slots {
                    let client = session.client_in_slot(key);
                    if slot.allow_bots
                        && client.is_none_or(Client::is_bot)
                        && let Some(bot) = pick(&bot_types)
                    {
                        n.borrow_mut().command(&format!("slot_bot {key} {controller} {bot}"));
                    }
                }
            }),
        }];
        if session.clients.iter().any(Client::is_bot) {
            let n = net.clone();
            bots.push(Item {
                title: message(ui, "options-slot-admin.remove-bots"),
                action: Rc::new(move |_| {
                    let session = n.borrow().session.clone();
                    for (key, slot) in &session.slots {
                        if session.client_in_slot(key).is_some_and(Client::is_bot) {
                            n.borrow_mut().command(&format!("slot_open {}", slot.player_reference));
                        }
                    }
                }),
            });
        }
        groups.push((message(ui, "options-slot-admin.configure-bots"), bots));
    }
    let occupied = session
        .slots
        .iter()
        .filter(|(k, s)| !s.lock_team && session.client_in_slot(k).is_some())
        .count() as i32;
    let team_count = (occupied + 1) / 2;
    if team_count >= 1 {
        let assign = |command: String| -> Action {
            let n = net.clone();
            Rc::new(move |_| n.borrow_mut().command(&command))
        };
        let mut teams: Vec<Item> = (2..=team_count)
            .rev()
            .map(|count| Item {
                title: message_with(ui, "options-slot-admin.teams-count", &[("count", MessageArg::Number(count as i64))]),
                action: assign(format!("assignteams {count}")),
            })
            .collect();
        if session.slots.values().any(|s| s.allow_bots) {
            teams.push(Item {
                title: message(ui, "options-slot-admin.humans-vs-bots"),
                action: assign("assignteams 1".to_string()),
            });
        }
        teams.push(Item {
            title: message(ui, "options-slot-admin.free-for-all"),
            action: assign("assignteams 0".to_string()),
        });
        groups.push((message(ui, "options-slot-admin.configure-teams"), teams));
    }
    DropDownButtonWidget::show_grouped_dropdown(ui, dropdown, "LABEL_DROPDOWN_TEMPLATE", 175, groups, |ui, item: Item, template| {
        let row = ScrollItemWidget::setup(ui, template, Rc::new(|| false), item.action);
        let label = ui.child(row, "LABEL");
        let title = item.title;
        ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || title.clone()));
        row
    });
}

pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs, logic_args: &[MiniYamlNode]) -> Rc<RefCell<dyn ChromeLogic>> {
    let net = services(ui).net.clone();
    let skirmish_mode: bool = args.value("skirmishMode").unwrap_or(false);
    let (on_exit, on_start) = (args.action("onExit"), args.action("onStart"));

    if let Some(name) = ui.find(widget, "SERVER_NAME") {
        let n = net.clone();
        ui.get::<LabelWidget>(name).get_text = Some(Rc::new(move || {
            n.borrow().session.global_settings.server_name.clone().unwrap_or_default()
        }));
    }

    let panel = Rc::new(Cell::new(PanelType::Players));
    let map: Rc<RefCell<Option<Rc<MapPreview>>>> = Rc::default();
    let map_status = Rc::new(Cell::new(0));
    let rules: Rc<RefCell<Option<(String, Rc<LobbyRules>)>>> = Rc::default();
    let spawn_occupants: Rc<RefCell<HashMap<i32, SpawnOccupant>>> = Rc::default();

    let (m, s) = (map.clone(), map_status.clone());
    let get_map: GetMap = Rc::new(move || (m.borrow().clone(), s.get()));
    let n = net.clone();
    let on_preview_mouse: OnPreviewMouseDown = Rc::new(move |ui, widget, preview, mi| {
        utils::select_spawn_point(ui, &n, widget, &preview, mi);
    });
    let o = spawn_occupants.clone();
    let get_occupants: Getter<HashMap<i32, SpawnOccupant>> = Rc::new(move || o.borrow().clone());
    let n = net.clone();
    let get_disabled: Getter<Vec<i32>> = Rc::new(move || n.borrow().session.disabled_spawn_points.iter().copied().collect());
    let preview_root = ui.child(widget, "MAP_PREVIEW_ROOT");
    let preview_args = WidgetArgs::default()
        .with_value("getMap", get_map)
        .with_value("onMouseDown", on_preview_mouse)
        .with_value("getSpawnOccupants", get_occupants)
        .with_value("getDisabledSpawnPoints", get_disabled)
        .with_value("showUnoccupiedSpawnpoints", true);
    let map_container = ui.load_widget("MAP_PREVIEW", Some(preview_root), &preview_args);
    let p = panel.clone();
    ui.set_visible_fn(map_container, move || p.get() != PanelType::Servers);

    let top_panels = ui.child(widget, "TOP_PANELS_ROOT");
    let player_bin = ui.load_widget("LOBBY_PLAYER_BIN", Some(top_panels), &WidgetArgs::default());
    let p = panel.clone();
    ui.set_visible_fn(player_bin, move || p.get() == PanelType::Players);
    let players = ui.child(player_bin, "LOBBY_PLAYERS");
    let template_editable_player = take_template(ui, players, "TEMPLATE_EDITABLE_PLAYER");
    let template_noneditable_player = take_template(ui, players, "TEMPLATE_NONEDITABLE_PLAYER");
    let template_empty = take_template(ui, players, "TEMPLATE_EMPTY");
    let template_editable_spectator = take_template(ui, players, "TEMPLATE_EDITABLE_SPECTATOR");
    let template_noneditable_spectator = take_template(ui, players, "TEMPLATE_NONEDITABLE_SPECTATOR");
    let template_new_spectator = take_template(ui, players, "TEMPLATE_NEW_SPECTATOR");

    let mut chat_templates = HashMap::new();
    if let Some(templates) = logic_args.iter().find(|n| n.key == "ChatTemplates") {
        for item in &templates.value.nodes {
            if let Some(pool) = pool_from_name(&item.key) {
                let template = ui.load_widget(item.value.value_str(), None, &WidgetArgs::default());
                chat_templates.insert(pool, template);
            }
        }
    }

    let chat_panel = ui.child(widget, "CHAT_DISPLAY");
    ui.remove_children(chat_panel);
    let sounds = Rc::new(LobbySounds::new(ui, logic_args));
    let chat_text_field = ui.child(widget, "CHAT_TEXTFIELD");

    let lobby = Rc::new(Lobby {
        widget,
        net: net.clone(),
        skirmish_mode,
        on_start: on_start.clone(),
        on_exit: on_exit.clone(),
        panel: panel.clone(),
        map: map.clone(),
        map_status,
        rules: rules.clone(),
        factions: RefCell::default(),
        spawn_occupants,
        players,
        template_editable_player,
        template_noneditable_player,
        template_empty,
        template_editable_spectator,
        template_noneditable_spectator,
        template_new_spectator,
        chat_panel,
        chat_text_field,
        chat_templates,
        sounds: sounds.clone(),
        chat_enabled: Cell::new(false),
        disable_team_chat: Rc::new(Cell::new(false)),
        team_chat: Rc::new(Cell::new(false)),
        insufficient_player_spawns: Rc::new(Cell::new(false)),
        tab_names: Rc::default(),
        reset_options_enabled: Rc::new(Cell::new(false)),
        revision: Cell::new(u64::MAX),
        notifications_seen: Cell::new(net.borrow().notifications.len()),
        connection_lost: Cell::new(false),
    });

    let game_starting = Rc::new(Cell::new(false));
    let configuration_disabled: Getter<bool> = {
        let (l, starting) = (lobby.clone(), game_starting.clone());
        Rc::new(move || {
            let net = l.net.borrow();
            !net.is_host()
                || starting.get()
                || matches!(l.panel.get(), PanelType::Kick | PanelType::ForceStart)
                || !l.map_is_playable()
                || net.local_client().is_none_or(is_ready)
        })
    };

    if let Some(button) = ui.find(widget, "CHANGEMAP_BUTTON") {
        let p = panel.clone();
        ui.set_visible_fn(button, move || p.get() != PanelType::Servers);
        let (l, starting) = (lobby.clone(), game_starting.clone());
        ui.button(button).is_disabled = Some(Rc::new(move || {
            let net = l.net.borrow();
            starting.get() || matches!(l.panel.get(), PanelType::Kick | PanelType::ForceStart) || net.local_client().is_none_or(is_ready)
        }));
        let l = lobby.clone();
        ui.button(button).on_click = Some(Rc::new(move |ui| {
            let current = l.map.borrow().as_ref().map(|m| m.uid.clone());
            let (is_host, pool) = {
                let net = l.net.borrow();
                (net.is_host(), net.server_map_pool.clone())
            };
            let settings = services(ui).settings.clone();
            let cache = services(ui).maps.clone();
            let (n, previous) = (l.net.clone(), current.clone());
            let on_select: OnSelectMap = Rc::new(move |_, uid| {
                let available = cache.borrow().get(&uid).is_some_and(|m| m.status == MapStatus::Available);
                if previous.as_deref() == Some(&uid) || !available {
                    return;
                }
                n.borrow_mut().command(&format!("map {uid}"));
                let mut settings = settings.borrow_mut();
                settings.server.map = Some(uid);
                if let Err(e) = settings.save() {
                    log::warn!("Cannot save the settings: {e}");
                }
            });
            let mut args = WidgetArgs::default()
                .with_value("initialMap", current)
                .with_value("remoteMapPool", pool)
                .with_value("initialTab", MapClassification::System)
                .with_value("filter", MapVisibility::LOBBY);
            if is_host {
                args = args.with_value("onSelect", on_select);
            }
            ui.open_window("MAPCHOOSER_PANEL", args);
        }));
    }

    if let Some(button) = ui.find(widget, "SLOTS_DROPDOWNBUTTON") {
        let p = panel.clone();
        ui.set_visible_fn(button, move || p.get() != PanelType::Servers && p.get() != PanelType::Options);
        let (l, disabled) = (lobby.clone(), configuration_disabled.clone());
        ui.button(button).is_disabled = Some(Rc::new(move || {
            let net = l.net.borrow();
            let session = &net.session;
            disabled()
                || l.panel.get() != PanelType::Players
                || (session.slots.values().all(|s| !s.allow_bots)
                    && !session
                        .slots
                        .iter()
                        .any(|(k, s)| !s.lock_team && session.client_in_slot(k).is_some()))
        }));
        let l = lobby.clone();
        ui.button(button).on_mouse_down = Some(Rc::new(move |ui, _| show_slots_dropdown(&l, ui, button)));
    }

    if let Some(button) = ui.find(widget, "RESET_OPTIONS_BUTTON") {
        ui.set_visible_fn(button, lobby.panel_is(PanelType::Options));
        let (enabled, disabled) = (lobby.reset_options_enabled.clone(), configuration_disabled.clone());
        ui.button(button).is_disabled = Some(Rc::new(move || disabled() || !enabled.get()));
        let n = net.clone();
        ui.button(button).on_mouse_down = Some(Rc::new(move |_, _| n.borrow_mut().command("reset_options")));
    }

    let r = rules.clone();
    let get_rules: GetRules = Rc::new(move || r.borrow().clone());
    let options_args = WidgetArgs::default()
        .with_value("getRules", get_rules)
        .with_value("configurationDisabled", configuration_disabled.clone());
    let options_bin = ui.load_widget("LOBBY_OPTIONS_BIN", Some(top_panels), &options_args);
    ui.set_visible_fn(options_bin, lobby.panel_is(PanelType::Options));

    let music_bin = ui.load_widget("LOBBY_MUSIC_BIN", Some(top_panels), &WidgetArgs::default());
    ui.set_visible_fn(music_bin, lobby.panel_is(PanelType::Music));

    let refresh_server_list: super::multiplayer::RefreshServerList = Rc::default();
    if !skirmish_mode {
        let args = WidgetArgs::default().with_value("refreshServerList", refresh_server_list.clone());
        let servers_bin = ui.load_widget("LOBBY_SERVERS_BIN", Some(top_panels), &args);
        ui.set_visible_fn(servers_bin, lobby.panel_is(PanelType::Servers));
    }

    let tab_container = ui.child(widget, if skirmish_mode { "SKIRMISH_TABS" } else { "MULTIPLAYER_TABS" });
    ui.set_visible_fn(tab_container, || true);
    let tab = |ui: &mut Ui, name: &str, kind: PanelType, extra_disabled: Option<Getter<bool>>| {
        let Some(button) = ui.find(tab_container, name) else {
            return;
        };
        ui.button(button).is_highlighted = Some(Rc::new(lobby.panel_is(kind)));
        let p = panel.clone();
        ui.button(button).is_disabled = Some(Rc::new(move || {
            matches!(p.get(), PanelType::Kick | PanelType::ForceStart) || extra_disabled.as_ref().is_some_and(|f| f())
        }));
        ui.button(button).on_click = Some(lobby.set_panel(kind));
    };
    let l = lobby.clone();
    tab(ui, "OPTIONS_TAB", PanelType::Options, Some(Rc::new(move || !l.map_is_playable())));
    tab(ui, "PLAYERS_TAB", PanelType::Players, None);
    tab(ui, "MUSIC_TAB", PanelType::Music, None);
    tab(ui, "SERVERS_TAB", PanelType::Servers, None);
    if let Some(servers_tab) = ui.find(tab_container, "SERVERS_TAB") {
        let (p, set_servers) = (panel.clone(), lobby.set_panel(PanelType::Servers));
        ui.button(servers_tab).on_click = Some(Rc::new(move |ui| {
            if p.get() != PanelType::Servers
                && let Some(refresh) = refresh_server_list.borrow().clone()
            {
                refresh(ui);
            }
            set_servers(ui);
        }));
    }

    let start_game: Action = {
        let (l, starting) = (lobby.clone(), game_starting.clone());
        Rc::new(move |_| {
            if l.map.borrow().as_ref().is_some_and(|m| m.status == MapStatus::Available) {
                starting.set(true);
                l.net.borrow_mut().command("startgame");
            }
        })
    };
    let start_disabled: Getter<bool> = {
        let l = lobby.clone();
        Rc::new(move || {
            let net = l.net.borrow();
            let session = &net.session;
            l.map.borrow().as_ref().is_none_or(|m| m.status != MapStatus::Available)
                || session.slots.iter().any(|(k, s)| s.required && session.client_in_slot(k).is_none())
                || session.slots.keys().all(|k| session.client_in_slot(k).is_none())
                || (!session.global_settings.enable_singleplayer && session.non_bot_players().count() < 2)
                || l.insufficient_player_spawns.get()
        })
    };
    if let Some(button) = ui.find(widget, "START_GAME_BUTTON") {
        let (disabled, not_startable) = (configuration_disabled.clone(), start_disabled.clone());
        ui.button(button).is_disabled = Some(Rc::new(move || disabled() || not_startable()));
        let (n, p, start) = (net.clone(), panel.clone(), start_game.clone());
        ui.button(button).on_click = Some(Rc::new(move |ui| {
            let unready = n
                .borrow()
                .session
                .clients
                .iter()
                .any(|c| c.slot.is_some() && !c.is_admin && !c.is_bot() && !is_ready(c));
            if unready {
                p.set(PanelType::ForceStart);
            } else {
                start(ui);
            }
        }));
    }

    let force_start = ui.load_widget("FORCE_START_DIALOG", Some(top_panels), &WidgetArgs::default());
    ui.set_visible_fn(force_start, lobby.panel_is(PanelType::ForceStart));
    let warning = ui.child(force_start, "KICK_WARNING");
    let n = net.clone();
    ui.set_visible_fn(warning, move || {
        n.borrow()
            .session
            .clients
            .iter()
            .any(|c| c.state == rustora_net::session::ClientState::Invalid)
    });
    let ok = ui.child(force_start, "OK_BUTTON");
    ui.button(ok).on_click = Some(start_game);
    ui.button(ok).is_disabled = Some(start_disabled);
    let cancel = ui.child(force_start, "CANCEL_BUTTON");
    ui.button(cancel).on_click = Some(lobby.set_panel(PanelType::Players));

    let disconnect = ui.child(widget, "DISCONNECT_BUTTON");
    let exit = on_exit.clone();
    ui.button(disconnect).on_click = Some(Rc::new(move |ui| {
        ui.close_window();
        exit(ui);
        play_ui_sound(ui, &sounds.player_left);
    }));
    if skirmish_mode {
        let back = message(ui, "button-back");
        ui.button(disconnect).get_text = Some(Rc::new(move || back.clone()));
    }

    let chat_mode = ui.child(widget, "CHAT_MODE");
    let (team, all) = (message(ui, "button-team-chat"), message(ui, "button-general-chat"));
    let t = lobby.team_chat.clone();
    ui.button(chat_mode).get_text = Some(Rc::new(move || if t.get() { team.clone() } else { all.clone() }));
    let t = lobby.team_chat.clone();
    ui.button(chat_mode).on_click = Some(Rc::new(move |_| t.set(!t.get())));
    let l = lobby.clone();
    ui.button(chat_mode).is_disabled = Some(Rc::new(move || l.disable_team_chat.get() || !l.chat_enabled.get()));

    {
        let mut field = ui.get::<TextFieldWidget>(chat_text_field);
        let l = lobby.clone();
        field.is_disabled = Some(Rc::new(move || !l.chat_enabled.get()));
        field.max_length = CHAT_MESSAGE_MAX_LENGTH;
        let l = lobby.clone();
        let on_enter: KeyCallback = Rc::new(move |ui, _| {
            let text = ui.get::<TextFieldWidget>(chat_text_field).text();
            if text.is_empty() {
                return true;
            }
            ui.scroll_to_bottom(chat_panel, false);
            let team = if l.team_chat.get() {
                l.net
                    .borrow()
                    .local_client()
                    .map_or(0, |c| if c.is_observer() { u32::MAX } else { c.team as u32 })
            } else {
                0
            };
            l.net.borrow_mut().chat(&text, team);
            ui.get::<TextFieldWidget>(chat_text_field).set_text("");
            true
        });
        field.on_enter_key = Some(on_enter);
        let names = lobby.tab_names.clone();
        field.on_tab_key = Some(Rc::new(move |ui, _| {
            let mut field = ui.get::<TextFieldWidget>(chat_text_field);
            let completed = complete_name(&field.text(), &names.borrow());
            field.cursor_position = completed.chars().count();
            field.set_text(&completed);
            true
        }));
        field.on_esc_key = Some(Rc::new(move |ui, _| ui.yield_keyboard_focus(chat_text_field)));
    }

    let start = on_start.clone();
    net.borrow_mut().before_game_start = Some(Rc::new(move |ui| {
        ui.close_window();
        start(ui);
    }));

    lobby.update_current_map(ui);
    cell(LobbyHandle(lobby))
}
