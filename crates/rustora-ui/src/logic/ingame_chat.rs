// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `LoadIngameChatLogic.cs`, `IngameChatLogic.cs` and `GameTimerLogic.cs`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use rustora_core::miniyaml::MiniYamlNode;
use rustora_engine::player::WinState;

use super::lobby::{complete_name, pool_from_name, setup_text_notification};
use super::{NoLogic, message, message_with, services};
use crate::fluent::MessageArg;
use crate::input::{KeyEvent, KeyInput};
use crate::loader::cell;
use crate::net::{Net, SYSTEM_CLIENT_ID, TextNotification, TextNotificationPool};
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::TextFieldWidget;
use crate::widgets::production::{LogicKeyListenerWidget, format_time};
use crate::widgets::text_notifications::TextNotificationsDisplayWidget;

/// `UnitOrders.ChatMessageMaxLength`.
const CHAT_MESSAGE_MAX_LENGTH: usize = 2500;

/// `LoadIngameChatLogic`.
pub fn load_ingame_chat(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let root = ui.child(id, "CHAT_ROOT");
    ui.load_widget("CHAT_PANEL", Some(root), &WidgetArgs::default().with_value("isMenuChat", false));
    cell(NoLogic)
}

fn is_notification_eligible(notification: &TextNotification) -> bool {
    matches!(
        notification.pool,
        TextNotificationPool::Chat | TextNotificationPool::System | TextNotificationPool::Mission
    )
}

struct IngameChatLogic {
    net: Net,
    is_replay: bool,
    is_menu_chat: bool,
    overlay_display: Option<WidgetId>,
    chrome: WidgetId,
    scroll_panel: WidgetId,
    text: WidgetId,
    templates: HashMap<TextNotificationPool, WidgetId>,
    chat_enabled: Rc<Cell<bool>>,
    chat_disabled: String,
    notifications_seen: usize,
    world: Rc<rustora_engine::world::World>,
    chat_line_sound: String,
    hide_replay_chat: Cell<bool>,
}

/// `OpenChat`.
fn open_chat(ui: &mut Ui, chrome: WidgetId, text: WidgetId, scroll_panel: WidgetId, overlay: Option<WidgetId>) {
    ui.get::<TextFieldWidget>(text).set_text("");
    ui.node_mut(chrome).visible = true;
    ui.scroll_to_bottom(scroll_panel, false);
    if !ui.get::<TextFieldWidget>(text).disabled_now() {
        ui.take_keyboard_focus(text);
    }
    if let Some(overlay) = overlay {
        ui.node_mut(overlay).visible = false;
    }
}

/// `CloseChat`.
fn close_chat(ui: &mut Ui, chrome: WidgetId, text: WidgetId, overlay: Option<WidgetId>) {
    ui.node_mut(chrome).visible = false;
    ui.yield_keyboard_focus(text);
    if let Some(overlay) = overlay {
        ui.node_mut(overlay).visible = true;
    }
    ui.reset_tooltips();
}

impl IngameChatLogic {
    /// `AddNotification`.
    fn add_notification(&self, ui: &mut Ui, notification: &TextNotification, suppress_sound: bool) {
        let Some(&template) = self.templates.get(&notification.pool) else {
            return;
        };
        let line = ui.clone_widget(template);
        let width = ui.node(self.scroll_panel).bounds.w - ui.get::<crate::widgets::ScrollPanelWidget>(self.scroll_panel).scrollbar_width;
        setup_text_notification(ui, line, notification, width, self.is_menu_chat && !self.is_replay);
        let at_bottom = ui.scrolled_to_bottom(self.scroll_panel);
        ui.add_child(self.scroll_panel, line);
        if at_bottom {
            ui.scroll_to_bottom(self.scroll_panel, true);
        }
        if !suppress_sound && !self.is_muted(notification) {
            rustora_engine::sound::play_notification(&self.world, None, "Sounds", &self.chat_line_sound, None);
        }
    }

    /// `IsNotificationMuted`.
    fn is_muted(&self, notification: &TextNotification) -> bool {
        self.is_replay && notification.client_id != SYSTEM_CLIENT_ID && self.hide_replay_chat.get()
    }
}

