// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The game server (`OpenRA.Game/Server/Server.cs`) of type `ServerType.Dedicated`, with the `PlayerPinger` trait.
//! Not ported yet: player authentication, replay recording, NAT port forwarding and master server advertising.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use indexmap::IndexSet;
use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode, nodes_to_string};
use rustora_core::world_types::MersenneTwister;
use rustora_mods_common::world::{make_slot, sync_client_to_player_reference};
use rustora_net::binary::Reader;
use rustora_net::handshake::{HANDSHAKE_PROTOCOL, HandshakeRequest, HandshakeResponse, ORDERS_PROTOCOL};
use rustora_net::order::{RawOrder, SYNC_HASH_ORDER_LENGTH, order_type};
use rustora_net::session::{Client, ClientState, ConnectionQuality, Global, Session};

use crate::connection::{Connection, Event, run_time};
use crate::mod_info::{MAP_STATUS_UNSAFE_CUSTOM_RULES, MapInfo, ServerMod};
use crate::order_buffer::OrderBuffer;
use crate::vote_kick::VoteKickTracker;

const PING_INTERVAL: u64 = 5000;
const CONN_REPORT_INTERVAL: u64 = 20000;
const CONN_TIMEOUT: u64 = 60000;
const DEFAULT_PLAYER_NAME: &str = "Commander";
const DEFAULT_MOTD: &str = "Welcome, have fun and good luck!";
pub const WHITE: Color = Color::from_argb(255, 255, 255, 255);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerState {
    WaitingPlayers,
    GameStarted,
    ShuttingDown,
}

/// The `ServerSettings` the dedicated server uses.
#[derive(Clone, Debug)]
pub struct ServerSettings {
    pub name: String,
    pub listen_port: u16,
    pub map: Option<String>,
    pub password: String,
    pub enable_singleplayer: bool,
    pub enable_sync_reports: bool,
    pub share_anonymized_ips: bool,
    pub ban: Vec<String>,
    pub map_pool: Vec<String>,
    pub seed: Option<i32>,
    pub support_dir: PathBuf,
    pub flood_limit_join_cooldown: u64,
    pub flood_limit_interval: u64,
    pub flood_limit_message_count: usize,
    pub flood_limit_cooldown: u64,
    pub enable_vote_kick: bool,
    pub vote_kick_timer: u64,
    pub vote_kicker_cooldown: u64,
}

impl Default for ServerSettings {
    fn default() -> Self {
        ServerSettings {
            name: String::new(),
            listen_port: 1234,
            map: None,
            password: String::new(),
            enable_singleplayer: false,
            enable_sync_reports: false,
            share_anonymized_ips: true,
            ban: Vec::new(),
            map_pool: Vec::new(),
            seed: None,
            support_dir: PathBuf::new(),
            flood_limit_join_cooldown: 5000,
            flood_limit_interval: 5000,
            flood_limit_message_count: 5,
            flood_limit_cooldown: 15000,
            enable_vote_kick: true,
            vote_kick_timer: 30000,
            vote_kicker_cooldown: 120000,
        }
    }
}

/// An argument of a fluent message, the `FluentArgument` types.
#[derive(Clone, Debug)]
pub enum FluentArg {
    Text(String),
    Number(i64),
}

impl From<&str> for FluentArg {
    fn from(v: &str) -> Self {
        FluentArg::Text(v.to_string())
    }
}

impl From<&String> for FluentArg {
    fn from(v: &String) -> Self {
        FluentArg::Text(v.clone())
    }
}

impl From<i64> for FluentArg {
    fn from(v: i64) -> Self {
        FluentArg::Number(v)
    }
}

/// `FluentMessage.Serialize`.
pub fn serialize_fluent_message(key: &str, args: &[(&str, FluentArg)]) -> String {
    let mut root = vec![
        MiniYamlNode::with_value_str("Protocol", "1"),
        MiniYamlNode::with_value_str("Key", key),
    ];
    if !args.is_empty() {
        let nodes = args
            .iter()
            .enumerate()
            .map(|(i, (k, v))| {
                let (value, ty) = match v {
                    FluentArg::Text(t) => (t.clone(), "String"),
                    FluentArg::Number(n) => (n.to_string(), "Number"),
                };
                let fields = vec![
                    MiniYamlNode::with_value_str("Key", *k),
                    MiniYamlNode::with_value_str("Value", value),
                    MiniYamlNode::with_value_str("Type", ty),
                ];
                MiniYamlNode::new(format!("Argument@{i}"), MiniYaml::new(None, fields))
            })
            .collect();
        root.push(MiniYamlNode::new("Arguments", MiniYaml::new(None, nodes)));
    }
    let mut lines = Vec::new();
    MiniYaml::new(None, root).to_lines("FluentMessage", None, &mut lines);
    lines.join("\n")
}

fn sanitized_name(dirty: &str) -> String {
    dirty.chars().filter(|c| !"#@:\n\t[]{}<>\"`".contains(*c)).collect()
}

