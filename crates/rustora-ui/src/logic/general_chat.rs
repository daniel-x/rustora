// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The general chat of rustora: an IRC channel on Libera.Chat, shown beside the server list and the multiplayer
//! lobby. It is connected while one of them is open and disconnected otherwise, also while playing.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use rustora_client::irc::{self, IrcClient, IrcEvent};

use crate::logic::services;
use crate::net::{JoinTarget, SYSTEM_CLIENT_ID, TextNotification, TextNotificationPool, local_time};
use crate::widget::{ChromeLogic, KeyCallback, Ui, WidgetArgs, WidgetId};
use crate::widgets::TextFieldWidget;

/// The distance of the server list and the lobby from the upper left corner, and of the chat from the window
/// edges.
const MARGIN: i32 = 10;
/// The width of the server list and the lobby.
const DIALOG_WIDTH: i32 = 900;
const DIALOG_HEIGHT: i32 = 600;
/// Below the logo and the version label of the main menu.
const CHAT_Y: i32 = 306;
const MIN_CHAT_WIDTH: i32 = 280;
const MIN_CHAT_HEIGHT: i32 = 120;
const HISTORY_LENGTH: usize = 500;
/// Milliseconds before a lost connection is tried again.
const RETRY_DELAY: i64 = 30_000;

/// The connection and the lines of the general chat, kept across the menus that show it.
#[derive(Default)]
pub struct GeneralChat {
    irc: Option<IrcClient>,
    /// The own nick once the channel is joined.
    nick: Option<String>,
    retry_at: i64,
    lines: VecDeque<TextNotification>,
    /// The count of lines ever added, including the ones dropped from `lines`.
    total: usize,
}

impl GeneralChat {
    fn push(&mut self, prefix: Option<String>, text: String, pool: TextNotificationPool) {
        if self.lines.len() == HISTORY_LENGTH {
            self.lines.pop_front();
        }
        self.lines.push_back(TextNotification {
            pool,
            client_id: SYSTEM_CLIENT_ID,
            prefix,
            text,
            prefix_color: None,
            text_color: None,
            time: local_time(),
        });
        self.total += 1;
    }

    fn connect(&mut self, name: &str) {
        self.irc = Some(IrcClient::connect(name));
        self.push(
            None,
            format!("Connecting to {} on Libera.Chat.", irc::CHANNEL),
            TextNotificationPool::System,
        );
    }

    fn disconnect(&mut self) {
        if self.irc.take().is_some() {
            self.nick = None;
            self.retry_at = 0;
            self.push(None, "Disconnected.".into(), TextNotificationPool::System);
        }
    }

    fn poll(&mut self, now: i64) {
        let events = self.irc.as_ref().map(IrcClient::poll).unwrap_or_default();
        for event in events {
            match event {
                IrcEvent::Joined(nick) => {
                    self.push(None, format!("Joined as {nick}."), TextNotificationPool::System);
                    self.nick = Some(nick);
                }
                IrcEvent::Message { nick, text, private } => {
                    let name = irc::display_name(&nick);
                    let prefix = if private { format!("{name} (private)") } else { name.to_string() };
                    self.push(Some(prefix), text, TextNotificationPool::Chat);
                }
                IrcEvent::Disconnected(reason) => {
                    self.irc = None;
                    self.nick = None;
                    self.retry_at = now + RETRY_DELAY;
                    self.push(None, format!("Disconnected: {reason}"), TextNotificationPool::System);
                }
            }
        }
    }

    fn send(&mut self, text: &str) -> bool {
        let (Some(irc), Some(nick)) = (&self.irc, &self.nick) else {
            return false;
        };
        irc.send(text);
        let name = irc::display_name(nick).to_string();
        self.push(Some(name), text.to_string(), TextNotificationPool::Chat);
        true
    }
}

/// Moves the server list or the lobby to the upper left corner, to make room for the general chat.
pub fn place_dialog(ui: &mut Ui, window: WidgetId) {
    let bounds = &mut ui.node_mut(window).bounds;
    bounds.x = MARGIN;
    bounds.y = MARGIN;
}

/// The arguments that `GENERAL_CHAT` is loaded with: the place of the chat. It is right of the dialogs, from below
/// the logo down, or below the dialogs when the window is too narrow for that.
pub fn widget_args(ui: &Ui) -> WidgetArgs {
    let (window_width, window_height) = ui.resolution;
    let right_x = MARGIN + DIALOG_WIDTH + MARGIN;
    let below_y = MARGIN + DIALOG_HEIGHT + MARGIN;
    let (x, y, width) = if window_width - right_x - MARGIN >= MIN_CHAT_WIDTH {
        (right_x, CHAT_Y, window_width - right_x - MARGIN)
    } else if window_height - below_y - MARGIN >= MIN_CHAT_HEIGHT {
        (MARGIN, below_y, window_width - 2 * MARGIN)
    } else {
        (window_width - MIN_CHAT_WIDTH - MARGIN, CHAT_Y, MIN_CHAT_WIDTH)
    };
    let mut args = WidgetArgs::default();
    args.substitutions.insert("CHAT_X".into(), x);
    args.substitutions.insert("CHAT_Y".into(), y);
    args.substitutions.insert("CHAT_WIDTH".into(), width);
    args.substitutions.insert("CHAT_HEIGHT".into(), window_height - y - MARGIN);
    args
}