impl ChromeLogic for IngameChatLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let (fresh, disabled_until) = {
            let net = self.net.borrow();
            let start = self.notifications_seen.min(net.notifications.len());
            (net.notifications[start..].to_vec(), net.chat_disabled_until)
        };
        self.notifications_seen += fresh.len();
        self.hide_replay_chat.set(services(ui).settings.borrow().game.hide_replay_chat);
        for notification in fresh.iter().filter(|n| is_notification_eligible(n)) {
            if let Some(display) = self.overlay_display.filter(|_| !self.is_muted(notification)) {
                TextNotificationsDisplayWidget::add_notification(ui, display, notification);
            }
            self.add_notification(ui, notification, self.overlay_display.is_none());
        }

        let run_time = ui.run_time();
        let was_enabled = self.chat_enabled.get();
        let enabled = self.is_replay || (run_time >= disabled_until && disabled_until != i64::MAX);
        self.chat_enabled.set(enabled);
        if enabled && !was_enabled {
            ui.get::<TextFieldWidget>(self.text).set_text("");
            if ui.keyboard_focus.is_none() && ui.node(self.chrome).visible {
                ui.take_keyboard_focus(self.text);
            }
        } else if !enabled {
            let remaining = if disabled_until == i64::MAX {
                0
            } else {
                (disabled_until - run_time + 999) / 1000
            };
            let text = if remaining == 0 {
                self.chat_disabled.clone()
            } else {
                message_with(ui, "label-chat-availability", &[("seconds", MessageArg::Number(remaining))])
            };
            ui.get::<TextFieldWidget>(self.text).set_text(&text);
        }
    }
}