/// `Settings.SanitizedPlayerName`.
pub fn sanitized_player_name(dirty: &str) -> String {
    let clean = sanitized_name(dirty);
    let clean = if clean.trim().is_empty() || clean == "Open" || clean == "Closed" {
        DEFAULT_PLAYER_NAME.to_string()
    } else {
        clean
    };
    clean.chars().take(16).collect()
}

/// `Session.AnonymizeIP`: IPv4 addresses without the last octet, nothing for IPv6.
fn anonymize_ip(ip: IpAddr) -> Option<String> {
    let v4 = match ip {
        IpAddr::V4(v) => Some(v),
        IpAddr::V6(v) => v.to_ipv4_mapped(),
    }?;
    let o = v4.octets();
    Some(format!("{}.{}.{}.*", o[0], o[1], o[2]))
}

fn create_frame(client: i32, frame: i32, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(data.len() + 12);
    v.extend_from_slice(&(data.len() as i32 + 4).to_le_bytes());
    v.extend_from_slice(&client.to_le_bytes());
    v.extend_from_slice(&frame.to_le_bytes());
    v.extend_from_slice(data);
    v
}

fn create_ack_frame(frame: i32, count: u8) -> Vec<u8> {
    create_frame(0, frame, &[order_type::ACK, count])
}

fn create_tick_scale_frame(scale: f32) -> Vec<u8> {
    let mut data = vec![order_type::TICK_SCALE];
    data.extend_from_slice(&scale.to_le_bytes());
    create_frame(0, 0, &data)
}

fn random_guid(random: &mut MersenneTwister) -> String {
    let b: Vec<u8> = (0..16).map(|_| random.next() as u8).collect();
    let hex = |r: std::ops::Range<usize>| b[r].iter().map(|x| format!("{x:02x}")).collect::<String>();
    format!("{}-{}-{}-{}-{}", hex(0..4), hex(4..6), hex(6..8), hex(8..10), hex(10..16))
}

fn open_listener(address: SocketAddr) -> std::io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
    if address.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    socket.listen(128)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

