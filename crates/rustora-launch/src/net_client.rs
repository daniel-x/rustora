// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The network client: `NetworkConnection` of `OpenRA.Game/Network/Connection.cs`, the lobby orders of
//! `UnitOrders.cs`, and a scripted lobby that matches the C# `RustoraHarness.SetupRemoteLobby`.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};

use rustora_net::stream::ByteStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustora_client::game_information::GameInformation;
use rustora_client::replay::ReplayRecorder;
use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::MiniYaml;
use rustora_engine::order_manager::{Connection, Incoming};
use rustora_engine::world::World;
use rustora_fs::Platform;
use rustora_game::moddata::ModData;
use rustora_net::handshake::{HANDSHAKE_PROTOCOL, HandshakeRequest, HandshakeResponse, ORDERS_PROTOCOL};
use rustora_net::order::{OrderPacket, Packet, RawOrder, hex_dump, order_type};
use rustora_net::session::{Client, ClientState, Session};

use crate::skirmish::BotSpec;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest silence of the server before the client treats the connection as lost. OpenRA and rustora
/// servers ping every client once a second, so only a dead connection stays silent this long. Without it, a
/// connection whose end the client never learns of, for example after a network change, would look alive.
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(30);
/// The log channel of packets that do not follow the protocol, see `rustora_core::log_channels`.
const MALFORMED_PACKETS: &str = "malformed_packets";
/// The received packets kept for the log line of a malformed packet, and the bytes of each.
const RECENT_PACKETS: usize = 64;
const RECENT_BYTES: usize = 32;
/// The bytes of a malformed packet in its log line.
const DUMP_BYTES: usize = 256;

fn read_i32(stream: &mut dyn ByteStream) -> std::io::Result<i32> {
    let mut b = [0u8; 4];
    stream.read_exact(&mut b)?;
    Ok(i32::from_le_bytes(b))
}

/// The silence after which the client logs a gap, to find where a connection stalls.
pub const DIAGNOSTIC_GAP: Duration = Duration::from_secs(5);