/// `IngameChatLogic`.
pub fn ingame_chat(ui: &mut Ui, id: WidgetId, args: &WidgetArgs, logic_args: &[MiniYamlNode]) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let net = services(ui).net.clone();
    let is_menu_chat = args.value::<bool>("isMenuChat").unwrap_or(false);
    let world = ingame.world.clone();
    let local_player = ingame.local_player.clone();
    let others = {
        let local = local_player.clone();
        world
            .players()
            .into_iter()
            .filter(move |p| local.as_ref().is_none_or(|l| !Rc::ptr_eq(l, p)) && !p.non_combatant && !p.is_bot)
            .collect::<Vec<_>>()
    };
    let (is_observer, local_team, tab_names) = {
        let n = net.borrow();
        let names: Vec<String> = {
            let mut names = Vec::new();
            for c in n.session.clients.iter().filter(|c| c.bot.is_none()) {
                if let Some(name) = c.name.clone().filter(|name| !names.contains(name)) {
                    names.push(name);
                }
            }
            names
        };
        (
            n.local_client().is_some_and(|c| c.is_observer()),
            n.local_client().map_or(0, |c| c.team),
            names,
        )
    };
    let non_bot_clients = world.lobby_info.clients.iter().filter(|c| c.bot.is_none()).count();
    let always_disabled = world.is_replay() || non_bot_clients == 1;
    let disable_team_chat = Rc::new(Cell::new(
        always_disabled
            || local_player
                .as_ref()
                .is_some_and(|l| !others.iter().any(|p| p.is_allied_with(Some(&**l)))),
    ));
    let team_chat = Rc::new(Cell::new(!disable_team_chat.get()));
    let (team_message, all_message) = (message(ui, "button-team-chat"), message(ui, "button-general-chat"));

    let mut templates = HashMap::new();
    if let Some(nodes) = logic_args.iter().find(|n| n.key == "Templates") {
        for item in &nodes.value.nodes {
            if let Some(pool) = pool_from_name(&item.key) {
                templates.insert(pool, ui.load_widget(item.value.value_str(), None, &WidgetArgs::default()));
            }
        }
    }

    let overlay = ui.find(id, "CHAT_OVERLAY");
    let overlay_display = overlay.map(|o| ui.child(o, "CHAT_DISPLAY"));
    if let Some(overlay) = overlay {
        ui.node_mut(overlay).visible = false;
    }
    let chrome = ui.child(id, "CHAT_CHROME");
    ui.node_mut(chrome).visible = true;
    let chat_enabled = Rc::new(Cell::new(false));
    let chat_mode = ui.child(chrome, "CHAT_MODE");
    {
        let (t, d) = (team_chat.clone(), disable_team_chat.clone());
        let mut b = ui.button(chat_mode);
        b.get_text = Some(Rc::new(move || {
            if t.get() && !d.get() {
                team_message.clone()
            } else {
                all_message.clone()
            }
        }));
        let t = team_chat.clone();
        b.on_click = Some(Rc::new(move |_ui: &mut Ui| t.set(!t.get())));
        let (d, enabled, world, local) = (disable_team_chat.clone(), chat_enabled.clone(), world.clone(), local_player.clone());
        let others = others.clone();
        b.is_disabled = Some(Rc::new(move || {
            let Some(local) = local.as_ref().filter(|_| !always_disabled) else {
                return d.get() || !enabled.get();
            };
            if world.is_game_over() || !enabled.get() {
                return true;
            }
            if local.win_state.get() != WinState::Undefined {
                d.set(false);
                return false;
            }
            if !d.get() {
                d.set(
                    others
                        .iter()
                        .all(|p| p.win_state.get() != WinState::Undefined || !p.is_allied_with(Some(&**local))),
                );
            }
            d.get()
        }));
    }

    let text = ui.child(chrome, "CHAT_TEXTFIELD");
    let scroll_panel = ui.child(chrome, "CHAT_SCROLLPANEL");
    {
        let mut field = ui.get::<TextFieldWidget>(text);
        field.max_length = CHAT_MESSAGE_MAX_LENGTH;
        let (t, d, n, local) = (team_chat.clone(), disable_team_chat.clone(), net.clone(), local_player.clone());
        field.on_enter_key = Some(Rc::new(move |ui: &mut Ui, _e: &KeyInput| {
            let team = t.get() && !d.get();
            let content = ui.get::<TextFieldWidget>(text).text();
            if !content.is_empty() {
                if content.starts_with('/') {
                    if let Some(ingame) = services(ui).ingame.borrow().clone() {
                        super::chat_commands::on_chat(ui, &ingame, content.trim());
                    }
                } else {
                    let finished = local.as_ref().is_some_and(|l| l.win_state.get() != WinState::Undefined);
                    let team_number = match (team, is_observer || finished) {
                        (false, _) => 0,
                        (true, true) => u32::MAX,
                        (true, false) => local_team as u32,
                    };
                    n.borrow_mut().chat(content.trim(), team_number);
                }
            }
            ui.get::<TextFieldWidget>(text).set_text("");
            if !is_menu_chat {
                close_chat(ui, chrome, text, overlay);
            }
            true
        }));
        field.on_tab_key = Some(Rc::new(move |ui: &mut Ui, e: &KeyInput| {
            let activated = {
                let key = ui.button(chat_mode).key.clone();
                key.is_some_and(|k| ui.assets.borrow().hotkeys.is_activated(&k, e))
            };
            if activated && !ui.button(chat_mode).disabled_now() {
                let action = ui.button(chat_mode).on_click.clone();
                if let Some(action) = action {
                    action(ui);
                }
            } else {
                let mut field = ui.get::<TextFieldWidget>(text);
                let completed = complete_name(&field.text(), &tab_names);
                field.cursor_position = completed.chars().count();
                field.set_text(&completed);
            }
            true
        }));
        field.on_esc_key = Some(Rc::new(move |ui: &mut Ui, _e: &KeyInput| {
            if is_menu_chat {
                ui.yield_keyboard_focus(text);
            } else {
                close_chat(ui, chrome, text, overlay);
            }
            true
        }));
        let (enabled, is_replay) = (chat_enabled.clone(), world.is_replay());
        field.is_disabled = Some(Rc::new(move || !enabled.get() || is_replay));
    }

    if !is_menu_chat {
        let key_of = |name: &str| {
            logic_args
                .iter()
                .find(|n| n.key == name)
                .and_then(|n| n.value.value.clone())
                .unwrap_or_default()
        };
        let (open_team, open_general) = (key_of("OpenTeamChatKey"), key_of("OpenGeneralChatKey"));
        let close = ui.child(chrome, "CHAT_CLOSE");
        ui.button(close).on_click = Some(Rc::new(move |ui: &mut Ui| close_chat(ui, chrome, text, overlay)));
        let listener = ui.child(id, "OPEN_CHAT_KEY_LISTENER");
        let (t, d) = (team_chat.clone(), disable_team_chat.clone());
        ui.get::<LogicKeyListenerWidget>(listener)
            .handlers
            .push(Rc::new(move |ui: &mut Ui, e: &KeyInput| {
                if e.event == KeyEvent::Up || ui.node(chrome).visible {
                    return false;
                }
                let (team_key, general_key) = {
                    let assets = ui.assets.borrow();
                    (
                        assets.hotkeys.is_activated(&open_team, e),
                        assets.hotkeys.is_activated(&open_general, e),
                    )
                };
                if !team_key && !general_key {
                    return false;
                }
                t.set(!d.get() && !general_key);
                ui.defer(move |ui| open_chat(ui, chrome, text, scroll_panel, overlay));
                true
            }));
    }

    ui.remove_children(scroll_panel);
    ui.scroll_to_bottom(scroll_panel, false);

    let chat_disabled = message(ui, "label-chat-disabled");
    let existing: Vec<TextNotification> = net.borrow().notifications.clone();
    let logic = IngameChatLogic {
        net: net.clone(),
        is_replay: world.is_replay(),
        is_menu_chat,
        overlay_display,
        chrome,
        scroll_panel,
        text,
        templates,
        chat_enabled,
        chat_disabled,
        notifications_seen: existing.len(),
        world: world.clone(),
        chat_line_sound: ui.assets.borrow().metrics.string("ChatLineSound"),
        hide_replay_chat: Cell::new(services(ui).settings.borrow().game.hide_replay_chat),
    };
    for notification in existing.iter().filter(|n| is_notification_eligible(n)) {
        logic.add_notification(ui, notification, true);
    }
    if !is_menu_chat {
        close_chat(ui, chrome, text, overlay);
        let listener = ui.child(chrome, "KEY_LISTENER");
        ui.get::<LogicKeyListenerWidget>(listener)
            .handlers
            .push(Rc::new(move |ui: &mut Ui, e: &KeyInput| {
                if e.event == KeyEvent::Up || !ui.get::<TextFieldWidget>(text).disabled_now() {
                    return false;
                }
                let is_close = matches!(e.key.as_str(), "RETURN" | "KP_ENTER" | "ESCAPE") && e.modifiers == Default::default();
                if is_close {
                    ui.defer(move |ui| close_chat(ui, chrome, text, overlay));
                }
                is_close
            }));
    }
    cell(logic)
}

