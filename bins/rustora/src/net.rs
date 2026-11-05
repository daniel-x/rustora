// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The socket side of the lobby: the local server thread, the connection and the orders of `UnitOrders.ProcessOrder`
//! that matter before a game starts.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use chrono::Timelike;
use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_engine::order_manager::{Connection, Incoming};
use rustora_game::moddata::ModData;
use rustora_launch::net_client::{JoinOptions, NetworkConnection, handshake_response};
use rustora_net::order::RawOrder;
use rustora_net::session::Session;
use rustora_server::mod_info::ServerMod;
use rustora_server::server::{Server, ServerSettings, ServerType};
use rustora_ui::fluent::{MessageArg, Messages};
use rustora_ui::logic::color_picker::ColorRules;
use rustora_ui::logic::{ColorRulesLookup, LobbyRulesLookup};
use rustora_ui::net::{
    ConnectionState, LOCAL_SERVER_TARGET, LobbyFaction, LobbyOption, LobbyRules, Net, NetState, TextNotification, TextNotificationPool,
};

/// How long a join of the local server waits for the server to listen.
const LOCAL_CONNECT_PATIENCE: Duration = Duration::from_secs(60);
/// `UnitOrders.ChatMessageMaxLength`.
const CHAT_MESSAGE_MAX_LENGTH: usize = 2500;
const JOINED: &str = "notification-joined";
const LEFT: &str = "notification-lobby-disconnected";

/// `Game.CreateLocalServer`. The thread ends when the host leaves. Returns the port.
fn create_local_server(engine: PathBuf, support: String, map: String, skirmish: bool) -> Option<u16> {
    let port = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?.local_addr().ok()?.port();
    std::thread::spawn(move || {
        let settings = ServerSettings {
            server_type: if skirmish { ServerType::Skirmish } else { ServerType::Local },
            name: "Skirmish Game".to_string(),
            listen_port: port,
            map: Some(map),
            advertise_online: false,
            support_dir: PathBuf::from(&support),
            ..Default::default()
        };
        let result = ServerMod::load(&engine, &support, "ra")
            .and_then(|game_mod| Server::new(settings, game_mod))
            .map(|mut server| server.run());
        if let Err(e) = result {
            log::error!("The local server failed: {e}");
        }
    });
    Some(port)
}

/// The mod as the server reads it, loaded on first use.
#[derive(Clone)]
pub struct LazyServerMod {
    engine: PathBuf,
    support: String,
    game_mod: Rc<RefCell<Option<ServerMod>>>,
}

impl LazyServerMod {
    pub fn new(engine: &Path, support: &str) -> LazyServerMod {
        LazyServerMod {
            engine: engine.to_path_buf(),
            support: support.to_string(),
            game_mod: Rc::default(),
        }
    }

    fn get(&self) -> Option<std::cell::RefMut<'_, Option<ServerMod>>> {
        let mut game_mod = self.game_mod.borrow_mut();
        if game_mod.is_none() {
            *game_mod = ServerMod::load(&self.engine, &self.support, "ra")
                .map_err(|e| log::error!("{e}"))
                .ok();
        }
        Some(game_mod)
    }
}

/// The color rules of the mod, read once.
pub fn color_rules_lookup(game_mod: LazyServerMod) -> ColorRulesLookup {
    let cached: RefCell<Option<Rc<ColorRules>>> = RefCell::default();
    Rc::new(move || {
        if cached.borrow().is_none() {
            let game_mod = game_mod.get()?;
            let (picker, terrain_colors) = game_mod.as_ref()?.default_color_rules().map_err(|e| log::error!("{e}")).ok()?;
            *cached.borrow_mut() = Some(Rc::new(ColorRules { picker, terrain_colors }));
        }
        cached.borrow().clone()
    })
}

