// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Drives a real server over TCP with raw protocol clients. Skips without the OpenRA reference checkout.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rustora_net::binary::Reader;
use rustora_net::handshake::{HandshakeRequest, HandshakeResponse, ORDERS_PROTOCOL};
use rustora_net::order::{RawOrder, order_type};
use rustora_net::session::{Client, ClientState, Session};
use rustora_server::mod_info::ServerMod;
use rustora_server::server::{Server, ServerSettings};

const MAP: &str = "ore-lord.oramap";

fn engine_dir() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../openra-ref");
    p.join("mods/ra/mod.yaml").exists().then_some(p)
}

/// Starts a server thread on a free port and returns the port.
fn start_server(engine: PathBuf, configure: impl FnOnce(&mut ServerSettings) + Send + 'static) -> u16 {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let support = std::env::temp_dir().join(format!("rustora-server-test-{port}"));
    std::thread::spawn(move || {
        let mut settings = ServerSettings {
            listen_port: port,
            map: Some(MAP.to_string()),
            enable_singleplayer: true,
            seed: Some(7),
            support_dir: support.clone(),
            ..Default::default()
        };
        configure(&mut settings);
        let game_mod = ServerMod::load(&engine, support.to_str().unwrap(), "ra").unwrap();
        Server::new(settings, game_mod).unwrap().run();
    });
    port
}

/// One packet from the server.
struct Frame {
    client: i32,
    frame: i32,
    data: Vec<u8>,
}

struct TestClient {
    stream: TcpStream,
    index: i32,
    lobby: Session,
    messages: Vec<String>,
    frames: Vec<Frame>,
}

