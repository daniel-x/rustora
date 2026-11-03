// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! A client connection of the server (`OpenRA.Game/Server/Connection.cs`). One thread per connection reads
//! packets, answers pings and writes the send queue; everything else goes to the server thread as an [`Event`].

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use rustora_net::order::order_type;

/// The largest order packet accepted from a multiplayer client.
pub const MAX_ORDER_LENGTH: i32 = 131072;
const MAX_PING_SAMPLES: usize = 15;

/// Events the connection threads send to the server thread.
pub enum Event {
    Connect(TcpStream),
    Packet { player_index: i32, frame: i32, data: Vec<u8> },
    Ping { player_index: i32, history: Vec<i32>, queue_length: u8 },
    Disconnect { player_index: i32 },
}

/// The server thread's handle of a connection.
pub struct Connection {
    pub player_index: i32,
    pub auth_token: String,
    pub end_point: SocketAddr,
    pub validated: bool,
    pub last_orders_frame: i32,
    pub timeout_message_shown: bool,
    send: Option<Sender<Vec<u8>>>,
    last_received: std::sync::Arc<std::sync::atomic::AtomicU64>,
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
    /// Starts the connection thread for `socket`.
    pub fn new(
        player_index: i32,
        auth_token: String,
        socket: TcpStream,
        events: Sender<Event>,
        start: Instant,
        is_multiplayer: bool,
    ) -> std::io::Result<Connection> {
        let end_point = socket.peer_addr()?;
        let (send, queue) = channel();
        let last_received = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let lr = last_received.clone();
        std::thread::Builder::new()
            .name(format!("Client communication ({end_point})"))
            .spawn(move || {
                let _ = send_receive_loop(player_index, socket, queue, &events, start, is_multiplayer, &lr);
                let _ = events.send(Event::Disconnect { player_index });
            })?;
        Ok(Connection {
            player_index,
            auth_token,
            end_point,
            validated: false,
            last_orders_frame: 0,
            timeout_message_shown: false,
            send: Some(send),
            last_received,
        })
    }

    /// Queues `data` for sending; false once the connection is closed.
    pub fn try_send_data(&self, data: Vec<u8>) -> bool {
        self.send.as_ref().is_some_and(|s| s.send(data).is_ok())
    }

    /// Milliseconds since the client last sent data.
    pub fn time_since_last_response(&self, start: Instant) -> u64 {
        run_time(start).saturating_sub(self.last_received.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Tells the connection thread to close the socket.
    pub fn dispose(&mut self) {
        self.send = None;
    }
}

fn send_receive_loop(
    player_index: i32,
    mut socket: TcpStream,
    queue: Receiver<Vec<u8>>,
    events: &Sender<Event>,
    start: Instant,
    is_multiplayer: bool,
    last_received: &std::sync::atomic::AtomicU64,
) -> std::io::Result<()> {
    socket.set_nodelay(true)?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut buf = [0u8; 1024];
    let mut read_buffer: VecDeque<u8> = VecDeque::new();
    let mut expect_length = 8usize;
    let mut in_header = true;
    let mut frame = 0;
    let mut ping_history: VecDeque<i32> = VecDeque::new();
    let mut last_ping_sent = Instant::now();
    loop {
        match socket.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                read_buffer.extend(&buf[..n]);
                last_received.store(run_time(start), std::sync::atomic::Ordering::Relaxed);
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(e) => return Err(e),
        }
        while read_buffer.len() >= expect_length {
            let bytes: Vec<u8> = read_buffer.drain(..expect_length).collect();
            if in_header {
                let length = i32::from_le_bytes(bytes[0..4].try_into().unwrap()) - 4;
                frame = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
                if length < 0 || (is_multiplayer && length > MAX_ORDER_LENGTH) {
                    return Ok(());
                }
                expect_length = length as usize;
                in_header = false;
            } else {
                if bytes.len() == 10 && bytes[0] == order_type::PING {
                    if ping_history.len() == MAX_PING_SAMPLES {
                        ping_history.pop_front();
                    }
                    let sent = i64::from_le_bytes(bytes[1..9].try_into().unwrap());
                    ping_history.push_back((run_time(start) as i64 - sent) as i32);
                    let _ = events.send(Event::Ping {
                        player_index,
                        history: ping_history.iter().copied().collect(),
                        queue_length: bytes[9],
                    });
                } else {
                    let _ = events.send(Event::Packet { player_index, frame, data: bytes });
                }
                expect_length = 8;
                in_header = true;
            }
        }
        if last_ping_sent.elapsed() > Duration::from_secs(1) {
            socket.write_all(&ping_frame(run_time(start)))?;
            last_ping_sent = Instant::now();
        }
        loop {
            match queue.try_recv() {
                Ok(data) => socket.write_all(&data)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }
}
