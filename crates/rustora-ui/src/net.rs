// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The lobby part of `OrderManager` and `TextNotificationsManager`, as logic classes see them. The client binary owns
//! the socket: it feeds this state and sends what `outgoing` holds.

use std::rc::Rc;

use indexmap::IndexMap;
use rustora_core::fieldloader::Color;
use rustora_net::order::RawOrder;
use rustora_net::session::{Client, Session};

/// The server that a connection goes to, the `ConnectionTarget` of C#.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinTarget {
    /// The single player server inside this client, of the last `AppRequest::StartSinglePlayer`.
    SinglePlayer,
    /// A server at `host:port`.
    Address(String),
}

impl Default for JoinTarget {
    fn default() -> Self {
        JoinTarget::Address(String::new())
    }
}

impl std::fmt::Display for JoinTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinTarget::SinglePlayer => f.write_str("the single player server"),
            JoinTarget::Address(address) => f.write_str(address),
        }
    }
}

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

/// Hour and minute of the local time, for `TextNotification.time`.
pub fn local_time() -> (u32, u32) {
    use chrono::Timelike;
    let now = chrono::Local::now();
    (now.hour(), now.minute())
}

/// `TextNotificationsManager.SystemClientId`.
pub const SYSTEM_CLIENT_ID: i32 = -1;

/// `TextNotification`.
#[derive(Clone, Debug)]
pub struct TextNotification {
    pub pool: TextNotificationPool,
    /// The sending client, or `SYSTEM_CLIENT_ID`.
    pub client_id: i32,
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

/// The fields of `MissionDataInfo` that the mission browser shows.
#[derive(Clone, Debug, Default)]
pub struct MissionInfo {
    pub briefing: Option<String>,
    pub briefing_video: Option<String>,
    pub background_video: Option<String>,
    pub start_video: Option<String>,
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
    /// `MissionDataInfo` of the world: the briefing and the briefing, background and start videos.
    pub mission: Option<MissionInfo>,
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
    pub target: JoinTarget,
    pub password: String,
    pub server_error: Option<String>,
    pub authentication_failed: bool,
    pub error_message: Option<String>,
    pub server_map_pool: Option<Vec<String>>,
    /// The software of the clients that announced it, from rustora's `SyncClientSoftware` extension. Others count
    /// as OpenRA.
    pub client_software: std::collections::HashMap<i32, String>,
    /// The last `LagReport` of a rustora server and the `Game.RunTime` it arrived at.
    pub lag_report: Option<(i64, rustora_net::extensions::LagReport)>,
    /// Whether this client announced its software on the current connection.
    pub software_announced: bool,
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
    /// The names this client banned as admin. rustora enforces them on servers without name bans.
    pub banned_names: std::collections::HashSet<String>,
    /// The client indexes that [`NetState::enforce_name_bans`] kicked already.
    pub name_ban_kicks: std::collections::HashSet<i32>,
    /// `Game.RunTime` of the last input of the user, for the idle timeout of the lobby.
    pub last_user_action: i64,
    /// Whether the user got the idle warning since the last input.
    pub idle_warned: bool,
    /// `Game.RunTime` when the client last told the server that the user is active.
    pub last_activity_report: Option<i64>,
}

/// What [`NetState::idle_step`] asks the client to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleStep {
    Nothing,
    /// Tell the user to react within [`IDLE_TIMEOUT`] - [`IDLE_WARNING`].
    Warn,
    /// Leave the server.
    Disconnect,
}

/// The time without input after which a client in a multiplayer lobby warns the user, in milliseconds.
pub const IDLE_WARNING: i64 = 29 * 60 * 1000;
/// The time without input after which a client leaves a multiplayer lobby, in milliseconds.
pub const IDLE_TIMEOUT: i64 = 30 * 60 * 1000;
/// The shortest time between two reports of user activity to the server, in milliseconds.
const ACTIVITY_REPORT_INTERVAL: i64 = 60 * 1000;

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

    /// Whether the server is a rustora server, which relays the software of the clients. It knows `ban`, `unban`
    /// and `banlist` and enforces name bans itself.
    pub fn server_is_rustora(&self) -> bool {
        !self.client_software.is_empty()
    }

    /// Remembers a banned client name for [`NetState::enforce_name_bans`].
    pub fn ban_name(&mut self, name: &str) {
        if !name.is_empty() {
            self.banned_names.insert(name.to_string());
        }
    }

    /// Forgets a banned client name. Returns whether it was banned.
    pub fn unban_name(&mut self, name: &str) -> bool {
        self.name_ban_kicks.clear();
        self.banned_names.remove(name)
    }

    /// Kicks each human client with a banned name while the local client is admin, once per client index, without
    /// banning its address. A rustora server rejects these clients itself; an OpenRA server ignores name bans.
    pub fn enforce_name_bans(&mut self) {
        if !self.is_host() {
            return;
        }
        let targets: Vec<i32> = self
            .session
            .clients
            .iter()
            .filter(|c| c.index != self.local_client_id && !c.is_bot() && !self.name_ban_kicks.contains(&c.index))
            .filter(|c| c.name.as_deref().is_some_and(|n| self.banned_names.contains(n)))
            .map(|c| c.index)
            .collect();
        for index in targets {
            self.name_ban_kicks.insert(index);
            self.command(&kick_command(index, false, true));
        }
    }

    /// Notes an input of the user at `now`.
    pub fn user_acted(&mut self, now: i64) {
        self.last_user_action = now;
        self.idle_warned = false;
    }

    /// Whether the client should tell the server now that the user is active: the user acted since the last report,
    /// and the last report is at least a minute old.
    pub fn take_activity_report(&mut self, now: i64) -> bool {
        let due = match self.last_activity_report {
            None => true,
            Some(t) => self.last_user_action > t && now - t >= ACTIVITY_REPORT_INTERVAL,
        };
        if due {
            self.last_activity_report = Some(now);
        }
        due
    }

    /// The idle timeout of the lobby at `now`: one warning after [`IDLE_WARNING`] without input, then the
    /// disconnect after [`IDLE_TIMEOUT`].
    pub fn idle_step(&mut self, now: i64) -> IdleStep {
        let idle = now - self.last_user_action;
        if idle >= IDLE_TIMEOUT {
            IdleStep::Disconnect
        } else if idle >= IDLE_WARNING && !self.idle_warned {
            self.idle_warned = true;
            IdleStep::Warn
        } else {
            IdleStep::Nothing
        }
    }

    /// `IssueOrder(Order.Chat(text, team))`.
    pub fn chat(&mut self, text: &str, team: u32) {
        let mut order = RawOrder::from_target_string("Chat", text, true);
        order.extra_data = team;
        self.outgoing.push(order);
    }
}