impl TestClient {
    fn connect(port: u16) -> TestClient {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut stream = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => break s,
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => panic!("connect: {e}"),
            }
        };
        stream.set_nodelay(true).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut hello = [0u8; 8];
        stream.read_exact(&mut hello).unwrap();
        assert_eq!(i32::from_le_bytes(hello[0..4].try_into().unwrap()), 7);
        TestClient {
            stream,
            index: i32::from_le_bytes(hello[4..8].try_into().unwrap()),
            lobby: Session::default(),
            messages: Vec::new(),
            frames: Vec::new(),
        }
    }

    fn send_packet(&mut self, frame: i32, data: &[u8]) {
        let mut out = (data.len() as i32 + 4).to_le_bytes().to_vec();
        out.extend_from_slice(&frame.to_le_bytes());
        out.extend_from_slice(data);
        self.stream.write_all(&out).unwrap();
    }

    fn command(&mut self, text: &str) {
        self.send_packet(0, &RawOrder::command(text).serialize());
    }

    fn read_frame(&mut self) -> Option<Frame> {
        let mut header = [0u8; 8];
        self.stream.read_exact(&mut header).ok()?;
        let length = i32::from_le_bytes(header[0..4].try_into().unwrap());
        let client = i32::from_le_bytes(header[4..8].try_into().unwrap());
        let mut body = vec![0u8; length as usize];
        self.stream.read_exact(&mut body).ok()?;
        Some(Frame {
            client,
            frame: i32::from_le_bytes(body[0..4].try_into().unwrap()),
            data: body[4..].to_vec(),
        })
    }

    /// Applies server orders until `done` holds. Returns false if the server closes the connection first.
    fn pump(&mut self, version: &str, mut done: impl FnMut(&TestClient) -> bool) -> bool {
        while !done(self) {
            let Some(frame) = self.read_frame() else {
                return false;
            };
            if frame.frame != 0 {
                self.frames.push(frame);
                continue;
            }
            if frame.data.first() == Some(&order_type::PING) {
                continue;
            }
            let mut r = Reader::new(&frame.data);
            while r.remaining() > 0 {
                let Ok(Some(order)) = RawOrder::deserialize(&mut r) else {
                    break;
                };
                self.apply(&order, version);
            }
        }
        true
    }

    fn apply(&mut self, order: &RawOrder, version: &str) {
        let text = order.target_string.clone().unwrap_or_default();
        match order.order_string.as_str() {
            "HandshakeRequest" => {
                let request = HandshakeRequest::deserialize(&text, "test").unwrap();
                assert_eq!(request.mod_id.as_deref(), Some("ra"));
                let response = HandshakeResponse {
                    mod_id: request.mod_id,
                    version: Some(version.to_string()),
                    orders_protocol: ORDERS_PROTOCOL,
                    client: Client {
                        name: Some(format!("Player{}", self.index)),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let order = RawOrder::handshake("HandshakeResponse", &response.serialize());
                self.send_packet(0, &order.serialize());
            }
            "SyncInfo" => self.lobby = Session::deserialize(&text, "test").unwrap(),
            "SyncLobbyClients" => self.lobby.clients = Session::deserialize(&text, "test").unwrap().clients,
            "SyncLobbySlots" => self.lobby.slots = Session::deserialize(&text, "test").unwrap().slots,
            "StartGame" => self.messages.push("StartGame".to_string()),
            _ => self.messages.push(text),
        }
    }

    fn joined(&self) -> bool {
        self.lobby.client_with_index(self.index).is_some()
    }

    fn got(&self, message: &str) -> bool {
        self.messages.iter().any(|m| m.contains(message))
    }
}

const VERSION: &str = "release-20250330";

fn join(port: u16) -> TestClient {
    let mut c = TestClient::connect(port);
    assert!(c.pump(VERSION, TestClient::joined), "client was not accepted");
    c
}

#[test]
fn wrong_version_is_rejected() {
    let Some(engine) = engine_dir() else { return };
    let port = start_server(engine, |_| {});
    let mut c = TestClient::connect(port);
    assert!(!c.pump("other-version", TestClient::joined));
    assert!(c.got("notification-incompatible-version"), "{:?}", c.messages);
}

#[test]
fn wrong_password_is_rejected() {
    let Some(engine) = engine_dir() else { return };
    let port = start_server(engine, |s| s.password = "secret".to_string());
    let mut c = TestClient::connect(port);
    assert!(!c.pump(VERSION, TestClient::joined));
    assert!(c.got("notification-requires-password"), "{:?}", c.messages);
}

#[test]
fn lobby_has_map_settings_and_only_the_admin_changes_them() {
    let Some(engine) = engine_dir() else { return };
    let port = start_server(engine, |_| {});
    let mut admin = join(port);
    let mut other = join(port);
    let client = |c: &TestClient, i: i32| c.lobby.client_with_index(i).cloned().unwrap();
    assert!(client(&admin, admin.index).is_admin);
    assert!(!client(&other, other.index).is_admin);

    let options = &other.lobby.global_settings.lobby_options;
    for (id, value) in [
        ("gamespeed", "default"),
        ("fog", "True"),
        ("explored", "False"),
        ("cheats", "False"),
        ("startingcash", "5000"),
    ] {
        assert_eq!(options.get(id).and_then(|o| o.value.as_deref()), Some(value), "option {id}");
    }

    other.command("slot_bot Multi0 1 normal");
    assert!(other.pump(VERSION, |c| c.got("notification-requires-host")));
    other.command("option crates False");
    assert!(other.pump(VERSION, |c| c.got("notification-admin-change-configuration")));
    other.command("no_such_command");
    assert!(other.pump(VERSION, |c| c.got("notification-unknown-server-command")));

    other.command("spectate");
    assert!(admin.pump(VERSION, |c| c.lobby.client_in_slot("Multi1").is_none()));
    admin.command("slot_bot Multi1 0 no_such_bot");
    assert!(admin.pump(VERSION, |c| c.got("notification-invalid-bot-type")));
    admin.command("slot_bot Multi1 0 normal");
    assert!(admin.pump(VERSION, |c| c.lobby.client_in_slot("Multi1").is_some_and(Client::is_bot)));
}

#[test]
fn leaving_admin_hands_over_and_removes_bots() {
    let Some(engine) = engine_dir() else { return };
    let port = start_server(engine, |_| {});
    let mut admin = join(port);
    let mut other = join(port);
    other.command("spectate");
    assert!(admin.pump(VERSION, |c| c.lobby.client_in_slot("Multi1").is_none()));
    admin.command("slot_bot Multi1 0 normal");
    assert!(other.pump(VERSION, |c| c.lobby.client_in_slot("Multi1").is_some_and(Client::is_bot)));
    drop(admin);
    let index = other.index;
    assert!(other.pump(VERSION, |c| c.lobby.client_with_index(index).is_some_and(|x| x.is_admin)));
    assert!(other.lobby.clients.iter().all(|c| !c.is_bot()));
}

#[test]
fn game_start_sends_empty_frames_and_acks_orders() {
    let Some(engine) = engine_dir() else { return };
    let port = start_server(engine, |_| {});
    let mut a = join(port);
    let mut b = join(port);
    b.command("spectate");
    b.command("state NotReady");
    let b_index = b.index;
    assert!(a.pump(VERSION, |c| {
        c.lobby
            .client_with_index(b_index)
            .is_some_and(|x| x.is_observer() && x.state == ClientState::NotReady)
    }));
    a.command("spectate");
    a.command("slot_bot Multi0 0 normal");
    assert!(a.pump(VERSION, |c| c.lobby.client_in_slot("Multi0").is_some()));
    a.command("state Ready");
    assert!(a.pump(VERSION, |c| c.got("StartGame")));
    assert!(b.pump(VERSION, |c| c.got("StartGame")));

    assert!(a.pump(VERSION, |c| c.frames.len() >= 6));
    let empty: Vec<(i32, i32)> = a.frames.iter().map(|f| (f.client, f.frame)).collect();
    assert_eq!(
        empty,
        [(a.index, 1), (a.index, 2), (a.index, 3), (b.index, 1), (b.index, 2), (b.index, 3)]
    );
    a.frames.clear();
    b.frames.clear();

    a.send_packet(1, &[]);
    assert!(a.pump(VERSION, |c| !c.frames.is_empty()));
    assert_eq!(a.frames[0].frame, 4);
    assert_eq!(a.frames[0].data, [order_type::ACK, 1]);
    assert!(b.pump(VERSION, |c| c.frames.iter().any(|f| f.frame == 4)));

    let (a_index, b_index) = (a.index, b.index);
    drop(a);
    assert!(b.pump(VERSION, |c| {
        c.frames
            .iter()
            .any(|f| f.frame == 5 && f.data.first() == Some(&order_type::DISCONNECT))
    }));
    let disconnect = b.frames.iter().find(|f| f.data.first() == Some(&order_type::DISCONNECT)).unwrap();
    assert_eq!(disconnect.client, 0);
    assert_eq!(disconnect.data[1..5], a_index.to_le_bytes());
    assert_ne!(a_index, b_index);
}