/// Reads the frames of the server until the connection ends. A frame grows as its data arrives, so a declared length
/// does not reserve memory by itself.
fn receive_loop(mut stream: Box<dyn ByteStream>, packets: std::sync::mpsc::Sender<(i32, Vec<u8>)>) -> std::io::Result<()> {
    let mut last_packet = Instant::now();
    loop {
        let length = read_i32(stream.as_mut())?;
        let gap = last_packet.elapsed();
        if gap > DIAGNOSTIC_GAP {
            log::warn!(target: "client", "The server sent nothing for {:.1} s.", gap.as_secs_f64());
        }
        last_packet = Instant::now();
        let client = read_i32(stream.as_mut())?;
        if length <= 0 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let mut data = Vec::new();
        (&mut *stream).take(length as u64).read_to_end(&mut data)?;
        if data.len() < length as usize {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        if packets.send((client, data)).is_err() {
            return Ok(());
        }
    }
}

/// `NetworkConnection`: a thread reads packets; orders the client sent wait in queues until the server
/// acknowledges them, because the server does not send a client's own orders back.
pub struct NetworkConnection {
    stream: Box<dyn ByteStream>,
    client_id: i32,
    received: Receiver<(i32, Vec<u8>)>,
    connected: Arc<AtomicBool>,
    sent_orders: VecDeque<Vec<RawOrder>>,
    sent_immediate_orders: VecDeque<Vec<RawOrder>>,
    sent_sync: VecDeque<(i32, i32, u64)>,
    queued_sync: Vec<(i32, i32, u64)>,
    recorder: Option<ReplayRecorder>,
    /// The address of the server, for the log of malformed packets.
    server: String,
    /// The last received packets as sender, length and the start of the data in hex.
    recent: VecDeque<(i32, usize, String)>,
}

/// Closes the stream, so the server notices at once that the client left.
impl Drop for NetworkConnection {
    fn drop(&mut self) {
        self.stream.shutdown_stream();
    }
}

impl NetworkConnection {
    /// Connects to `address` and reads the handshake version and the client index.
    pub fn connect(address: &str) -> Result<NetworkConnection, String> {
        NetworkConnection::connect_with_timeout(address, RECEIVE_TIMEOUT)
    }

    /// Connects like [`NetworkConnection::connect`] and treats `receive_timeout` of silence as a lost connection.
    fn connect_with_timeout(address: &str, receive_timeout: Duration) -> Result<NetworkConnection, String> {
        let addresses: Vec<_> = address.to_socket_addrs().map_err(|e| format!("{address}: {e}"))?.collect();
        let stream = addresses
            .iter()
            .find_map(|a| TcpStream::connect_timeout(a, CONNECT_TIMEOUT).ok())
            .ok_or_else(|| format!("Failed to connect to {address}"))?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(receive_timeout)).map_err(|e| e.to_string())?;
        let mut connection = NetworkConnection::from_stream(Box::new(stream))?;
        connection.server = address.to_string();
        Ok(connection)
    }

    /// Reads the handshake version and the client index from a connected `stream`.
    pub fn from_stream(mut stream: Box<dyn ByteStream>) -> Result<NetworkConnection, String> {
        let protocol = read_i32(stream.as_mut()).map_err(|e| e.to_string())?;
        if protocol != HANDSHAKE_PROTOCOL {
            return Err(format!(
                "Handshake protocol version mismatch. Server={protocol} Client={HANDSHAKE_PROTOCOL}"
            ));
        }
        let client_id = read_i32(stream.as_mut()).map_err(|e| e.to_string())?;
        let reader = stream.try_clone_stream().map_err(|e| e.to_string())?;
        let (send, received) = channel();
        let connected = Arc::new(AtomicBool::new(true));
        let flag = connected.clone();
        std::thread::spawn(move || {
            match receive_loop(reader, send) {
                Ok(()) => log::info!(target: "client", "The connection closed on the client side."),
                Err(e) => log::warn!(target: "client", "The connection ended: {e}"),
            }
            flag.store(false, Ordering::Relaxed);
        });
        Ok(NetworkConnection {
            stream,
            client_id,
            received,
            connected,
            sent_orders: VecDeque::new(),
            sent_immediate_orders: VecDeque::new(),
            sent_sync: VecDeque::new(),
            queued_sync: Vec::new(),
            recorder: None,
            server: String::new(),
            recent: VecDeque::new(),
        })
    }

    /// Logs a packet from client `from` that does not follow the protocol, with the packets before it.
    fn log_malformed(&self, what: &str, from: i32, data: &[u8]) {
        log::warn!(target: MALFORMED_PACKETS,
            "kind=malformed what=\"{what}\" server={} from={from} length={} data={}",
            self.server,
            data.len(),
            hex_dump(data, DUMP_BYTES)
        );
        for (from, length, data) in &self.recent {
            log::warn!(target: MALFORMED_PACKETS, "kind=recent from={from} length={length} data={data}");
        }
    }

    /// `StartRecording`: records the packets to a replay in `directory` once the game starts.
    pub fn start_recording(&mut self, directory: PathBuf, filename: String) {
        self.recorder = Some(ReplayRecorder::new(directory, filename));
    }

    /// The `ReplayRecorder.Metadata` that the replay file ends with.
    pub fn set_replay_metadata(&mut self, info: Arc<Mutex<GameInformation>>) {
        if let Some(r) = &mut self.recorder {
            r.metadata = Some(info);
        }
    }

    fn record(&mut self, client_id: i32, data: &[u8]) {
        if let Some(r) = &mut self.recorder {
            r.receive(client_id, data);
        }
    }

    /// `Send`: a packet followed by the queued sync packets; write errors show up on the reader thread.
    fn send_packet(&mut self, packet: &[u8]) {
        let mut out = Vec::with_capacity(packet.len() + 4);
        out.extend_from_slice(&(packet.len() as i32).to_le_bytes());
        out.extend_from_slice(packet);
        for s in std::mem::take(&mut self.queued_sync) {
            let q = Packet::serialize_sync(s.0, s.1, s.2);
            out.extend_from_slice(&(q.len() as i32).to_le_bytes());
            out.extend_from_slice(&q);
            self.sent_sync.push_back(s);
        }
        let _ = self.stream.write_all(&out);
    }

    /// The ping reply. The queue length is reported as 0, the server does not use it yet.
    fn send_ping_response(&mut self, timestamp: i64) {
        let mut data = 0i32.to_le_bytes().to_vec();
        data.push(order_type::PING);
        data.extend_from_slice(&timestamp.to_le_bytes());
        data.push(0);
        self.send_packet(&data);
    }

    /// Takes the next thing to process, in the order of `IConnection.Receive`.
    pub fn next_incoming(&mut self) -> Option<Incoming> {
        if let Some(orders) = self.sent_immediate_orders.pop_front() {
            self.record(self.client_id, &Packet::serialize_orders(0, &orders));
            return Some(Incoming::Immediate {
                client_id: self.client_id,
                orders: OrderPacket { frame: 0, orders },
            });
        }
        if let Some((frame, sync_hash, defeat_state)) = self.sent_sync.pop_front() {
            self.record(self.client_id, &Packet::serialize_sync(frame, sync_hash, defeat_state));
            return Some(Incoming::Sync {
                frame,
                sync_hash,
                defeat_state,
            });
        }
        loop {
            let (from, data) = match self.received.try_recv() {
                Ok(p) => p,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return None,
            };
            if self.recent.len() == RECENT_PACKETS {
                self.recent.pop_front();
            }
            self.recent.push_back((from, data.len(), hex_dump(&data, RECENT_BYTES)));
            match Packet::parse(from, &data) {
                Ok(Packet::Disconnect { frame, client_id }) => {
                    self.record(from, &data);
                    return Some(Incoming::Disconnect { client_id, frame });
                }
                Ok(Packet::Sync {
                    frame,
                    sync_hash,
                    defeat_state,
                }) => {
                    self.record(from, &data);
                    return Some(Incoming::Sync {
                        frame,
                        sync_hash,
                        defeat_state,
                    });
                }
                Ok(Packet::TickScale(scale)) => return Some(Incoming::TickScale(scale)),
                Ok(Packet::Ping { timestamp }) => self.send_ping_response(timestamp),
                Ok(Packet::Ack { count, .. }) if count as usize > self.sent_orders.len() => {
                    log::warn!(target: "client", "Ignored an Ack for {count} > {} frames.", self.sent_orders.len());
                    self.log_malformed(&format!("Ack for {count} frames, {} sent", self.sent_orders.len()), from, &data);
                }
                Ok(Packet::Ack { frame, count }) => {
                    let orders: Vec<RawOrder> = self.sent_orders.drain(..count as usize).flatten().collect();
                    self.record(self.client_id, &Packet::serialize_orders(frame, &orders));
                    return Some(Incoming::Orders {
                        client_id: self.client_id,
                        orders: OrderPacket { frame, orders },
                    });
                }
                Ok(Packet::Orders(orders)) => {
                    self.record(from, &data);
                    return Some(if orders.frame == 0 {
                        Incoming::Immediate { client_id: from, orders }
                    } else {
                        Incoming::Orders { client_id: from, orders }
                    });
                }
                Err(e) => {
                    log::warn!(target: "client", "Ignored a malformed packet from client {from} with length {}: {e}", data.len());
                    self.log_malformed(&e.to_string(), from, &data);
                }
            }
        }
    }
}

