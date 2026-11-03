// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Drives a real server over TCP with raw protocol clients. Skips without the OpenRA reference checkout.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine;
use rsa::pkcs8::{EncodePublicKey, LineEnding};
use rsa::{Pkcs1v15Sign, RsaPrivateKey};
use rustora_net::binary::Reader;
use rustora_net::handshake::{HandshakeRequest, HandshakeResponse, ORDERS_PROTOCOL};
use rustora_net::order::{RawOrder, order_type};
use rustora_net::session::{Client, ClientState, Session};
use rustora_server::mod_info::ServerMod;
use rustora_server::server::{Server, ServerSettings, ServerType};
use sha1::{Digest, Sha1};

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

/// The fingerprint and key a client authenticates with.
type Identity = (String, RsaPrivateKey);

struct TestClient {
    identity: Option<Identity>,
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
            identity: None,
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
                let auth_signature = self
                    .identity
                    .as_ref()
                    .map(|(_, key)| sign(key, request.auth_token.as_deref().unwrap_or("")));
                let response = HandshakeResponse {
                    fingerprint: self.identity.as_ref().map(|(f, _)| f.clone()),
                    auth_signature,
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
            "StartGame" => self.messages.push(format!("StartGame {text}")),
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

fn sign(key: &RsaPrivateKey, data: &str) -> String {
    let hash = Sha1::digest(data.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(key.sign(Pkcs1v15Sign::new::<Sha1>(), &hash).unwrap())
}

/// A web server for one kind of answer; returns its URL and the requests as `first line` and body.
fn start_http_stub(answer: String) -> (String, std::sync::mpsc::Receiver<(String, String)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let (send, requests) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            let (head, body) = loop {
                let n = stream.read(&mut buf).unwrap();
                data.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&data).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break (head.to_string(), body.to_string());
                    }
                }
            };
            let first = head.lines().next().unwrap_or("").to_string();
            let _ = send.send((first, body));
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
        }
    });
    (url, requests)
}

/// A key and the answer of the player database for it.
fn identity() -> (Identity, String) {
    let key = RsaPrivateKey::new(&mut rand::thread_rng(), 1024).unwrap();
    let pem = key.to_public_key().to_public_key_pem(LineEnding::LF).unwrap();
    let public_key = base64::engine::general_purpose::STANDARD.encode(pem);
    let yaml =
        format!("Player:\n\tFingerprint: abc\n\tPublicKey: {public_key}\n\tKeyRevoked: False\n\tProfileID: 42\n\tProfileName: Tester\n");
    (("abc".to_string(), key), yaml)
}

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

    assert!(admin.pump(VERSION, |c| c.lobby.client_in_slot("Multi1").is_some()));
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
    assert!(admin.pump(VERSION, |c| c.lobby.client_in_slot("Multi1").is_some()));
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

fn sync_packet(hash: i32) -> Vec<u8> {
    let mut p = vec![order_type::SYNC_HASH];
    p.extend_from_slice(&hash.to_le_bytes());
    p.extend_from_slice(&0u64.to_le_bytes());
    p
}

fn local_server(engine: PathBuf, support: PathBuf) -> u16 {
    start_server(engine, move |s| {
        s.server_type = ServerType::Local;
        s.support_dir = support;
    })
}