/// The `kick` command with the temporary address ban. rustora adds the name ban as third argument, which OpenRA
/// servers ignore.
pub fn kick_command(index: i32, temp_ban: bool, ban_name: bool) -> String {
    let flag = |b: bool| if b { "True" } else { "False" };
    if ban_name {
        format!("kick {index} {} True", flag(temp_ban))
    } else {
        format!("kick {index} {}", flag(temp_ban))
    }
}

/// `Client.IsReady`.
pub fn is_ready(client: &Client) -> bool {
    client.state == rustora_net::session::ClientState::Ready
}

/// The shared handle that logic classes capture.
pub type Net = Rc<std::cell::RefCell<NetState>>;

#[cfg(test)]
mod tests {
    use rustora_net::session::{Client, Session};

    use super::{IDLE_TIMEOUT, IDLE_WARNING, IdleStep, NetState, kick_command};

    fn client(index: i32, name: &str, is_admin: bool) -> Client {
        Client {
            index,
            name: Some(name.to_string()),
            is_admin,
            ..Client::default()
        }
    }

    #[test]
    fn kick_commands() {
        assert_eq!(kick_command(3, false, false), "kick 3 False");
        assert_eq!(kick_command(3, true, false), "kick 3 True");
        assert_eq!(kick_command(3, true, true), "kick 3 True True");
        assert_eq!(kick_command(3, false, true), "kick 3 False True");
    }

    #[test]
    fn the_admin_kicks_a_banned_name_once() {
        let mut net = NetState {
            local_client_id: 0,
            session: Session {
                clients: vec![client(0, "Admin", true), client(1, "Other", false), client(2, "Troll", false)],
                ..Session::default()
            },
            ..NetState::default()
        };
        net.ban_name("Troll");
        net.enforce_name_bans();
        net.enforce_name_bans();
        let sent: Vec<String> = net.outgoing.iter().map(|o| o.target_string.clone().unwrap_or_default()).collect();
        assert_eq!(sent, vec!["kick 2 False True".to_string()]);
    }

    #[test]
    fn a_client_without_admin_rights_kicks_nobody() {
        let mut net = NetState {
            local_client_id: 1,
            session: Session {
                clients: vec![client(0, "Admin", true), client(1, "Other", false), client(2, "Troll", false)],
                ..Session::default()
            },
            ..NetState::default()
        };
        net.ban_name("Troll");
        net.enforce_name_bans();
        assert!(net.outgoing.is_empty());
    }

    #[test]
    fn the_idle_timeout_warns_once_and_then_disconnects() {
        let mut net = NetState::default();
        net.user_acted(1000);
        assert_eq!(net.idle_step(1000 + IDLE_WARNING - 1), IdleStep::Nothing);
        assert_eq!(net.idle_step(1000 + IDLE_WARNING), IdleStep::Warn);
        assert_eq!(net.idle_step(1000 + IDLE_WARNING + 1), IdleStep::Nothing);
        net.user_acted(1000 + IDLE_WARNING + 2);
        assert_eq!(net.idle_step(1000 + IDLE_TIMEOUT), IdleStep::Nothing);
        assert_eq!(net.idle_step(1000 + IDLE_WARNING + 2 + IDLE_TIMEOUT), IdleStep::Disconnect);
    }

    #[test]
    fn activity_reports_come_at_most_once_a_minute() {
        let mut net = NetState::default();
        assert!(net.take_activity_report(0));
        net.user_acted(10_000);
        assert!(!net.take_activity_report(10_000));
        assert!(net.take_activity_report(60_000));
        assert!(!net.take_activity_report(200_000));
        net.user_acted(200_000);
        assert!(net.take_activity_report(200_000));
    }
}