/// `GameTimerLogic`, showing the elapsed time or the time left of the time limit. The RA chrome has no status label.
pub fn game_timer(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let Some(timer) = ui.find(id, "GAME_TIMER").or(Some(id)) else {
        return cell(NoLogic);
    };
    let paused_text = message(ui, "label-paused");
    let world = ingame.world.clone();
    let start = Instant::now();
    let time_limit = world
        .world_actor()
        .trait_or_default::<rustora_mods_common::player::objectives::TimeLimitManager>()
        .map_or(0, |t| t.time_limit.get());
    ui.label(timer).get_text = Some(Rc::new(move || {
        let paused = world.paused() || world.replay_timestep.get() == 0;
        let show_status = (world.paused() || world.replay_timestep.get() != world.timestep) && start.elapsed().as_secs() % 2 == 0;
        if paused && show_status {
            return paused_text.clone();
        }
        let display_tick = if time_limit > 0 {
            time_limit - world.world_tick()
        } else {
            world.world_tick()
        };
        format_time(display_tick.max(0), world.timestep)
    }));
    let is_tooltip_label = ui.kind(timer).borrow_mut().as_any().is::<crate::widgets::LabelWithTooltipWidget>();
    if is_tooltip_label {
        let complete = |ui: &Ui, p: i32| message_with(ui, "label-replay-complete", &[("percentage", MessageArg::Number(p as i64))]);
        let texts: Vec<String> = (0..=100).map(|p| complete(ui, p)).collect();
        let world = ingame.world.clone();
        let getter: Option<crate::widget::Getter<String>> = match &ingame.replay {
            Some(r) if r.final_game_tick != 0 => {
                let total = r.final_game_tick;
                Some(Rc::new(move || {
                    texts[(world.world_tick() * 100 / total).clamp(0, 100) as usize].clone()
                }))
            }
            Some(r) if r.tick_count != 0 => {
                let total = r.tick_count;
                Some(Rc::new(move || {
                    texts[(world.net_frame_number() * 100 / total).clamp(0, 100) as usize].clone()
                }))
            }
            _ => None,
        };
        ui.get::<crate::widgets::LabelWithTooltipWidget>(timer).get_tooltip_text = getter;
    }
    cell(NoLogic)
}