/// The rules of maps for the lobby, read the way the server reads them.
pub fn lobby_rules_lookup(game_mod: LazyServerMod) -> LobbyRulesLookup {
    Rc::new(move |uid| {
        let mut game_mod = game_mod.get()?;
        let info = game_mod.as_mut()?.map_info(uid).map_err(|e| log::warn!("{e}")).ok()?;
        Some(Rc::new(LobbyRules {
            options: info
                .options
                .iter()
                .map(|o| LobbyOption {
                    id: o.id.clone(),
                    name: o.name.clone(),
                    description: o.description.clone(),
                    values: o.values.clone(),
                    default_value: o.default_value.clone(),
                    is_locked: o.is_locked,
                    is_visible: o.is_visible,
                    display_order: o.display_order,
                    is_boolean: o.is_boolean,
                })
                .collect(),
            bots: info.bots.iter().map(|b| (b.bot_type.clone(), b.name.clone())).collect(),
            factions: info
                .factions
                .iter()
                .map(|f| {
                    (
                        f.internal_name.clone(),
                        LobbyFaction {
                            selectable: f.selectable,
                            name: f.name.clone(),
                            description: f.description.clone(),
                            side: f.side.clone(),
                        },
                    )
                })
                .collect(),
        }))
    })
}

/// The connection and the local server of the client, in place of `Game.OrderManager` before a game starts.
pub struct NetClient {
    engine: PathBuf,
    support: String,
    pub net: Net,
    local_port: Option<u16>,
    connecting: Option<Receiver<Result<NetworkConnection, String>>>,
    connection: Option<NetworkConnection>,
}

impl NetClient {
    pub fn new(engine: &Path, support: &str, net: Net) -> NetClient {
        NetClient {
            engine: engine.to_path_buf(),
            support: support.to_string(),
            net,
            local_port: None,
            connecting: None,
            connection: None,
        }
    }

    /// `Game.CreateServer`.
    pub fn create_server(&mut self, name: String, port: u16, password: String, advertise_online: bool, map: String) {
        let (engine, support) = (self.engine.clone(), self.support.clone());
        std::thread::spawn(move || {
            let settings = ServerSettings {
                server_type: ServerType::Multiplayer,
                name,
                listen_port: port,
                password,
                map: Some(map),
                advertise_online,
                support_dir: PathBuf::from(&support),
                ..Default::default()
            };
            let result = ServerMod::load(&engine, &support, "ra")
                .and_then(|game_mod| Server::new(settings, game_mod))
                .map(|mut server| server.run());
            if let Err(e) = result {
                log::error!("The server failed: {e}");
            }
        });
    }

    pub fn create_local_server(&mut self, map: String, skirmish: bool) {
        self.local_port = create_local_server(self.engine.clone(), self.support.clone(), map, skirmish);
    }

    /// `Game.JoinServer`.
    pub fn join(&mut self, target: &str, password: &str) {
        self.connection = None;
        let local = target == LOCAL_SERVER_TARGET;
        let address = match (local, self.local_port) {
            (true, Some(port)) => format!("127.0.0.1:{port}"),
            _ => target.to_string(),
        };
        let mut net = self.net.borrow_mut();
        *net = NetState {
            target: address.clone(),
            password: password.to_string(),
            connection_state: ConnectionState::Connecting,
            notifications: std::mem::take(&mut net.notifications),
            revision: net.revision + 1,
            ..Default::default()
        };
        let (send, receive) = channel();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result = loop {
                let result = NetworkConnection::connect(&address);
                if result.is_ok() || !local || start.elapsed() > LOCAL_CONNECT_PATIENCE {
                    break result;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            let _ = send.send(result);
        });
        self.connecting = Some(receive);
    }

    /// `Game.Disconnect`. The local server ends when its host is gone.
    pub fn disconnect(&mut self) {
        self.connection = None;
        self.connecting = None;
        self.local_port = None;
        let mut net = self.net.borrow_mut();
        net.connection_state = ConnectionState::NotConnected;
        net.session = Session::default();
        net.outgoing.clear();
        net.notifications.clear();
    }