impl Connection for NetworkConnection {
    fn local_client_id(&self) -> i32 {
        self.client_id
    }

    fn send(&mut self, frame: i32, orders: Vec<RawOrder>) {
        let packet = Packet::serialize_orders(frame, &orders);
        self.sent_orders.push_back(orders);
        self.send_packet(&packet);
    }

    fn send_immediate(&mut self, orders: Vec<RawOrder>) {
        let packet = Packet::serialize_orders(0, &orders);
        self.sent_immediate_orders.push_back(orders);
        self.send_packet(&packet);
    }

    /// Sync packets go out together with the next orders.
    fn send_sync(&mut self, frame: i32, sync_hash: i32, defeat_state: u64) {
        self.queued_sync.push((frame, sync_hash, defeat_state));
    }

    fn receive(&mut self, _net_frame_number: i32) -> Vec<Incoming> {
        std::iter::from_fn(|| self.next_incoming()).collect()
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

/// How `join_game` behaves in the lobby.
#[derive(Clone, Debug)]
pub struct JoinOptions {
    pub name: String,
    pub color: Color,
    pub password: String,
    pub bots: Vec<BotSpec>,
    /// With bots: the number of human clients to wait for before setting up the game.
    pub wait_clients: usize,
    /// The name of a save the server should load instead of following the lobby script.
    pub load_game: Option<String>,
    /// The mod of the game, `ra` by default.
    pub mod_id: String,
}

impl Default for JoinOptions {
    fn default() -> Self {
        JoinOptions {
            name: "Commander".to_string(),
            color: Color::from_argb(255, 0x7C, 0xD2, 0xF2),
            password: String::new(),
            bots: Vec::new(),
            wait_clients: 1,
            load_game: None,
            mod_id: "ra".to_string(),
        }
    }
}

/// A game that has started.
pub struct JoinedGame {
    pub world: Rc<World>,
    pub connection: NetworkConnection,
    pub is_host: bool,
}

/// The lobby commands of the test client, the same as those of the C# `RustoraHarness.SetupRemoteLobby`.
struct LobbyScript<'a> {
    options: &'a JoinOptions,
    client_id: i32,
    stage: u8,
}

impl LobbyScript<'_> {
    fn others_ready_to_start(&self, lobby: &Session) -> bool {
        lobby.non_bot_clients().count() >= self.options.wait_clients
            && lobby
                .non_bot_clients()
                .all(|c| c.index == self.client_id || (c.is_observer() && c.state != ClientState::Invalid))
    }

