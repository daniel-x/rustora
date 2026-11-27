// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The games that servers advertise in `irc::SERVERS_CHANNEL`, as the client's IRC connection receives them.

use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rustora_net::game_server::GameServer;
use rustora_net::irc_games::{STATE_SHUTTING_DOWN, parse_advertisement};

/// Milliseconds after its last advertisement that a game counts as offline and is dropped: one and a half of the
/// intervals of 1 minute after which servers advertise their games again.
const EXPIRY: i64 = 90 * 1000;

struct Entry {
    nick: String,
    /// The time of sending of the last advertisement, from the line.
    time: i64,
    /// The local time of receiving the last advertisement.
    received: i64,
    game: GameServer,
}

/// The host names of the games with their addresses, filled by threads.
#[derive(Default)]
struct Resolved {
    addresses: Mutex<HashMap<String, Option<IpAddr>>>,
    /// Grows with every resolved name.
    revision: AtomicU64,
}

/// The advertised games by their session guid.
#[derive(Default)]
pub struct IrcGames {
    games: HashMap<String, Entry>,
    resolved: Arc<Resolved>,
    revision: u64,
}

impl IrcGames {
    /// Takes the message `text` from `nick` at `host` in the servers channel, received at `now`.
    pub fn apply_message(&mut self, nick: &str, host: &str, text: &str, now: i64) {
        let Some(ad) = parse_advertisement(text, host, now) else {
            return;
        };
        if self.games.get(&ad.guid).is_some_and(|e| e.time >= ad.time) {
            return;
        }
        if let Some(address) = &ad.game.address {
            self.resolve(address);
        }
        self.games.insert(
            ad.guid,
            Entry {
                nick: nick.to_string(),
                time: ad.time,
                received: now,
                game: ad.game,
            },
        );
        self.revision += 1;
    }

    /// Drops the games of the senders that are no longer in the channel.
    pub fn retain_users(&mut self, users: &[String]) {
        let count = self.games.len();
        self.games.retain(|_, e| users.iter().any(|u| u.eq_ignore_ascii_case(&e.nick)));
        if self.games.len() != count {
            self.revision += 1;
        }
    }

    pub fn clear(&mut self) {
        if !self.games.is_empty() {
            self.games.clear();
            self.revision += 1;
        }
    }

    /// Drops the games that were not advertised again in time.
    pub fn expire(&mut self, now: i64) {
        let count = self.games.len();
        self.games.retain(|_, e| now - e.received <= EXPIRY);
        if self.games.len() != count {
            self.revision += 1;
        }
    }

    /// Changes whenever `games` changes.
    pub fn revision(&self) -> u64 {
        self.revision + self.resolved.revision.load(Ordering::Relaxed)
    }

    /// The games that are not shutting down, with the host names replaced by their addresses once resolved.
    pub fn games(&self) -> Vec<GameServer> {
        let addresses = self.resolved.addresses.lock().map(|a| a.clone()).unwrap_or_default();
        self.games
            .values()
            .filter(|e| e.game.state != STATE_SHUTTING_DOWN)
            .map(|e| {
                let mut game = e.game.clone();
                if let Some((host, port)) = game.address.as_deref().and_then(|a| a.rsplit_once(':'))
                    && let Some(Some(ip)) = addresses.get(host)
                {
                    game.address = Some(match ip {
                        IpAddr::V4(ip) => format!("{ip}:{port}"),
                        IpAddr::V6(ip) => format!("[{ip}]:{port}"),
                    });
                }
                game
            })
            .collect()
    }

