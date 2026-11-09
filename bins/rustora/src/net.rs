// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The socket side of the lobby: the local server process, the connection and the orders of `UnitOrders.ProcessOrder`
//! that matter before a game starts.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use chrono::Timelike;
use rustora_client::game_information::timestamped_filename;
use rustora_client::map_cache::MapCache;
use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_engine::order_manager::{Connection, Incoming};
use rustora_game::lobby_rules::{MapInfo, ModLobbyRules};
use rustora_game::map::MapPackage;
use rustora_game::moddata::ModData;
use rustora_launch::net_client::{JoinOptions, NetworkConnection, handshake_response};
use rustora_net::order::RawOrder;
use rustora_net::session::Session;
use rustora_ui::fluent::{MessageArg, Messages};
use rustora_ui::logic::color_picker::ColorRules;
use rustora_ui::logic::{ColorRulesLookup, LobbyRulesLookup};
use rustora_ui::net::{
    ConnectionState, LOCAL_SERVER_TARGET, LobbyFaction, LobbyOption, LobbyRules, Net, NetState, TextNotification, TextNotificationPool,
};

/// How long a join of the local server waits for the server to listen.
const LOCAL_CONNECT_PATIENCE: Duration = Duration::from_secs(60);
/// The start of the line in which `rustora-server` announces its port.
const LISTENING_ON_PORT: &str = "Listening on port ";
/// `UnitOrders.ChatMessageMaxLength`.
const CHAT_MESSAGE_MAX_LENGTH: usize = 2500;
const JOINED: &str = "notification-joined";
/// The server name of `Game.CreateLocalServer`.
const LOCAL_GAME_NAME: &str = "Skirmish Game";
const LEFT: &str = "notification-lobby-disconnected";

/// The port of a started server, set once the server announced that it accepts players.
type AnnouncedPort = Arc<OnceLock<u16>>;

/// Starts the `rustora-server` binary next to this executable as a dependent server of `kind` with `args`. It exits
/// after the game it serves, or when the client process is gone, because its standard input closes then. The port
/// is set when the server announces it.
fn spawn_server(engine: &Path, support: &str, kind: &str, args: &[&str]) -> Option<(Child, AnnouncedPort)> {
    let exe = std::env::current_exe()
        .ok()?
        .with_file_name(format!("rustora-server{}", std::env::consts::EXE_SUFFIX));
    let result = Command::new(&exe)
        .arg("--engine-dir")
        .arg(engine)
        .args(["--support-dir", support, "--dependent", kind])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let mut child = result
        .inspect_err(|e| log::error!("Starting the server {} failed: {e}", exe.display()))
        .ok()?;
    let port = AnnouncedPort::default();
    let stdout = child.stdout.take()?;
    let announced = port.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(p) = line.strip_prefix(LISTENING_ON_PORT).and_then(|p| p.trim().parse().ok()) {
                let _ = announced.set(p);
            }
        }
    });
    Some((child, port))
}

/// The lobby rules of the mod of the client and of the maps in its map cache, loaded on first use.
#[derive(Clone)]
pub struct LobbyData {
    mod_data: Rc<ModData>,
    maps: Rc<RefCell<MapCache>>,
    rules: Rc<RefCell<Option<Rc<ModLobbyRules>>>>,
    infos: Rc<RefCell<HashMap<String, Rc<MapInfo>>>>,
}

impl LobbyData {
    pub fn new(mod_data: Rc<ModData>, maps: Rc<RefCell<MapCache>>) -> LobbyData {
        LobbyData {
            mod_data,
            maps,
            rules: Rc::default(),
            infos: Rc::default(),
        }
    }

    fn rules(&self) -> Option<Rc<ModLobbyRules>> {
        let mut rules = self.rules.borrow_mut();
        if rules.is_none() {
            *rules = ModLobbyRules::load(&self.mod_data)
                .map(Rc::new)
                .map_err(|e| log::error!("{e}"))
                .ok();
        }
        rules.clone()
    }

    fn map_info(&self, uid: &str) -> Option<Rc<MapInfo>> {
        if let Some(info) = self.infos.borrow().get(uid) {
            return Some(info.clone());
        }
        let rules = self.rules()?;
        let preview = self.maps.borrow().get(uid)?;
        let package = preview.open_package(&self.mod_data.file_system)?;
        let map = MapPackage::load(package.clone()).map_err(|e| log::warn!("{e}")).ok()?;
        let info = Rc::new(rules.map_info(&self.mod_data, &package, &map).map_err(|e| log::warn!("{e}")).ok()?);
        self.infos.borrow_mut().insert(uid.to_string(), info.clone());
        Some(info)
    }
}