/// Whether the server list or a multiplayer lobby is open, also below other windows.
fn multiplayer_menu_open(ui: &Ui) -> bool {
    let multiplayer_lobby = services(ui).net.borrow().target != JoinTarget::SinglePlayer;
    ui.windows().iter().any(|&w| match ui.node(w).id.as_deref() {
        Some("MULTIPLAYER_PANEL") => true,
        Some("SERVER_LOBBY") => multiplayer_lobby,
        _ => false,
    })
}

struct GeneralChatLogic {
    panel: WidgetId,
    display: WidgetId,
    templates: [(TextNotificationPool, WidgetId); 2],
    seen: usize,
}

impl GeneralChatLogic {
    fn add_line(&self, ui: &mut Ui, line: &TextNotification) {
        let Some(&(_, template)) = self.templates.iter().find(|(pool, _)| *pool == line.pool) else {
            return;
        };
        let widget = ui.clone_widget(template);
        let width = ui.node(self.display).bounds.w - ui.get::<crate::widgets::ScrollPanelWidget>(self.display).scrollbar_width;
        crate::logic::lobby::setup_text_notification(ui, widget, line, width, true);
        let at_bottom = ui.scrolled_to_bottom(self.display);
        ui.add_child(self.display, widget);
        if at_bottom {
            ui.scroll_to_bottom(self.display, true);
        }
    }

    /// Whether the top window leaves the chat uncovered. The server list and the lobby do not count: they may
    /// overlap the chat in small windows.
    fn uncovered(&self, ui: &Ui) -> bool {
        let chat = ui.render_bounds(self.panel);
        ui.current_window().is_none_or(|w| {
            matches!(ui.node(w).id.as_deref(), Some("MULTIPLAYER_PANEL" | "SERVER_LOBBY")) || !ui.render_bounds(w).intersects(chat)
        })
    }
}

impl ChromeLogic for GeneralChatLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let services = services(ui);
        let wanted = multiplayer_menu_open(ui);
        let now = ui.run_time();
        let new_lines = {
            let mut chat = services.general_chat.borrow_mut();
            if !wanted {
                chat.disconnect();
            } else if chat.irc.is_none() && now >= chat.retry_at {
                let name = services.settings.borrow().player.name.clone();
                chat.connect(&name);
            }
            chat.poll(now);
            let skip = (chat.lines.len() + self.seen).saturating_sub(chat.total);
            let lines: Vec<_> = chat.lines.iter().skip(skip).cloned().collect();
            self.seen = chat.total;
            lines
        };
        for line in &new_lines {
            self.add_line(ui, line);
        }
        let visible = wanted && self.uncovered(ui);
        ui.node_mut(self.panel).visible = visible;
    }

    fn disposed(&mut self, ui: &mut Ui) {
        services(ui).general_chat.borrow_mut().disconnect();
        for (_, template) in self.templates {
            ui.dispose_detached(template);
        }
    }
}

/// `GeneralChatLogic`'s constructor.
pub fn create(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let panel = ui.child(widget, "GENERAL_CHAT_PANEL");
    ui.node_mut(panel).visible = false;
    let title = ui.child(panel, "TITLE");
    ui.label(title).get_text = Some(Rc::new(|| format!("General Chat - {}", irc::CHANNEL)));
    let display = ui.child(panel, "CHAT_DISPLAY");
    ui.remove_children(display);
    let templates = [
        (
            TextNotificationPool::Chat,
            ui.load_widget("CHAT_LINE_TEMPLATE", None, &WidgetArgs::default()),
        ),
        (
            TextNotificationPool::System,
            ui.load_widget("SYSTEM_LINE_TEMPLATE", None, &WidgetArgs::default()),
        ),
    ];
    let text_field = ui.child(panel, "CHAT_TEXTFIELD");
    {
        let mut field = ui.get::<TextFieldWidget>(text_field);
        let on_enter: KeyCallback = Rc::new(move |ui, _| {
            let text = ui.get::<TextFieldWidget>(text_field).text();
            if !text.is_empty() && services(ui).general_chat.borrow_mut().send(&text) {
                ui.scroll_to_bottom(display, false);
                ui.get::<TextFieldWidget>(text_field).set_text("");
            }
            true
        });
        field.on_enter_key = Some(on_enter);
        field.on_esc_key = Some(Rc::new(move |ui, _| ui.yield_keyboard_focus(text_field)));
    }
    let logic = GeneralChatLogic {
        panel,
        display,
        templates,
        seen: 0,
    };
    let history: Vec<_> = services(ui).general_chat.borrow().lines.iter().cloned().collect();
    for line in &history {
        logic.add_line(ui, line);
    }
    let seen = services(ui).general_chat.borrow().total;
    Rc::new(RefCell::new(GeneralChatLogic { seen, ..logic }))
}
