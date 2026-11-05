// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The lobby part of `OrderManager` and `TextNotificationsManager`, as logic classes see them. The client binary owns
//! the socket: it feeds this state and sends what `outgoing` holds.

use std::rc::Rc;

use indexmap::IndexMap;
use rustora_core::fieldloader::Color;
use rustora_net::order::RawOrder;
use rustora_net::session::{Client, Session};

/// The join target that stands for the server of the last `AppRequest::CreateLocalServer`.
pub const LOCAL_SERVER_TARGET: &str = "{local}";

/// `ConnectionState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionState {
    #[default]
    NotConnected,
    Connecting,
    Connected,
}

/// `TextNotificationPool`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextNotificationPool {
    Chat,
    Join,
    Leave,
    System,
    Mission,
    Feedback,
    Transients,
}

/// `TextNotification`.
#[derive(Clone, Debug)]
pub struct TextNotification {
    pub pool: TextNotificationPool,
    pub prefix: Option<String>,
    pub text: String,
    pub prefix_color: Option<Color>,
    pub text_color: Option<Color>,
    /// Hour and minute of the local time.
    pub time: (u32, u32),
}

/// `Traits.LobbyOption` with the text shown to players.
#[derive(Clone, Debug)]
pub struct LobbyOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub values: IndexMap<String, String>,
    pub default_value: String,
    pub is_locked: bool,
    pub is_visible: bool,
    pub display_order: i32,
    pub is_boolean: bool,
}

/// `LobbyFaction`. The texts are message keys.
#[derive(Clone, Debug, Default)]
pub struct LobbyFaction {
    pub selectable: bool,
    pub name: String,
    pub description: Option<String>,
    pub side: Option<String>,
}

/// What the lobby reads from the rules of a map.
#[derive(Clone, Debug, Default)]
pub struct LobbyRules {
    pub options: Vec<LobbyOption>,
    /// The type and the name message key of each bot.
    pub bots: Vec<(String, String)>,
    pub factions: IndexMap<String, LobbyFaction>,
}

/// The connection and lobby state of the client.
#[derive(Default)]
pub struct NetState {
    pub session: Session,
    pub local_client_id: i32,
    pub connection_state: ConnectionState,
    pub target: String,
    pub password: String,
    pub server_error: Option<String>,
    pub authentication_failed: bool,
    pub error_message: Option<String>,
    pub server_map_pool: Option<Vec<String>>,
    /// Grows with every change of `session`, in place of the `Game.LobbyInfoChanged` event.
    pub revision: u64,
    /// `Game.RunTime` until which chat is disabled, `i64::MAX` until further notice.
    pub chat_disabled_until: i64,
    pub notifications: Vec<TextNotification>,
    /// `TextNotificationsManager.MutedPlayers`: the client indexes whose chat is hidden.
    pub muted_players: std::collections::HashSet<i32>,
    /// `UnitOrders.KickVoteTarget`.
    pub kick_vote_target: Option<i32>,
    pub outgoing: Vec<RawOrder>,
    /// The `Game.BeforeGameStart` handler.
    pub before_game_start: Option<crate::widget::Action>,
}

impl NetState {
    /// `OrderManager.LocalClient`.
    pub fn local_client(&self) -> Option<&Client> {
        self.session.client_with_index(self.local_client_id)
    }

    /// `Game.IsHost`.
    pub fn is_host(&self) -> bool {
        self.local_client().is_some_and(|c| c.is_admin)
    }

    pub fn local_is_ready(&self) -> bool {
        self.local_client().is_some_and(is_ready)
    }

    /// `IssueOrder(Order.Command(text))`.
    pub fn command(&mut self, text: &str) {
        self.outgoing.push(RawOrder::command(text));
    }

    /// `IssueOrder(Order.Chat(text, team))`.
    pub fn chat(&mut self, text: &str, team: u32) {
        let mut order = RawOrder::from_target_string("Chat", text, true);
        order.extra_data = team;
        self.outgoing.push(order);
    }
}

/// `Client.IsReady`.
pub fn is_ready(client: &Client) -> bool {
    client.state == rustora_net::session::ClientState::Ready
}

/// The shared handle that logic classes capture.
pub type Net = Rc<std::cell::RefCell<NetState>>;