    /// Takes the connection and the final session when `StartGame` arrived.
    pub fn take_connection(&mut self) -> Option<NetworkConnection> {
        self.connection.take()
    }

    /// Receives and sends. Returns true if the game starts.
    pub fn pump(&mut self, mod_data: &ModData, messages: &Messages, run_time: i64, name: &str, color: Color) -> bool {
        if let Some(receive) = &self.connecting {
            match receive.try_recv() {
                Ok(Ok(connection)) => {
                    let mut net = self.net.borrow_mut();
                    net.local_client_id = connection.local_client_id();
                    net.connection_state = ConnectionState::Connected;
                    self.connection = Some(connection);
                    self.connecting = None;
                }
                Ok(Err(e)) => {
                    let mut net = self.net.borrow_mut();
                    net.connection_state = ConnectionState::NotConnected;
                    net.error_message = Some(e);
                    self.connecting = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.connecting = None,
            }
        }
        let Some(connection) = &mut self.connection else {
            return false;
        };
        let mut started = false;
        while let Some(incoming) = connection.next_incoming() {
            if let Incoming::Immediate { client_id, orders } = incoming {
                for order in &orders.orders {
                    let mut net = self.net.borrow_mut();
                    match process_order(mod_data, messages, connection, &mut net, client_id, order, run_time, name, color) {
                        Ok(true) => started = true,
                        Ok(false) => {}
                        Err(e) => net.error_message = Some(e),
                    }
                }
            }
            if started {
                return true;
            }
        }
        let outgoing = std::mem::take(&mut self.net.borrow_mut().outgoing);
        if !outgoing.is_empty() {
            connection.send_immediate(outgoing);
        }
        if !connection.is_connected() {
            self.net.borrow_mut().connection_state = ConnectionState::NotConnected;
            self.connection = None;
        }
        false
    }
}

fn notification(pool: TextNotificationPool, prefix: Option<String>, text: String, prefix_color: Option<Color>) -> TextNotification {
    let now = chrono::Local::now();
    TextNotification {
        pool,
        prefix,
        text,
        prefix_color,
        text_color: None,
        time: (now.hour(), now.minute()),
    }
}

/// Port of `FluentMessage` for each node of a `FluentMessage` order.
fn fluent_notifications(messages: &Messages, text: &str, order_name: &str) -> Vec<TextNotification> {
    let nodes = MiniYaml::from_str(text, order_name, true).unwrap_or_default();
    nodes
        .iter()
        .map(|node| {
            let field = |n: &MiniYamlNode, key: &str| n.value.nodes.iter().find(|f| f.key == key).map(|f| f.value.value_str().to_string());
            let key = field(node, "Key").unwrap_or_default();
            let arguments: Vec<(String, String, bool)> = node
                .value
                .nodes
                .iter()
                .find(|n| n.key == "Arguments")
                .map_or(&[][..], |n| n.value.nodes.as_slice())
                .iter()
                .map(|a| {
                    (
                        field(a, "Key").unwrap_or_default(),
                        field(a, "Value").unwrap_or_default(),
                        field(a, "Type").as_deref() == Some("Number"),
                    )
                })
                .collect();
            let args: Vec<(&str, MessageArg)> = arguments
                .iter()
                .map(|(k, v, number)| {
                    let value = match (number, v.parse()) {
                        (true, Ok(n)) => MessageArg::Number(n),
                        _ => MessageArg::Text(v),
                    };
                    (k.as_str(), value)
                })
                .collect();
            let pool = match key.as_str() {
                JOINED => TextNotificationPool::Join,
                LEFT => TextNotificationPool::Leave,
                _ => TextNotificationPool::System,
            };
            notification(pool, Some("Battlefield Control".to_string()), messages.get_with(&key, &args), None)
        })
        .collect()
}

/// The chat part of `UnitOrders.ProcessOrder` while no world exists.
fn chat_notification(net: &NetState, client_id: i32, order: &RawOrder) -> Option<TextNotification> {
    let client = net.session.client_with_index(client_id)?;
    let local = net.local_client();
    let message: String = order
        .target_string
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(CHAT_MESSAGE_MAX_LENGTH)
        .collect();
    let name = client.name.clone().unwrap_or_default();
    let prefix = if order.extra_data == 0 {
        let mut suffix = if client.is_observer() {
            " (Spectator)".to_string()
        } else {
            String::new()
        };
        if local.is_some_and(|l| l.index != client.index && client.team > 0 && client.team == l.team) {
            suffix += " (Ally)";
        }
        format!("{name}{suffix}")
    } else {
        if local.is_none_or(|l| l.team != client.team) {
            return None;
        }
        let tag = if order.extra_data == u32::MAX { "[Spectators] " } else { "[Team] " };
        format!("{tag}{name}")
    };
    Some(notification(TextNotificationPool::Chat, Some(prefix), message, Some(client.color)))
}

/// Applies an immediate order of the lobby. Returns true on `StartGame`.
#[allow(clippy::too_many_arguments)]
fn process_order(
    mod_data: &ModData,
    messages: &Messages,
    connection: &mut NetworkConnection,
    net: &mut NetState,
    client_id: i32,
    order: &RawOrder,
    run_time: i64,
    name: &str,
    color: Color,
) -> Result<bool, String> {
    if client_id == 0 {
        match order.order_string.as_str() {
            "HandshakeRequest" => {
                let options = JoinOptions {
                    name: name.to_string(),
                    color,
                    password: net.password.clone(),
                    ..Default::default()
                };
                let text = order.target_string.as_deref().unwrap_or("");
                connection.send_immediate(vec![handshake_response(mod_data, text, &options)?]);
                return Ok(false);
            }
            "StartGame" => return Ok(true),
            _ => {}
        }
    }
    process_client_order(messages, net, client_id, order, run_time)?;
    Ok(false)
}

/// The orders of `UnitOrders.ProcessOrder` that concern the user interface, in the lobby and in the game.
pub fn process_client_order(
    messages: &Messages,
    net: &mut NetState,
    client_id: i32,
    order: &RawOrder,
    run_time: i64,
) -> Result<(), String> {
    let text = order.target_string.as_deref().unwrap_or("");
    let parse = |t: &str| Session::deserialize(t, &order.order_string).map_err(|e| e.to_string());
    match order.order_string.as_str() {
        "Chat" => {
            if let Some(line) = chat_notification(net, client_id, order) {
                net.notifications.push(line);
            }
            return Ok(());
        }
        _ if client_id != 0 => return Ok(()),
        "ServerError" => net.server_error = Some(text.to_string()),
        "AuthenticationError" => {
            net.server_error = Some(text.to_string());
            net.authentication_failed = true;
        }
        "SyncInfo" => {
            net.session = parse(text)?;
            net.revision += 1;
        }
        "SyncLobbyClients" => {
            net.session.clients = parse(text)?.clients;
            net.revision += 1;
        }
        "SyncLobbySlots" => {
            net.session.slots = parse(text)?.slots;
            net.revision += 1;
        }
        "SyncLobbyGlobalSettings" => {
            net.session.global_settings = parse(text)?.global_settings;
            net.revision += 1;
        }
        "SyncClientPings" | "SyncConnectionQuality" => {}
        "Message" => net.notifications.push(notification(
            TextNotificationPool::System,
            Some("Battlefield Control".to_string()),
            text.to_string(),
            None,
        )),
        "FluentMessage" => net.notifications.extend(fluent_notifications(messages, text, &order.order_string)),
        "DisableChatEntry" => {
            net.chat_disabled_until = if order.extra_data == u32::MAX {
                i64::MAX
            } else {
                run_time + order.extra_data as i64
            };
        }
        _ => {}
    }
    Ok(())
}