#[test]
fn local_server_saves_a_game_and_loads_it_again() {
    let Some(engine) = engine_dir() else { return };
    let support = std::env::temp_dir().join(format!("rustora-save-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&support);

    let mut a = join(local_server(engine.clone(), support.clone()));
    a.command("spectate");
    a.command("slot_bot Multi0 0 normal");
    assert!(a.pump(VERSION, |c| c.lobby.client_in_slot("Multi0").is_some()));
    a.command("state Ready");
    assert!(a.pump(VERSION, |c| c.got("StartGame") && !c.frames.is_empty()));
    assert!(a.lobby.global_settings.enable_singleplayer);
    a.send_packet(1, &[]);
    a.send_packet(1, &sync_packet(1234));
    a.send_packet(2, &[]);
    assert!(a.pump(VERSION, |c| c.frames.iter().any(|f| f.frame == 3)));
    let trait_data = RawOrder::from_target_string("GameSaveTraitData", "3:\n\tKey: value", true);
    a.send_packet(0, &trait_data.serialize());
    let create = RawOrder::from_target_string("CreateGameSave", "../test.orasav", true);
    a.send_packet(0, &create.serialize());
    assert!(a.pump(VERSION, |c| c.got("..test.orasav")));
    let file = support.join("Saves/ra").join(VERSION).join("..test.orasav");
    assert!(file.exists());
    drop(a);

    let mut b = join(local_server(engine.clone(), support.clone()));
    let load = RawOrder::from_target_string("LoadGameSave", "..test.orasav", true);
    b.send_packet(0, &load.serialize());
    assert!(b.pump(VERSION, |c| {
        c.lobby.client_in_slot("Multi0").is_some_and(|x| x.bot.as_deref() == Some("normal"))
    }));
    b.command("state Ready");
    assert!(b.pump(VERSION, |c| c.got("SaveLastOrdersFrame") && c.frames.iter().any(|f| f.frame == 4)));
    assert!(b.got("SaveSyncFrame: 1"));
    assert!(b.got("Key: value"));
    let frames: Vec<i32> = b.frames.iter().filter(|f| f.data.is_empty()).map(|f| f.frame).collect();
    assert_eq!(frames, [1, 2, 3, 4]);
    assert_eq!(
        b.frames.iter().find(|f| f.frame == 1 && !f.data.is_empty()).unwrap().data,
        sync_packet(1234)
    );
    drop(b);

    let dedicated = start_server(engine, move |s| s.support_dir = support);
    let mut c = join(dedicated);
    c.send_packet(0, &load.serialize());
    c.command("spectate");
    let index = c.index;
    assert!(c.pump(VERSION, |x| x.lobby.client_with_index(index).is_some_and(Client::is_observer)));
    assert!(c.lobby.clients.iter().all(|x| !x.is_bot()));
}

#[test]
fn valid_signature_sets_the_fingerprint() {
    let Some(engine) = engine_dir() else { return };
    let ((fingerprint, key), yaml) = identity();
    let (url, requests) = start_http_stub(yaml);
    let port = start_server(engine, move |s| {
        s.player_profile_url = url;
        s.require_authentication = true;
    });
    let mut c = TestClient::connect(port);
    c.identity = Some((fingerprint, key));
    assert!(c.pump(VERSION, TestClient::joined));
    assert_eq!(c.lobby.client_with_index(c.index).unwrap().fingerprint.as_deref(), Some("abc"));
    assert!(requests.recv().unwrap().0.starts_with("GET /abc "));
}

#[test]
fn bad_signature_counts_as_not_authenticated() {
    let Some(engine) = engine_dir() else { return };
    let ((fingerprint, _), yaml) = identity();
    let ((_, other_key), _) = identity();
    let (url, _requests) = start_http_stub(yaml);
    let url2 = url.clone();
    let port = start_server(engine.clone(), move |s| {
        s.player_profile_url = url;
        s.require_authentication = true;
    });
    let mut c = TestClient::connect(port);
    c.identity = Some((fingerprint.clone(), other_key.clone()));
    assert!(!c.pump(VERSION, TestClient::joined));
    assert!(c.got("notification-requires-authentication"), "{:?}", c.messages);

    let port = start_server(engine, move |s| s.player_profile_url = url2);
    let mut c = TestClient::connect(port);
    c.identity = Some((fingerprint, other_key));
    assert!(c.pump(VERSION, TestClient::joined));
    assert_eq!(c.lobby.client_with_index(c.index).unwrap().fingerprint, None);
}

#[test]
fn unsigned_client_and_blacklisted_profile_are_rejected() {
    let Some(engine) = engine_dir() else { return };
    let port = start_server(engine.clone(), |s| s.require_authentication = true);
    let mut c = TestClient::connect(port);
    assert!(!c.pump(VERSION, TestClient::joined));
    assert!(c.got("notification-requires-authentication"), "{:?}", c.messages);

    let (identity, yaml) = identity();
    let (url, _requests) = start_http_stub(yaml);
    let port = start_server(engine, move |s| {
        s.player_profile_url = url;
        s.profile_id_blacklist = vec![42];
    });
    let mut c = TestClient::connect(port);
    c.identity = Some(identity);
    assert!(!c.pump(VERSION, TestClient::joined));
    assert!(c.got("notification-no-permission-to-join"), "{:?}", c.messages);
}

#[test]
fn advertising_posts_the_game_and_its_changes() {
    let Some(engine) = engine_dir() else { return };
    let (url, requests) = start_http_stub(String::new());
    let port = start_server(engine, move |s| {
        s.advertise_online = true;
        s.server_advertise_url = url;
        s.name = "Stub Game".to_string();
    });
    let timeout = Duration::from_secs(60);
    let (first, body) = requests.recv_timeout(timeout).unwrap();
    assert!(first.starts_with("POST "), "{first}");
    assert!(body.starts_with("Game:\n\tProtocol: 2\n\tName: Stub Game\n"), "{body}");
    assert!(body.contains("\tState: 1\n"), "{body}");
    assert!(body.contains(&format!("\tAddress: 0.0.0.0:{port}\n")), "{body}");
    let mut c = join(port);
    let deadline = Instant::now() + timeout;
    let name = format!("Player{}", c.index);
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        let (_, body) = requests.recv_timeout(wait).expect("no update with the client");
        if body.contains(&name) {
            break;
        }
        c.command("spectate");
    }
}

#[test]
fn servers_are_silent_towards_the_master_server_by_default() {
    let Some(engine) = engine_dir() else { return };
    let port = start_server(engine, |_| {});
    let c = join(port);
    assert!(c.joined());
    assert!(!ServerSettings::default().advertise_online);
    assert!(!ServerSettings::default().discover_nat_devices);
}