    /// Looks up the host name of `address` on a thread, once per name, so that the game can be matched with the
    /// same game from the master server.
    fn resolve(&self, address: &str) {
        let Some((host, _)) = address.rsplit_once(':') else {
            return;
        };
        if host.starts_with('[') || host.parse::<IpAddr>().is_ok() {
            return;
        }
        {
            let Ok(mut addresses) = self.resolved.addresses.lock() else {
                return;
            };
            if addresses.contains_key(host) {
                return;
            }
            addresses.insert(host.to_string(), None);
        }
        let (resolved, host, address) = (self.resolved.clone(), host.to_string(), address.to_string());
        std::thread::spawn(move || {
            let ip = address.to_socket_addrs().ok().and_then(|mut a| a.next()).map(|a| a.ip());
            if let (Some(ip), Ok(mut addresses)) = (ip, resolved.addresses.lock()) {
                addresses.insert(host, Some(ip));
                resolved.revision.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustora_net::irc_games::advertisement;

    const NOW: i64 = 1_764_200_000_000;

    fn game(state: i32) -> GameServer {
        GameServer {
            name: Some("g".into()),
            address: Some("0.0.0.0:1234".into()),
            mod_id: Some("ra".into()),
            state,
            ..Default::default()
        }
    }

    #[test]
    fn newer_advertisements_replace_older_ones() {
        let mut games = IrcGames::default();
        let first = advertisement(&game(1), "a", NOW);
        let second = advertisement(&game(2), "a", NOW + 1);
        games.apply_message("ora_srv_1", "203.0.113.5", &second, NOW + 1);
        games.apply_message("ora_srv_1", "203.0.113.5", &first, NOW + 2);
        assert_eq!(games.games()[0].state, 2);
        games.apply_message("ora_srv_1", "203.0.113.5", &second, NOW + 3);
        assert_eq!(games.revision(), 1);
        games.apply_message("ora_srv_2", "203.0.113.6", &advertisement(&game(3), "b", NOW), NOW);
        assert_eq!(games.games().len(), 1);
        games.retain_users(&["ora_srv_2".into()]);
        assert!(games.games().is_empty());
        games.apply_message("ora_srv_2", "203.0.113.6", &advertisement(&game(1), "b", NOW + 5), NOW + 5);
        games.expire(NOW + 5 + EXPIRY + 1);
        assert!(games.games().is_empty());
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use rustora_net::irc::{self, IrcClient, IrcConfig, IrcEvent};
    use rustora_net::irc_games::{advertisement, is_query, now_millis, query};

    /// A server connection answers the query of a client connection with an advertisement, which the client takes.
    #[test]
    #[ignore = "connects to Libera.Chat"]
    fn query_and_advertisement() {
        let server = IrcClient::connect(IrcConfig {
            channels: vec![irc::SERVERS_CHANNEL.into()],
            nick: Box::new(|attempt| format!("ora_srv_t{attempt:07}")),
            real_name: "rustora test server".into(),
            ipv4_only: true,
        });
        let client = IrcClient::connect(IrcConfig::player("rtstgames", &[irc::SERVERS_CHANNEL]));
        let game = GameServer {
            name: Some("rustora IRC test".into()),
            address: Some("0.0.0.0:1234".into()),
            mod_id: Some("ra".into()),
            state: 1,
            ..Default::default()
        };
        let mut games = IrcGames::default();
        let mut client_joined = false;
        for _ in 0..600 {
            for event in server.poll() {
                match event {
                    IrcEvent::Message { text, .. } if is_query(&text, now_millis()) => {
                        server.send(irc::SERVERS_CHANNEL, &advertisement(&game, "livetest", now_millis()));
                    }
                    IrcEvent::Disconnected(reason) => panic!("server disconnected: {reason}"),
                    _ => {}
                }
            }
            for event in client.poll() {
                match event {
                    IrcEvent::Joined { channel, .. } => {
                        client_joined = true;
                        client.send(&channel, &query(now_millis()));
                    }
                    IrcEvent::Message { nick, host, text, .. } => {
                        eprintln!("{nick}@{host}: {text}");
                        games.apply_message(&nick, &host, &text, now_millis());
                    }
                    IrcEvent::Disconnected(reason) => panic!("client disconnected: {reason}"),
                    IrcEvent::Users { .. } => {}
                }
            }
            if let Some(game) = games.games().first() {
                eprintln!("{:?}", game.address);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("no game, client joined: {client_joined}");
    }

    /// Prints the games that the servers in the channel advertise within 20 s of a query.
    #[test]
    #[ignore = "connects to Libera.Chat"]
    fn list_games() {
        let client = IrcClient::connect(IrcConfig::player("rtstlist", &[irc::SERVERS_CHANNEL]));
        let mut games = IrcGames::default();
        for _ in 0..300 {
            for event in client.poll() {
                match event {
                    IrcEvent::Joined { channel, .. } => client.send(&channel, &query(now_millis())),
                    IrcEvent::Message { nick, host, text, .. } => {
                        if std::env::var_os("IRC_GAMES_RAW").is_some() {
                            eprintln!("{nick}: {text}");
                        }
                        games.apply_message(&nick, &host, &text, now_millis());
                    }
                    IrcEvent::Disconnected(reason) => panic!("disconnected: {reason}"),
                    IrcEvent::Users { .. } => {}
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let mut lines: Vec<String> = games
            .games()
            .iter()
            .map(|g| format!("{} port {} state {}", g.name.as_deref().unwrap_or_default(), g.port, g.state))
            .collect();
        lines.sort();
        eprintln!("{} games\n{}", lines.len(), lines.join("\n"));
    }
}