fn listen_loop(listener: TcpListener, events: Sender<Event>, accepting: Arc<AtomicBool>) {
    while accepting.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = events.send(Event::Connect(stream));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// The outcome of a player, for `GameInformation.Player` of the win and loss tracking.
struct WorldPlayer {
    client_index: i32,
    team: i32,
    undecided: bool,
}

/// `AnyUndefinedWinStates`: true while more than one side is undecided.
fn any_undecided_opponents(players: &[Option<WorldPlayer>]) -> bool {
    let mut last_team = -1;
    for p in players.iter().flatten().filter(|p| p.undecided) {
        if last_team >= 0 && (p.team != last_team || p.team == 0) {
            return true;
        }
        last_team = p.team;
    }
    false
}

pub struct Server {
    pub settings: ServerSettings,
    pub game_mod: ServerMod,
    pub map: Rc<MapInfo>,
    pub lobby: Session,
    pub conns: Vec<Connection>,
    pub state: ServerState,
    pub random: MersenneTwister,
    pub order_latency: i32,
    pub temp_bans: Vec<String>,
    pub map_pool: IndexSet<String>,
    pub vote_kick: VoteKickTracker,
    world_players: Vec<Option<WorldPlayer>>,
    sync_for_frame: HashMap<i32, Vec<u8>>,
    last_defeat_state_frame: i32,
    last_defeat_state: u64,
    message_tracker: HashMap<i32, Vec<u64>>,
    order_buffer: Option<OrderBuffer>,
    start: Instant,
    next_player_index: i32,
    events_tx: Sender<Event>,
    events_rx: Receiver<Event>,
    accepting: Arc<AtomicBool>,
    listener_threads: Vec<JoinHandle<()>>,
    ping_updated: Instant,
    last_ping: u64,
    is_initial_ping: bool,
    last_conn_report: u64,
}

impl Server {
    /// Opens the listeners and selects the initial map, like the constructor and `LobbyCommands.ServerStarted`.
    pub fn new(mut settings: ServerSettings, mut game_mod: ServerMod) -> Result<Server, String> {
        let (events_tx, events_rx) = channel();
        let accepting = Arc::new(AtomicBool::new(true));
        let mut listener_threads = Vec::new();
        let mut last_error = None;
        for ip in [IpAddr::V6(Ipv6Addr::UNSPECIFIED), IpAddr::V4(Ipv4Addr::UNSPECIFIED)] {
            let address = SocketAddr::new(ip, settings.listen_port);
            match open_listener(address) {
                Ok(l) => {
                    let (tx, flag) = (events_tx.clone(), accepting.clone());
                    listener_threads.push(std::thread::spawn(move || listen_loop(l, tx, flag)));
                }
                Err(e) => {
                    println!("Failed to listen on {address}: {e}");
                    last_error = Some(e);
                }
            }
        }
        if listener_threads.is_empty() {
            return Err(last_error.map_or_else(|| "no listeners".to_string(), |e| e.to_string()));
        }

        let clock = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let random_seed = settings.seed.unwrap_or(clock as i32);
        let mut random = MersenneTwister::new(settings.seed.unwrap_or((clock >> 20) as i32));

        let clean_name = sanitized_name(&settings.name);
        settings.name = if clean_name.trim().is_empty() {
            format!("{DEFAULT_PLAYER_NAME}'s Game")
        } else {
            clean_name
        };

        let map_pool: IndexSet<String> = if settings.map_pool.is_empty() {
            game_mod.lobby_maps().into_iter().collect()
        } else {
            settings.map_pool.iter().filter(|m| game_mod.map_exists(m)).cloned().collect()
        };
        let known = settings
            .map
            .as_deref()
            .and_then(|m| game_mod.resolve_map(m))
            .filter(|uid| map_pool.contains(uid));
        let uid = known
            .or_else(|| {
                let n = map_pool.len() as i32;
                map_pool.get_index(random.next_max(n) as usize).cloned()
            })
            .ok_or("Unable to resolve a valid initial map")?;
        settings.map = Some(uid.clone());
        let map = game_mod.map_info(&uid)?;

        let mut lobby = Session {
            global_settings: Global {
                random_seed,
                server_name: Some(settings.name.clone()),
                enable_singleplayer: settings.enable_singleplayer,
                enable_sync_reports: settings.enable_sync_reports,
                game_uid: Some(random_guid(&mut random)),
                dedicated: true,
                map: Some(uid),
                map_status: map.status,
                ..Default::default()
            },
            slots: map
                .players
                .values()
                .filter_map(make_slot)
                .map(|s| (s.player_reference.clone(), s))
                .collect(),
            ..Default::default()
        };
        map.load_map_settings(&mut lobby);
        println!("Initial mod: {}", game_mod.data.manifest.id);
        println!("Initial map: {}", map.uid);

        Ok(Server {
            settings,
            game_mod,
            map,
            lobby,
            conns: Vec::new(),
            state: ServerState::WaitingPlayers,
            random,
            order_latency: 1,
            temp_bans: Vec::new(),
            map_pool,
            vote_kick: VoteKickTracker::default(),
            world_players: Vec::new(),
            sync_for_frame: HashMap::new(),
            last_defeat_state_frame: 0,
            last_defeat_state: 0,
            message_tracker: HashMap::new(),
            order_buffer: None,
            start: Instant::now(),
            next_player_index: 0,
            events_tx,
            events_rx,
            accepting,
            listener_threads,
            ping_updated: Instant::now(),
            last_ping: 0,
            is_initial_ping: true,
            last_conn_report: 0,
        })
    }

    /// Processes events until the game has started and all clients have left.
    pub fn run(&mut self) {
        while self.state != ServerState::ShuttingDown {
            if let Ok(e) = self.events_rx.recv_timeout(Duration::from_secs(1)) {
                self.handle_event(e);
            }
            self.pinger_tick();
            self.vote_kick_tick();
            if self.state == ServerState::GameStarted {
                self.send_tick_scales();
                if self.conns.is_empty() {
                    println!("No one is playing, shutting down...");
                    self.state = ServerState::ShuttingDown;
                }
            }
        }
        self.accepting.store(false, Ordering::Relaxed);
        for t in self.listener_threads.drain(..) {
            let _ = t.join();
        }
        for c in &mut self.conns {
            c.dispose();
        }
        self.conns.clear();
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Connect(socket) => self.accept_connection(socket),
            Event::Packet { player_index, frame, data } => self.receive_orders(player_index, frame, &data),
            Event::Ping { player_index, history, .. } => self.receive_ping(player_index, &history),
            Event::Disconnect { player_index } => self.drop_client(player_index),
        }
    }

    fn send_tick_scales(&mut self) {
        let scales = self.order_buffer.as_mut().map_or_else(Vec::new, OrderBuffer::get_tick_scales);
        for (player_index, scale) in scales {
            if self.conn(player_index).is_some_and(|c| c.validated) {
                self.dispatch_frame_to_client(player_index, create_tick_scale_frame(scale));
            }
        }
    }

    pub fn conn(&self, player_index: i32) -> Option<&Connection> {
        self.conns.iter().find(|c| c.player_index == player_index)
    }

    fn conn_mut(&mut self, player_index: i32) -> Option<&mut Connection> {
        self.conns.iter_mut().find(|c| c.player_index == player_index)
    }

    pub fn is_validated(&self, player_index: i32) -> bool {
        self.conn(player_index).is_some_and(|c| c.validated)
    }

    pub fn choose_free_player_index(&mut self) -> i32 {
        let i = self.next_player_index;
        self.next_player_index += 1;
        i
    }

    fn accept_connection(&mut self, socket: std::net::TcpStream) {
        if self.state != ServerState::WaitingPlayers {
            return;
        }
        let bytes: Vec<u8> = (0..256).map(|_| self.random.next() as u8).collect();
        let token = base64::engine::general_purpose::STANDARD.encode(bytes);
        let index = self.choose_free_player_index();
        let conn = match Connection::new(index, token.clone(), socket, self.events_tx.clone(), self.start, true) {
            Ok(c) => c,
            Err(e) => {
                println!("Handshake failed: {e}");
                return;
            }
        };
        let mut hello = HANDSHAKE_PROTOCOL.to_le_bytes().to_vec();
        hello.extend_from_slice(&index.to_le_bytes());
        conn.try_send_data(hello);
        let request = HandshakeRequest {
            mod_id: Some(self.game_mod.data.manifest.id.clone()),
            version: Some(self.game_mod.data.manifest.metadata.version.clone()),
            auth_token: Some(token),
        };
        let order = RawOrder::handshake("HandshakeRequest", &request.serialize());
        conn.try_send_data(create_frame(0, 0, &order.serialize()));
        self.conns.push(conn);
    }

    fn reject(&mut self, player_index: i32, order: &str, message: &str, reason: &str) {
        if let Some(c) = self.conn(player_index) {
            println!("Rejected connection from {}; {reason}.", c.end_point);
        }
        self.send_order_to(player_index, order, message);
        self.drop_client(player_index);
    }

    /// `ValidateClient`. Fingerprints are ignored as no player database is queried yet.
    fn validate_client(&mut self, player_index: i32, data: &str, name: &str) {
        if self.state == ServerState::GameStarted {
            return self.reject(
                player_index,
                "ServerError",
                "notification-error-game-started",
                "game is already started",
            );
        }
        let handshake = match HandshakeResponse::deserialize(data, name) {
            Ok(h) => h,
            Err(e) => {
                println!("Dropping connection because an error occurred: {e}");
                return self.drop_client(player_index);
            }
        };
        let password = handshake.password.as_deref().unwrap_or("");
        if !self.settings.password.is_empty() && password != self.settings.password {
            let message = if password.is_empty() {
                "notification-requires-password"
            } else {
                "notification-incorrect-password"
            };
            return self.reject(player_index, "AuthenticationError", message, "wrong password");
        }
        let Some(ip) = self.conn(player_index).map(|c| c.end_point.ip().to_canonical()) else {
            return;
        };
        let manifest = &self.game_mod.data.manifest;
        if handshake.mod_id.as_deref() != Some(manifest.id.as_str()) {
            return self.reject(player_index, "ServerError", "notification-incompatible-mod", "mods do not match");
        }
        if handshake.version.as_deref() != Some(manifest.metadata.version.as_str()) {
            return self.reject(
                player_index,
                "ServerError",
                "notification-incompatible-version",
                "not running the same version",
            );
        }
        if handshake.orders_protocol != ORDERS_PROTOCOL {
            return self.reject(
                player_index,
                "ServerError",
                "notification-incompatible-protocol",
                "incompatible orders protocol",
            );
        }
        let ip_text = ip.to_string();
        if self.settings.ban.contains(&ip_text) {
            return self.reject(player_index, "ServerError", "notification-you-were-banned", "banned");
        }
        if self.temp_bans.contains(&ip_text) {
            return self.reject(player_index, "ServerError", "notification-you-were-temp-banned", "banned");
        }
        let client = Client {
            name: Some(sanitized_player_name(handshake.client.name.as_deref().unwrap_or(""))),
            anonymized_ip_address: if self.settings.share_anonymized_ips {
                anonymize_ip(ip)
            } else {
                None
            },
            index: player_index,
            preferred_color: handshake.client.preferred_color,
            color: handshake.client.color,
            faction: Some("Random".to_string()),
            state: ClientState::Invalid,
            ..Default::default()
        };
        self.complete_connection(player_index, client);
    }

    fn complete_connection(&mut self, player_index: i32, mut client: Client) {
        client.slot = self.lobby.first_empty_slot();
        client.is_admin = !self.lobby.clients.iter().any(|c| c.is_admin);
        if client.is_observer() && !self.lobby.global_settings.allow_spectators {
            self.send_order_to(player_index, "ServerError", "notification-game-full");
            return self.drop_client(player_index);
        }
        match client.slot.as_ref().and_then(|s| self.map.players.get(s)) {
            Some(pr) => sync_client_to_player_reference(&mut client, pr),
            None => client.color = WHITE,
        }
        let name = client.name.clone().unwrap_or_default();
        let is_admin = client.is_admin;
        self.lobby.clients.push(client);
        let map = self.map.clone();
        if let Some(c) = self.conn_mut(player_index) {
            c.validated = true;
            println!("Client {player_index}: Accepted connection from {}.", c.end_point);
        }
        let join_cooldown = self.settings.flood_limit_join_cooldown;
        if !is_admin && join_cooldown > 0 {
            self.disable_chat_ui(player_index, join_cooldown);
        }
        self.client_joined(player_index);
        self.sync_lobby_info();
        println!("{name} has joined the game.");
        self.send_fluent_message("notification-joined", &[("player", (&name).into())]);

        let motd_file = self.settings.support_dir.join("motd.txt");
        if !motd_file.exists() {
            let _ = std::fs::create_dir_all(&self.settings.support_dir);
            let _ = std::fs::write(&motd_file, DEFAULT_MOTD);
        }
        let motd = std::fs::read_to_string(&motd_file).unwrap_or_else(|_| DEFAULT_MOTD.to_string());
        if !motd.is_empty() {
            self.send_order_to(player_index, "Message", &motd);
        }
        if self.lobby.global_settings.map_status & MAP_STATUS_UNSAFE_CUSTOM_RULES != 0 {
            self.send_fluent_message_to(player_index, "notification-custom-rules", &[]);
        }
        if !self.lobby.global_settings.enable_singleplayer {
            self.send_fluent_message_to(player_index, "notification-two-humans-required", &[]);
        } else if map.players.values().filter(|p| p.playable).all(|p| !p.allow_bots) {
            self.send_fluent_message_to(player_index, "notification-map-bots-disabled", &[]);
        }
    }

    /// `DispatchFrameToClient`: drops the client if sending fails.
    fn dispatch_frame_to_client(&mut self, player_index: i32, frame: Vec<u8>) {
        let sent = self.conn(player_index).is_some_and(|c| c.try_send_data(frame));
        if !sent {
            self.drop_client(player_index);
            println!("Dropping client {player_index} because dispatching orders failed!");
        }
    }

    fn validated_conn_indices(&self) -> Vec<i32> {
        self.conns.iter().filter(|c| c.validated).map(|c| c.player_index).collect()
    }

    /// `DispatchOrdersToClients`: forwards a client's orders to the other validated clients.
    pub fn dispatch_orders_to_clients(&mut self, from: i32, frame: i32, data: &[u8]) {
        let frame_data = create_frame(from, frame, data);
        for c in self.validated_conn_indices() {
            if c != from {
                self.dispatch_frame_to_client(c, frame_data.clone());
            }
        }
        self.record_order(frame, data, from);
    }

    /// `DispatchServerOrdersToClients`.
    pub fn dispatch_server_orders_to_clients(&mut self, frame: i32, data: &[u8]) {
        let frame_data = create_frame(0, frame, data);
        for c in self.validated_conn_indices() {
            self.dispatch_frame_to_client(c, frame_data.clone());
        }
        self.record_order(frame, data, 0);
    }

    /// `RecordOrder` without a replay recorder: tracks defeats through the sync packets.
    fn record_order(&mut self, frame: i32, data: &[u8], from: i32) {
        if data.first() != Some(&order_type::SYNC_HASH) {
            return;
        }
        if data.len() == SYNC_HASH_ORDER_LENGTH {
            self.handle_sync_order(frame, data);
        } else {
            println!(
                "Dropped sync order with length {} from client {from}. Expected length {SYNC_HASH_ORDER_LENGTH}.",
                data.len()
            );
        }
    }

    /// `HandleSyncOrder`. The first client's sync packet of a frame decides; others only detect a desync.
    fn handle_sync_order(&mut self, frame: i32, packet: &[u8]) {
        if let Some(existing) = self.sync_for_frame.get(&frame) {
            if existing != packet {
                println!("Out of sync detected at frame {frame}");
            }
            return;
        }
        let defeat_state = u64::from_le_bytes(packet[5..13].try_into().expect("sync order length"));
        if frame > self.last_defeat_state_frame && self.last_defeat_state != defeat_state {
            let new_defeats = defeat_state & !self.last_defeat_state;
            for i in 0..self.world_players.len().min(64) {
                if new_defeats & (1u64 << i) != 0 {
                    self.set_player_defeat(i);
                }
            }
            self.last_defeat_state = defeat_state;
            self.last_defeat_state_frame = frame;
        }
        self.sync_for_frame.insert(frame, packet.to_vec());
    }

    /// `SetPlayerDefeat`: when only one side remains, its players have won.
    fn set_player_defeat(&mut self, player_index: usize) {
        let Some(Some(player)) = self.world_players.get_mut(player_index) else {
            return;
        };
        if !player.undecided {
            return;
        }
        player.undecided = false;
        if !any_undecided_opponents(&self.world_players) {
            for p in self.world_players.iter_mut().flatten() {
                p.undecided = false;
            }
        }
    }

    /// `HasClientWonOrLost`: also true for clients without a player.
    pub fn has_client_won_or_lost(&self, client_index: i32) -> bool {
        self.world_players
            .iter()
            .flatten()
            .find(|p| p.client_index == client_index)
            .is_none_or(|p| !p.undecided)
    }

    /// `ICreatePlayersInfo.CreateServerPlayers` without the resolved factions and spawn points, which only
    /// replay metadata uses: the padded player list with the indexes of `world.Players` on the clients.
    fn create_world_players(&mut self) {
        self.world_players = self.map.players.values().filter(|p| !p.playable).map(|_| None).collect();
        for slot in self.lobby.slots.keys() {
            if let Some(c) = self.lobby.client_in_slot(slot) {
                self.world_players.push(Some(WorldPlayer {
                    client_index: c.index,
                    team: c.team,
                    undecided: true,
                }));
            }
        }
        self.world_players.push(None);
    }

    /// `PlayerMessageTracker.DisableChatUI`.
    fn disable_chat_ui(&mut self, player_index: i32, time: u64) {
        let order = RawOrder {
            extra_data: time as u32,
            ..RawOrder::new("DisableChatEntry")
        };
        self.dispatch_frame_to_client(player_index, create_frame(0, 0, &order.serialize()));
    }

    /// `PlayerMessageTracker.IsPlayerAtFloodLimit`.
    fn is_player_at_flood_limit(&mut self, player_index: i32) -> bool {
        let Some(time) = self.conn(player_index).map(|c| c.connection_timer()) else {
            return true;
        };
        let is_admin = self.lobby.client_with_index(player_index).is_some_and(|c| c.is_admin);
        let interval = self.settings.flood_limit_interval;
        let count = self.settings.flood_limit_message_count;
        let tracker = self.message_tracker.entry(player_index).or_default();
        tracker.retain(|t| t + interval >= time);
        let remaining = |cooldown: u64| (cooldown.saturating_sub(time).saturating_add(999) / 1000) as i64;
        let blocked_until = if !is_admin && time < self.settings.flood_limit_join_cooldown {
            Some(self.settings.flood_limit_join_cooldown)
        } else if tracker.len() >= count {
            Some(tracker.first().map_or(0, |t| t + interval))
        } else {
            None
        };
        if let Some(until) = blocked_until {
            self.send_fluent_message_to(
                player_index,
                "notification-chat-temp-disabled",
                &[("remaining", remaining(until).into())],
            );
            return true;
        }
        tracker.push(time);
        if tracker.len() >= count {
            let delta = self.settings.flood_limit_cooldown.saturating_sub(interval);
            tracker.iter_mut().for_each(|t| *t = time + delta);
            self.disable_chat_ui(player_index, self.settings.flood_limit_cooldown);
        }
        false
    }

    fn broadcast_order(&mut self, order: &str, text: &str) {
        self.dispatch_server_orders_to_clients(0, &RawOrder::from_target_string(order, text, true).serialize());
    }

    fn receive_orders(&mut self, player_index: i32, frame: i32, data: &[u8]) {
        if self.conn(player_index).is_none() {
            return;
        }
        if frame == 0 {
            return self.interpret_server_orders(player_index, data);
        }
        let mut frame = frame;
        if data.first() != Some(&order_type::SYNC_HASH) {
            frame += self.order_latency;
            self.dispatch_frame_to_client(player_index, create_ack_frame(frame, 1));
            if let Some(b) = &mut self.order_buffer {
                b.add_order_timestamp(player_index);
            }
            if let Some(c) = self.conn_mut(player_index) {
                c.last_orders_frame = frame;
            }
        }
        self.dispatch_orders_to_clients(player_index, frame, data);
    }

    fn interpret_server_orders(&mut self, player_index: i32, data: &[u8]) {
        let mut r = Reader::new(data);
        while r.remaining() > 0 {
            match RawOrder::deserialize(&mut r) {
                Ok(Some(o)) => self.interpret_server_order(player_index, &o),
                Ok(None) => {}
                Err(_) => break,
            }
        }
    }

    fn interpret_server_order(&mut self, player_index: i32, o: &RawOrder) {
        let target = o.target_string.as_deref().unwrap_or("");
        if !self.is_validated(player_index) {
            if o.order_string == "HandshakeResponse" {
                self.validate_client(player_index, target, &o.order_string);
            } else {
                println!("Rejected connection; order `{}` is not a `HandshakeResponse`.", o.order_string);
                self.drop_client(player_index);
            }
            return;
        }
        match o.order_string.as_str() {
            "Command" => {
                if !self.interpret_command(target, player_index) {
                    println!("Unknown server command: {target}");
                    self.send_fluent_message_to(player_index, "notification-unknown-server-command", &[("command", target.into())]);
                }
            }
            "Chat" if !self.is_player_at_flood_limit(player_index) => {
                self.dispatch_orders_to_clients(player_index, 0, &o.serialize());
            }
            _ => {}
        }
    }

    /// `SendOrderTo`.
    pub fn send_order_to(&mut self, player_index: i32, order: &str, data: &str) {
        let serialized = RawOrder::from_target_string(order, data, true).serialize();
        self.dispatch_frame_to_client(player_index, create_frame(0, 0, &serialized));
    }

    /// `SendMessage`.
    pub fn send_message(&mut self, text: &str) {
        self.broadcast_order("Message", text);
        println!("{text}");
    }

    /// `SendFluentMessage`.
    pub fn send_fluent_message(&mut self, key: &str, args: &[(&str, FluentArg)]) {
        self.broadcast_order("FluentMessage", &serialize_fluent_message(key, args));
        println!("{} {args:?}", self.game_mod.message(key));
    }

    /// `SendFluentMessageTo`.
    pub fn send_fluent_message_to(&mut self, player_index: i32, key: &str, args: &[(&str, FluentArg)]) {
        self.send_order_to(player_index, "FluentMessage", &serialize_fluent_message(key, args));
    }

    fn receive_ping(&mut self, player_index: i32, history: &[i32]) {
        if history.is_empty() {
            return;
        }
        let latency = history.iter().sum::<i32>() / history.len() as i32;
        let quality = if latency < 240 {
            ConnectionQuality::Good
        } else if latency < 360 {
            ConnectionQuality::Moderate
        } else {
            ConnectionQuality::Poor
        };
        for c in &mut self.lobby.clients {
            if c.index == player_index || (c.is_bot() && c.bot_controller_client_index == player_index) {
                c.connection_quality = quality;
            }
        }
        if self.ping_updated.elapsed() > Duration::from_secs(5) {
            let nodes: Vec<MiniYamlNode> = self
                .lobby
                .clients
                .iter()
                .map(|c| MiniYamlNode::with_value_str(format!("ConnectionQuality@{}", c.index), c.connection_quality.name()))
                .collect();
            self.broadcast_order("SyncConnectionQuality", &nodes_to_string(&nodes));
            self.ping_updated = Instant::now();
        }
    }

    /// `DropClient`. Does nothing if the connection is already gone.
    pub fn drop_client(&mut self, player_index: i32) {
        if let Some(b) = &mut self.order_buffer {
            b.remove_player(player_index);
        }
        let Some(position) = self.conns.iter().position(|c| c.player_index == player_index) else {
            return;
        };
        let mut conn = self.conns.remove(position);
        let last_orders_frame = conn.last_orders_frame;
        if let Some(client) = self.lobby.client_with_index(player_index).cloned() {
            self.announce_drop(&client);
            self.lobby.clients.retain(|c| c.index != player_index);
            if client.is_admin && self.state == ServerState::WaitingPlayers {
                self.lobby
                    .clients
                    .retain(|c| !(c.is_bot() && c.bot_controller_client_index == player_index));
                let next_admin = self.lobby.clients.iter_mut().filter(|c| !c.is_bot()).min_by_key(|c| c.index);
                if let Some(a) = next_admin {
                    a.is_admin = true;
                    let name = a.name.clone().unwrap_or_default();
                    self.send_fluent_message("notification-new-admin", &[("player", (&name).into())]);
                }
            }
            let mut packet = vec![order_type::DISCONNECT];
            packet.extend_from_slice(&player_index.to_le_bytes());
            self.dispatch_server_orders_to_clients(last_orders_frame + 1, &packet);
            if !self.conns.iter().any(|c| c.validated) {
                self.server_empty();
            }
            self.sync_lobby_clients();
        }
        conn.dispose();
    }

    fn announce_drop(&mut self, client: &Client) {
        let name = client.name.clone().unwrap_or_default();
        let player = ("player", FluentArg::from(&name));
        if self.state != ServerState::GameStarted {
            self.send_fluent_message("notification-lobby-disconnected", &[player]);
        } else if client.is_observer() {
            self.send_fluent_message("notification-observer-disconnected", &[player]);
        } else if client.team > 0 {
            let team = ("team", FluentArg::from(client.team as i64));
            self.send_fluent_message("notification-team-player-disconnected", &[player, team]);
        } else {
            self.send_fluent_message("notification-player-disconnected", &[player]);
        }
    }

    /// `SyncLobbyInfo`.
    pub fn sync_lobby_info(&mut self) {
        if self.state == ServerState::WaitingPlayers {
            self.broadcast_order("SyncInfo", &self.lobby.serialize());
        }
    }

    /// `SyncLobbyClients`.
    pub fn sync_lobby_clients(&mut self) {
        if self.state == ServerState::WaitingPlayers {
            self.broadcast_order("SyncLobbyClients", &self.lobby.serialize_clients());
            self.ping_updated = Instant::now();
        }
    }

    /// `SyncLobbySlots`.
    pub fn sync_lobby_slots(&mut self) {
        if self.state == ServerState::WaitingPlayers {
            self.broadcast_order("SyncLobbySlots", &self.lobby.serialize_slots());
        }
    }

    /// `SyncLobbyGlobalSettings`.
    pub fn sync_lobby_global_settings(&mut self) {
        if self.state == ServerState::WaitingPlayers {
            self.broadcast_order("SyncLobbyGlobalSettings", &self.lobby.serialize_global_settings());
        }
    }

    /// `StartGame`.
    pub fn start_game(&mut self) {
        println!("{}", self.game_mod.message("notification-game-started"));
        let to_kick: Vec<i32> = self
            .conns
            .iter()
            .filter(|c| {
                !c.validated
                    || self
                        .lobby
                        .client_with_index(c.player_index)
                        .is_none_or(|x| x.state == ClientState::Invalid)
            })
            .map(|c| c.player_index)
            .collect();
        for c in to_kick {
            self.send_order_to(c, "ServerError", "notification-you-were-kicked");
            self.drop_client(c);
        }
        self.lobby.global_settings.game_saves_enabled = false;
        self.create_world_players();
        self.sync_lobby_info();

        let default_speed = self.game_mod.default_speed.clone();
        let speed_name = self.lobby.global_settings.option_str("gamespeed", &default_speed).to_string();
        let speed = self
            .game_mod
            .speeds
            .get(&speed_name)
            .or_else(|| self.game_mod.speeds.get(&default_speed))
            .cloned()
            .expect("mod defines the default game speed");
        let players = self.validated_conn_indices();
        self.order_buffer = Some(OrderBuffer::start(speed.timestep, players.clone()));
        self.state = ServerState::GameStarted;
        self.accepting.store(false, Ordering::Relaxed);
        self.order_latency = speed.order_latency;
        self.broadcast_order("StartGame", "");

        for from in &players {
            for i in 0..self.order_latency {
                let frame = 1 + i;
                if let Some(c) = self.conn_mut(*from) {
                    c.last_orders_frame = frame;
                }
                let frame_data = create_frame(*from, frame, &[]);
                for to in &players {
                    self.dispatch_frame_to_client(*to, frame_data.clone());
                }
            }
        }
    }

    /// `PlayerPinger.Tick`.
    fn pinger_tick(&mut self) {
        let now = run_time(self.start);
        if now.saturating_sub(self.last_ping) <= PING_INTERVAL && !self.is_initial_ping {
            return;
        }
        self.is_initial_ping = false;
        self.last_ping = now;
        for player_index in self.validated_conn_indices() {
            let Some(name) = self
                .lobby
                .client_with_index(player_index)
                .map(|c| c.name.clone().unwrap_or_default())
            else {
                self.drop_client(player_index);
                self.send_fluent_message("notification-player-dropped", &[]);
                continue;
            };
            let start = self.start;
            let Some(c) = self.conn_mut(player_index) else {
                continue;
            };
            let silent = c.time_since_last_response(start);
            if silent < CONN_TIMEOUT {
                if !c.timeout_message_shown && silent > PING_INTERVAL * 2 {
                    c.timeout_message_shown = true;
                    self.send_fluent_message("notification-connection-problems", &[("player", (&name).into())]);
                }
            } else {
                self.send_fluent_message("notification-timeout-dropped", &[("player", (&name).into())]);
                self.drop_client(player_index);
            }
        }
        if now.saturating_sub(self.last_conn_report) > CONN_REPORT_INTERVAL {
            self.last_conn_report = now;
            self.report_timeouts();
        }
    }

    fn report_timeouts(&mut self) {
        let start = self.start;
        let mut timeouts: Vec<(u64, i32)> = self
            .conns
            .iter()
            .filter(|c| c.validated)
            .map(|c| (c.time_since_last_response(start), c.player_index))
            .filter(|(t, _)| *t > CONN_REPORT_INTERVAL && *t < CONN_TIMEOUT)
            .collect();
        timeouts.sort_unstable();
        for (silent, player_index) in timeouts {
            if let Some(name) = self.lobby.client_with_index(player_index).and_then(|c| c.name.clone()) {
                let timeout = ((CONN_TIMEOUT - silent) / 1000) as i64;
                self.send_fluent_message(
                    "notification-timeout-dropped-in",
                    &[("player", (&name).into()), ("timeout", timeout.into())],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_side_standing_wins() {
        let player = |client_index, team| {
            Some(WorldPlayer {
                client_index,
                team,
                undecided: true,
            })
        };
        let mut players = vec![None, player(1, 1), player(2, 1), player(3, 2), None];
        assert!(any_undecided_opponents(&players));
        players[3].as_mut().unwrap().undecided = false;
        assert!(!any_undecided_opponents(&players));
    }

    #[test]
    fn players_without_a_team_are_opponents() {
        let player = |client_index| {
            Some(WorldPlayer {
                client_index,
                team: 0,
                undecided: true,
            })
        };
        assert!(any_undecided_opponents(&[player(1), player(2)]));
        assert!(!any_undecided_opponents(&[player(1)]));
    }

    #[test]
    fn frame_layout() {
        assert_eq!(create_frame(2, 3, &[9]), [5, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 9]);
        assert_eq!(create_ack_frame(7, 1), [6, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, order_type::ACK, 1]);
        assert_eq!(create_tick_scale_frame(1.0).len(), 17);
    }

    #[test]
    fn fluent_message_layout() {
        let text = serialize_fluent_message("notification-joined", &[("player", "Bob".into()), ("team", 2i64.into())]);
        let nodes = MiniYaml::from_str(&text, "test", true).unwrap();
        assert_eq!(nodes[0].key, "FluentMessage");
        assert!(text.contains("\tKey: notification-joined"));
        assert!(text.contains("\t\tArgument@1:\n\t\t\tKey: team\n\t\t\tValue: 2\n\t\t\tType: Number"));
    }

    #[test]
    fn player_names_are_sanitized() {
        assert_eq!(sanitized_player_name("a#b@c"), "abc");
        assert_eq!(sanitized_player_name("Open"), DEFAULT_PLAYER_NAME);
        assert_eq!(sanitized_player_name("12345678901234567890").len(), 16);
    }

    #[test]
    fn ipv6_addresses_are_not_anonymized() {
        assert_eq!(anonymize_ip("10.1.2.3".parse().unwrap()).as_deref(), Some("10.1.2.*"));
        assert_eq!(anonymize_ip("::1".parse().unwrap()), None);
    }
}