    fn commands(&mut self, lobby: &Session) -> Vec<String> {
        if lobby.client_with_index(self.client_id).is_none() {
            return Vec::new();
        }
        if self.options.bots.is_empty() {
            if self.stage > 0 {
                return Vec::new();
            }
            self.stage = 1;
            return vec!["spectate".to_string(), "state NotReady".to_string()];
        }
        match self.stage {
            0 if self.others_ready_to_start(lobby) => {
                self.stage = 1;
                let mut out = vec!["option gamespeed default".to_string(), "spectate".to_string()];
                for b in &self.options.bots {
                    out.push(format!("slot_bot {} {} {}", b.slot, self.client_id, b.bot));
                }
                out
            }
            1 if lobby.clients.iter().filter(|c| c.is_bot()).count() >= self.options.bots.len() => {
                self.stage = 2;
                let mut out = Vec::new();
                for b in &self.options.bots {
                    let Some(c) = lobby.client_in_slot(&b.slot) else {
                        continue;
                    };
                    if let Some(f) = &b.faction {
                        out.push(format!("faction {} {f}", c.index));
                    }
                    if let Some(s) = b.spawn {
                        out.push(format!("spawn {} {s}", c.index));
                    }
                }
                out.push("state Ready".to_string());
                out
            }
            _ => Vec::new(),
        }
    }
}

/// `UnitOrders.ProcessOrder` for `HandshakeRequest`.
pub fn handshake_response(md: &ModData, request: &str, options: &JoinOptions) -> Result<RawOrder, String> {
    HandshakeRequest::deserialize(request, "HandshakeRequest").map_err(|e| e.to_string())?;
    let color = options.color;
    let response = HandshakeResponse {
        mod_id: Some(md.manifest.id.clone()),
        version: Some(md.manifest.metadata.version.clone()),
        password: Some(options.password.clone()),
        orders_protocol: ORDERS_PROTOCOL,
        client: Client {
            name: Some(options.name.clone()),
            preferred_color: color,
            color,
            faction: Some("Random".to_string()),
            state: ClientState::Invalid,
            ..Default::default()
        },
        ..Default::default()
    };
    Ok(RawOrder {
        is_immediate: true,
        ..RawOrder::handshake("HandshakeResponse", &response.serialize())
    })
}

/// The `SaveLastOrdersFrame` and `SaveSyncFrame` of a `StartGame` order, if it starts a game save.
pub fn parse_game_save_frames(order: &RawOrder) -> Option<(i32, i32)> {
    let text = order.target_string.as_deref().filter(|t| !t.is_empty())?;
    let nodes = MiniYaml::from_str(text, &order.order_string, true).ok()?;
    let frame = |key: &str| nodes.iter().find(|n| n.key == key)?.value.value_str().parse().ok();
    Some((frame("SaveLastOrdersFrame")?, frame("SaveSyncFrame")?))
}

/// Applies a lobby order from the server. Returns true on `StartGame`, an error on a rejection.
fn process_lobby_order(
    md: &ModData,
    connection: &mut NetworkConnection,
    lobby: &mut Session,
    options: &JoinOptions,
    order: &RawOrder,
) -> Result<bool, String> {
    let text = order.target_string.as_deref().unwrap_or("");
    let parse = |t: &str| Session::deserialize(t, &order.order_string).map_err(|e| e.to_string());
    match order.order_string.as_str() {
        "HandshakeRequest" => connection.send_immediate(vec![handshake_response(md, text, options)?]),
        "ServerError" | "AuthenticationError" => return Err(format!("{}: {text}", order.order_string)),
        "SyncInfo" => *lobby = parse(text)?,
        "SyncLobbyClients" => lobby.clients = parse(text)?.clients,
        "SyncLobbySlots" => lobby.slots = parse(text)?.slots,
        "SyncLobbyGlobalSettings" => lobby.global_settings = parse(text)?.global_settings,
        "StartGame" => return Ok(true),
        _ => {}
    }
    Ok(false)
}

