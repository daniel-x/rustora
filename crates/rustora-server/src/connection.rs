// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! A client connection of the server (`OpenRA.Game/Server/Connection.cs`). A reader thread parses packets and
//! answers ping replies, a writer thread drains the send queue and sends pings; everything else goes to the
//! server thread as an [`Event`]. Two blocking threads send each packet as soon as it is queued. The stream is a TCP
//! socket, or an in-memory pipe for a client in the same process.

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use rustora_net::order::order_type;
use rustora_net::stream::ByteStream;

/// The largest order packet accepted from a multiplayer client.
pub const MAX_ORDER_LENGTH: i32 = 131072;
const MAX_PING_SAMPLES: usize = 15;
const PING_INTERVAL: Duration = Duration::from_secs(1);

/// Where a client connects from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientOrigin {
    /// A client on the network, at this address.
    Network(SocketAddr),
    /// A client in the same process, connected through an in-memory pipe.
    InProcess,
}

impl ClientOrigin {
    /// The IP address of a network client, mapped IPv4 addresses as IPv4.
    pub fn ip(self) -> Option<std::net::IpAddr> {
        match self {
            ClientOrigin::Network(address) => Some(address.ip().to_canonical()),
            ClientOrigin::InProcess => None,
        }
    }
}

impl std::fmt::Display for ClientOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientOrigin::Network(address) => write!(f, "{address}"),
            ClientOrigin::InProcess => f.write_str("in-process client"),
        }
    }
}

/// Events the listener and connection threads send to the server thread.
pub enum Event {
    /// A new client stream and where it comes from.
    Connect(Box<dyn ByteStream>, ClientOrigin),
    Packet {
        player_index: i32,
        frame: i32,
        data: Vec<u8>,
    },
    Ping {
        player_index: i32,
        history: Vec<i32>,
        queue_length: u8,
    },
    Disconnect {
        player_index: i32,
    },
    /// The outcome of a profile lookup: the profile, or why authentication failed.
    Authenticated {
        player_index: i32,
        profile: Result<crate::auth::PlayerProfile, String>,
    },
    /// A fluent message to all clients, from the master server thread.
    MasterServerMessage(String),
    /// Ends the server, from the client process that hosts it.
    Shutdown,
}

/// The server thread's handle of a connection.
pub struct Connection {
    pub player_index: i32,
    pub auth_token: String,
    pub origin: ClientOrigin,
    pub validated: bool,
    pub last_orders_frame: i32,
    pub timeout_message_shown: bool,
    connected_at: Instant,
    send: Option<Sender<Vec<u8>>>,
    last_received: Arc<AtomicU64>,
}

/// Milliseconds since the server started, the `Game.RunTime` of the server process.
pub fn run_time(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

fn ping_frame(now: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(21);
    v.extend_from_slice(&13i32.to_le_bytes());
    v.extend_from_slice(&0i32.to_le_bytes());
    v.extend_from_slice(&0i32.to_le_bytes());
    v.push(order_type::PING);
    v.extend_from_slice(&(now as i64).to_le_bytes());
    v
}

impl Connection {
    /// Starts the threads of `socket`, which comes from `origin`.
    pub fn new(
        player_index: i32,
        auth_token: String,
        socket: Box<dyn ByteStream>,
        origin: ClientOrigin,
        events: Sender<Event>,
        start: Instant,
        is_multiplayer: bool,
    ) -> std::io::Result<Connection> {
        let writer_socket = socket.try_clone_stream()?;
        let (send, queue) = channel();
        let last_received = Arc::new(AtomicU64::new(run_time(start)));
        let lr = last_received.clone();
        std::thread::Builder::new().name(format!("Client read ({origin})")).spawn(move || {
            let _ = read_loop(player_index, socket, &events, start, is_multiplayer, &lr);
            let _ = events.send(Event::Disconnect { player_index });
        })?;
        std::thread::Builder::new()
            .name(format!("Client write ({origin})"))
            .spawn(move || write_loop(writer_socket, &queue, start))?;
        Ok(Connection {
            player_index,
            auth_token,
            origin,
            validated: false,
            last_orders_frame: 0,
            timeout_message_shown: false,
            connected_at: Instant::now(),
            send: Some(send),
            last_received,
        })
    }

    /// Queues `data` for sending; false once the connection is closed.
    pub fn try_send_data(&self, data: Vec<u8>) -> bool {
        self.send.as_ref().is_some_and(|s| s.send(data).is_ok())
    }

    /// Milliseconds since the client connected, the `ConnectionTimer`.
    pub fn connection_timer(&self) -> u64 {
        self.connected_at.elapsed().as_millis() as u64
    }

    /// Milliseconds since the client last sent data.
    pub fn time_since_last_response(&self, start: Instant) -> u64 {
        run_time(start).saturating_sub(self.last_received.load(Ordering::Relaxed))
    }

    /// Closes the socket after the queued data is sent.
    pub fn dispose(&mut self) {
        self.send = None;
    }
}

/// Sends queued data and pings until the queue is closed or a write fails, then closes the socket,
/// which also ends the read loop.
fn write_loop(mut socket: Box<dyn ByteStream>, queue: &Receiver<Vec<u8>>, start: Instant) {
    let mut next_ping = Instant::now() + PING_INTERVAL;
    loop {
        let result = match queue.recv_timeout(next_ping.saturating_duration_since(Instant::now())) {
            Ok(data) => socket.write_all(&data),
            Err(RecvTimeoutError::Timeout) => {
                next_ping = Instant::now() + PING_INTERVAL;
                socket.write_all(&ping_frame(run_time(start)))
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if result.is_err() {
            break;
        }
    }
    socket.shutdown_stream();
}

fn read_exact_counted(socket: &mut Box<dyn ByteStream>, buf: &mut [u8], start: Instant, last_received: &AtomicU64) -> std::io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        match socket.read(&mut buf[filled..])? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => {
                filled += n;
                last_received.store(run_time(start), Ordering::Relaxed);
            }
        }
    }
    Ok(())
}

fn read_loop(
    player_index: i32,
    mut socket: Box<dyn ByteStream>,
    events: &Sender<Event>,
    start: Instant,
    is_multiplayer: bool,
    last_received: &AtomicU64,
) -> std::io::Result<()> {
    let mut ping_history = std::collections::VecDeque::with_capacity(MAX_PING_SAMPLES);
    loop {
        let mut header = [0u8; 8];
        read_exact_counted(&mut socket, &mut header, start, last_received)?;
        let length = i32::from_le_bytes(header[0..4].try_into().unwrap()) - 4;
        let frame = i32::from_le_bytes(header[4..8].try_into().unwrap());
        if length < 0 || (is_multiplayer && length > MAX_ORDER_LENGTH) {
            return Ok(());
        }
        let mut data = vec![0u8; length as usize];
        read_exact_counted(&mut socket, &mut data, start, last_received)?;
        if data.len() == 10 && data[0] == order_type::PING {
            if ping_history.len() == MAX_PING_SAMPLES {
                ping_history.pop_front();
            }
            let sent = i64::from_le_bytes(data[1..9].try_into().unwrap());
            ping_history.push_back((run_time(start) as i64 - sent) as i32);
            let _ = events.send(Event::Ping {
                player_index,
                history: ping_history.iter().copied().collect(),
                queue_length: data[9],
            });
        } else {
            let _ = events.send(Event::Packet { player_index, frame, data });
        }
    }
}