/// The color rules of the mod, read once.
pub fn color_rules_lookup(lobby: LobbyData) -> ColorRulesLookup {
    let cached: RefCell<Option<Rc<ColorRules>>> = RefCell::default();
    Rc::new(move || {
        if cached.borrow().is_none() {
            let rules = lobby.rules()?;
            let (picker, terrain_colors) = rules.default_color_rules(&lobby.mod_data).map_err(|e| log::error!("{e}")).ok()?;
            *cached.borrow_mut() = Some(Rc::new(ColorRules { picker, terrain_colors }));
        }
        cached.borrow().clone()
    })
}

/// The rules of maps for the lobby, read the way the server reads them.
pub fn lobby_rules_lookup(lobby: LobbyData) -> LobbyRulesLookup {
    Rc::new(move |uid| {
        let info = lobby.map_info(uid)?;
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
            mission: info.mission.as_ref().map(|m| rustora_ui::net::MissionInfo {
                briefing: m.briefing.clone(),
                briefing_video: m.briefing_video.clone(),
                background_video: m.background_video.clone(),
                start_video: m.start_video.clone(),
            }),
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
    /// The announced port of the server this client started, which may still be loading when the client joins it.
    started_port: Option<AnnouncedPort>,
    /// The port of the server of `create_server`.
    created_port: Option<u16>,
    /// The server process this client started.
    server: Option<Child>,
    /// The address that the connection attempt resolved its target to, and its outcome.
    connecting: Option<Receiver<(String, Result<NetworkConnection, String>)>>,
    connection: Option<NetworkConnection>,
    /// The orders of `Game.CreateAndStartLocalServer`, sent once the first lobby information arrived.
    setup_orders: Vec<RawOrder>,
    /// The `SaveLastOrdersFrame` and `SaveSyncFrame` of the `StartGame` order of a game save.
    game_save_frames: Option<(i32, i32)>,
}

impl NetClient {
    pub fn new(engine: &Path, support: &str, net: Net) -> NetClient {
        NetClient {
            engine: engine.to_path_buf(),
            support: support.to_string(),
            net,
            started_port: None,
            created_port: None,
            server: None,
            connecting: None,
            connection: None,
            setup_orders: Vec::new(),
            game_save_frames: None,
        }
    }

    /// `Game.CreateServer`, as a separate server process.
    pub fn create_server(&mut self, name: String, port: u16, password: String, advertise_online: bool, map: String) {
        self.created_port = Some(port);
        let port = port.to_string();
        let mut args = vec!["--name", &name, "--port", &port, "--password", &password, "--map", &map];
        if advertise_online {
            args.push("--advertise-online");
        }
        self.replace_server(spawn_server(&self.engine, &self.support, "multiplayer", &args));
    }

    /// `Game.CreateLocalServer`, as a separate server process on a loopback port that the system chooses.
    pub fn create_local_server(&mut self, map: String, skirmish: bool) {
        let kind = if skirmish { "skirmish" } else { "local" };
        let args = ["--name", LOCAL_GAME_NAME, "--port", "0", "--map", &map];
        self.replace_server(spawn_server(&self.engine, &self.support, kind, &args));
    }

    /// Stops the server this client started last, if it still runs, and keeps `server` in its place.
    fn replace_server(&mut self, server: Option<(Child, AnnouncedPort)>) {
        let (server, port) = server.unzip();
        self.started_port = port;
        if let Some(mut old) = std::mem::replace(&mut self.server, server) {
            let _ = old.kill();
            let _ = old.wait();
        }
    }

    /// Sets the orders that the next connection sends when its lobby information is ready.
    pub fn set_setup_orders(&mut self, orders: Vec<RawOrder>) {
        self.setup_orders = orders;
    }

    /// The game save frames of the last `StartGame` order, if it started a game save.
    pub fn take_game_save_frames(&mut self) -> Option<(i32, i32)> {
        self.game_save_frames.take()
    }

    /// `Game.JoinServer`.
    pub fn join(&mut self, target: &str, password: &str) {
        self.connection = None;
        let created = self
            .created_port
            .is_some_and(|port| target.rsplit_once(':').is_some_and(|(_, p)| p == port.to_string()));
        let local = target == LOCAL_SERVER_TARGET || created;
        let started_port = self.started_port.clone().filter(|_| local);
        let target_address = target.to_string();
        let mut net = self.net.borrow_mut();
        *net = NetState {
            target: target_address.clone(),
            password: password.to_string(),
            connection_state: ConnectionState::Connecting,
            notifications: std::mem::take(&mut net.notifications),
            revision: net.revision + 1,
            ..Default::default()
        };
        let (send, receive) = channel();
        std::thread::spawn(move || {
            let start = Instant::now();
            let address = match started_port {
                Some(port) => loop {
                    if let Some(p) = port.get() {
                        break if target_address == LOCAL_SERVER_TARGET {
                            format!("127.0.0.1:{p}")
                        } else {
                            target_address
                        };
                    }
                    if start.elapsed() > LOCAL_CONNECT_PATIENCE {
                        let _ = send.send((target_address, Err("the server did not start".to_string())));
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                },
                None => target_address,
            };
            let result = loop {
                let result = NetworkConnection::connect(&address);
                if result.is_ok() || !local || start.elapsed() > LOCAL_CONNECT_PATIENCE {
                    break result;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            let _ = send.send((address, result));
        });
        self.connecting = Some(receive);
    }

    /// `Game.Disconnect`. The local server ends when its host is gone.
    pub fn disconnect(&mut self) {
        self.connection = None;
        self.connecting = None;
        self.started_port = None;
        self.created_port = None;
        self.setup_orders.clear();
        let mut net = self.net.borrow_mut();
        net.connection_state = ConnectionState::NotConnected;
        net.session = Session::default();
        net.outgoing.clear();
        net.notifications.clear();
        net.muted_players.clear();
        net.kick_vote_target = None;
    }

    /// Takes the connection and the final session when `StartGame` arrived.
    pub fn take_connection(&mut self) -> Option<NetworkConnection> {
        self.connection.take()
    }

    /// Receives and sends. Returns true if the game starts.
    pub fn pump(&mut self, mod_data: &ModData, messages: &Messages, run_time: i64, name: &str, color: Color) -> bool {
        if let Some(receive) = &self.connecting {
            match receive.try_recv() {
                Ok((address, Ok(mut connection))) => {
                    let manifest = &mod_data.manifest;
                    let directory =
                        rustora_core::own_files::replays_dir(Path::new(&self.support), &manifest.id, &manifest.metadata.version);
                    connection.start_recording(directory, timestamped_filename(&manifest.id, ""));
                    let mut net = self.net.borrow_mut();
                    net.target = address;
                    net.local_client_id = connection.local_client_id();
                    net.connection_state = ConnectionState::Connected;
                    self.connection = Some(connection);
                    self.connecting = None;
                }
                Ok((address, Err(e))) => {
                    let mut net = self.net.borrow_mut();
                    net.target = address;
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
                        Ok(true) => {
                            started = true;
                            self.game_save_frames = rustora_launch::net_client::parse_game_save_frames(order);
                        }
                        Ok(false) => {}
                        Err(e) => net.error_message = Some(e),
                    }
                }
            }
            if started {
                return true;
            }
        }
        if !self.setup_orders.is_empty() {
            let mut net = self.net.borrow_mut();
            if net.local_client().is_some() {
                net.outgoing.extend(std::mem::take(&mut self.setup_orders));
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
        client_id: rustora_ui::net::SYSTEM_CLIENT_ID,
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
    let mut line = notification(TextNotificationPool::Chat, Some(prefix), message, Some(client.color));
    line.client_id = client_id;
    Some(line)
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
            if !net.muted_players.contains(&client_id)
                && let Some(line) = chat_notification(net, client_id, order)
            {
                net.notifications.push(line);
            }
            return Ok(());
        }
        _ if client_id != 0 => return Ok(()),
        "StartKickVote" => {
            net.kick_vote_target = Some(order.extra_data as i32);
            return Ok(());
        }
        "EndKickVote" => {
            if net.kick_vote_target == Some(order.extra_data as i32) {
                net.kick_vote_target = None;
            }
            return Ok(());
        }
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

impl Drop for NetClient {
    fn drop(&mut self) {
        self.replace_server(None);
    }
}