/// Connects to a server, follows the lobby script until the game starts and loads the world.
pub fn join_game(engine: &Path, support: &str, address: &str, options: &JoinOptions) -> Result<JoinedGame, String> {
    let md = ModData::load(Platform::new(engine, support), &options.mod_id).map_err(|e| e.to_string())?;
    let mut connection = NetworkConnection::connect(address)?;
    let mut lobby = Session::default();
    let mut script = LobbyScript {
        options,
        client_id: connection.local_client_id(),
        stage: 0,
    };
    let mut load_requested = false;
    let mut software_announced = false;
    loop {
        let Some(incoming) = connection.next_incoming() else {
            if !connection.is_connected() {
                return Err("Connection lost in the lobby".to_string());
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        let Incoming::Immediate { client_id: 0, orders } = incoming else {
            continue;
        };
        for order in &orders.orders {
            if process_lobby_order(&md, &mut connection, &mut lobby, options, order)? {
                let uid = lobby.global_settings.map.clone().ok_or("lobby has no map")?;
                let package = crate::find_map(engine, support, &options.mod_id, &uid).ok_or_else(|| format!("map {uid} not found"))?;
                let is_host = lobby.client_with_index(connection.local_client_id()).is_some_and(|c| c.is_admin);
                let (world, _, _) = crate::load_world(engine, support, package, lobby, false);
                if let Some((last_frame, last_sync_frame)) = parse_game_save_frames(order) {
                    world.begin_loading_game_save(last_frame, last_sync_frame);
                }
                return Ok(JoinedGame {
                    world,
                    connection,
                    is_host,
                });
            }
        }
        if !software_announced && lobby.client_with_index(connection.local_client_id()).is_some() {
            software_announced = true;
            let software = rustora_net::extensions::own_software();
            connection.send_immediate(vec![RawOrder::from_target_string(
                rustora_net::extensions::CLIENT_SOFTWARE,
                &software,
                true,
            )]);
        }
        if let Some(save) = &options.load_game {
            if !load_requested && lobby.client_with_index(connection.local_client_id()).is_some() {
                load_requested = true;
                connection.send_immediate(vec![
                    RawOrder::from_target_string("LoadGameSave", save, true),
                    RawOrder::command("state Ready"),
                ]);
            }
            continue;
        }
        for command in script.commands(&lobby) {
            connection.send_immediate(vec![RawOrder::command(&command)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustora_net::session::Slot;

    #[test]
    fn a_silent_server_counts_as_a_lost_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.write_all(&HANDSHAKE_PROTOCOL.to_le_bytes()).unwrap();
            socket.write_all(&1i32.to_le_bytes()).unwrap();
            std::thread::sleep(Duration::from_secs(2));
            socket
        });
        let connection = NetworkConnection::connect_with_timeout(&address, Duration::from_millis(300)).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        assert!(!connection.is_connected());
        drop(server.join().unwrap());
    }

    fn lobby_with_host() -> Session {
        let mut lobby = Session {
            clients: vec![Client {
                index: 0,
                is_admin: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        lobby.slots.insert(
            "Multi0".to_string(),
            Slot {
                player_reference: "Multi0".to_string(),
                ..Default::default()
            },
        );
        lobby
    }

    #[test]
    fn host_waits_for_clients_then_adds_bots_then_readies() {
        let options = JoinOptions {
            bots: vec![BotSpec {
                slot: "Multi0".to_string(),
                bot: "normal".to_string(),
                faction: Some("russia".to_string()),
                spawn: None,
            }],
            wait_clients: 2,
            ..Default::default()
        };
        let mut script = LobbyScript {
            options: &options,
            client_id: 0,
            stage: 0,
        };
        let mut lobby = lobby_with_host();
        assert!(script.commands(&lobby).is_empty());
        lobby.clients.push(Client {
            index: 1,
            state: ClientState::NotReady,
            ..Default::default()
        });
        assert_eq!(script.commands(&lobby).last().map(String::as_str), Some("slot_bot Multi0 0 normal"));
        lobby.clients.push(Client {
            index: 2,
            bot: Some("normal".to_string()),
            slot: Some("Multi0".to_string()),
            ..Default::default()
        });
        assert_eq!(script.commands(&lobby), ["faction 2 russia", "state Ready"]);
        assert!(script.commands(&lobby).is_empty());
    }

    #[test]
    fn follower_spectates_once() {
        let options = JoinOptions::default();
        let mut script = LobbyScript {
            options: &options,
            client_id: 0,
            stage: 0,
        };
        let lobby = lobby_with_host();
        assert_eq!(script.commands(&lobby), ["spectate", "state NotReady"]);
        assert!(script.commands(&lobby).is_empty());
    }
}
