// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Announces the game to LAN clients and, when `AdvertiseOnline` is set, to the master server
//! (`OpenRA.Mods.Common/ServerTraits/MasterServerPinger.cs`) and, a rustora addition, in the IRC channel
//! `irc::SERVERS_CHANNEL` (see `rustora_net::irc_games`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use rustora_core::world_types::MersenneTwister;
use rustora_net::beacon::{Beacon, DISCOVERY_PORT, LAN_GAME_BEACON};
use rustora_net::game_server::{GameClient, GameServer};
use rustora_net::irc::{self, IrcClient, IrcConfig, IrcEvent, NICK_LEN};
use rustora_net::irc_games::{self, now_millis};

use crate::connection::Event;
use crate::server::{Server, ServerState};

const MASTER_PING_INTERVAL: u64 = 3 * 60 * 1000;
const RATE_LIMIT_INTERVAL: u64 = 1000;
const NO_PORT_FORWARD: &str = "notification-no-port-forward";
const BLACKLISTED_TITLE: &str = "notification-blacklisted-server-name";
const INVALID_ERROR_CODE: &str = "notification-invalid-error-code";
const CONNECTED: &str = "notification-master-server-connected";
const ERROR: &str = "notification-master-server-error";
const GAME_OFFLINE: &str = "notification-game-offline";
/// The shortest time between two advertisements in IRC, against flooding the channel.
const IRC_RATE_LIMIT: u64 = 5000;
/// The time before a lost IRC connection is tried again.
const IRC_RETRY_DELAY: u64 = 60_000;
const IRC_NICK_PREFIX: &str = "ora_srv_";

/// The connection that advertises the game in IRC.
#[derive(Default)]
struct IrcAdvertiser {
    client: Option<IrcClient>,
    joined: bool,
    /// An advertisement waits for the rate limit.
    pending: bool,
    last_sent: Option<u64>,
    retry_at: u64,
}

pub struct MasterServerPinger {
    beacon: Option<Beacon>,
    session_guid: String,
    last_ping: u64,
    last_changed: u64,
    is_initial_ping: Arc<AtomicBool>,
    is_busy: Arc<AtomicBool>,
    is_multiplayer: bool,
    /// The seed of the IRC nicks.
    nick_seed: u32,
    irc: IrcAdvertiser,
}

impl MasterServerPinger {
    /// Starts the LAN beacon. Failure only means that LAN clients will not see the game.
    pub fn start(random: &mut MersenneTwister, is_multiplayer: bool) -> MasterServerPinger {
        let advertised_port = random.next_range(2048, 60000) as u16;
        let beacon = is_multiplayer
            .then(|| {
                Beacon::start(LAN_GAME_BEACON, DISCOVERY_PORT, advertised_port)
                    .inspect_err(|e| log::info!(target: "server", "LAN beacon failed: {e}"))
                    .ok()
            })
            .flatten();
        MasterServerPinger {
            beacon,
            session_guid: format!("{:08x}{:08x}", random.next(), random.next()),
            last_ping: 0,
            last_changed: 0,
            is_initial_ping: Arc::new(AtomicBool::new(true)),
            is_busy: Arc::new(AtomicBool::new(false)),
            is_multiplayer,
            nick_seed: random.next() as u32,
            irc: IrcAdvertiser::default(),
        }
    }
}

/// The messages to show after the first answer of the master server, an optional `[code]message` text.
pub fn parse_master_response(text: &str) -> Vec<String> {
    let mut out = vec![CONNECTED.to_string()];
    let text = text.trim();
    if text.is_empty() {
        return out;
    }
    let parsed = text
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .and_then(|(code, message)| {
            let code: i32 = code.parse().ok()?;
            Some((code, message.trim()))
        });
    let (code, message) = match parsed {
        Some(p) => p,
        None => (0, INVALID_ERROR_CODE),
    };
    if code != 0 {
        out.push(
            match code {
                1 => NO_PORT_FORWARD,
                2 => BLACKLISTED_TITLE,
                _ => message,
            }
            .to_string(),
        );
        if code > 0 {
            out.push(GAME_OFFLINE.to_string());
        }
    }
    out
}

fn state_number(state: ServerState) -> i32 {
    match state {
        ServerState::WaitingPlayers => 1,
        ServerState::GameStarted => 2,
        ServerState::ShuttingDown => 3,
    }
}

/// `UpdateMasterServer` on a thread. Messages come back to the server thread as events.
fn update_master_server(url: String, data: String, events: Sender<Event>, initial: Arc<AtomicBool>, busy: Arc<AtomicBool>) {
    busy.store(true, Ordering::Relaxed);
    std::thread::spawn(move || {
        let messages = match rustora_net::web::post(&url, &data) {
            Ok(text) if initial.swap(false, Ordering::Relaxed) => {
                log::info!(target: "server", "Master server: {text}");
                parse_master_response(&text)
            }
            Ok(_) => Vec::new(),
            Err(e) => {
                log::info!(target: "server", "Master server: {e}");
                vec![ERROR.to_string()]
            }
        };
        for m in messages {
            let _ = events.send(Event::MasterServerMessage(m));
        }
        busy.store(false, Ordering::Relaxed);
    });
}

impl Server {
    /// `GameServer(Server)`.
    fn game_server(&self) -> GameServer {
        let manifest = &self.game_mod.data.manifest;
        let lobby = &self.lobby;
        let open_slots = lobby.slots.values().filter(|s| !s.closed).count() as i32;
        let bots = lobby.clients.iter().filter(|c| c.is_bot()).count() as i32;
        GameServer {
            name: Some(self.settings.name.clone()),
            address: Some(format!("0.0.0.0:{}", self.settings.listen_port)),
            state: state_number(self.state),
            max_players: open_slots - bots,
            map: Some(self.map.uid.clone()),
            mod_id: Some(manifest.id.clone()),
            version: Some(manifest.metadata.version.clone()),
            mod_title: Some(self.game_mod.message(&manifest.metadata.title)),
            mod_website: Some(manifest.metadata.website.clone()),
            mod_icon32: Some(manifest.metadata.web_icon32.clone()),
            protected: !self.settings.password.is_empty(),
            authentication: self.authentication_required(),
            clients: lobby.clients.iter().map(GameClient::from_client).collect(),
            disabled_spawn_points: lobby.disabled_spawn_points.iter().copied().collect(),
            ..Default::default()
        }
    }

    /// `LobbyInfoSynced` and `GameStarted` of the pinger.
    pub(crate) fn master_server_changed(&mut self) {
        self.master_server.last_changed = self.run_time();
    }

    fn post_to_master_server(&self, data: String) {
        update_master_server(
            self.settings.server_advertise_url.clone(),
            data,
            self.events_sender(),
            self.master_server.is_initial_ping.clone(),
            self.master_server.is_busy.clone(),
        );
    }

    /// `MasterServerPinger.Tick`.
    pub(crate) fn master_server_tick(&mut self) {
        let now = self.run_time();
        if now.saturating_sub(self.master_server.last_changed) > MASTER_PING_INTERVAL {
            self.master_server.last_changed = now;
        }
        let pinger = &self.master_server;
        let changed = pinger.last_changed > pinger.last_ping && now.saturating_sub(pinger.last_ping) > RATE_LIMIT_INTERVAL;
        let first = pinger.is_initial_ping.load(Ordering::Relaxed);
        if pinger.is_busy.load(Ordering::Relaxed) || !(changed || first) {
            return;
        }
        let game = self.game_server();
        if self.settings.advertise_online {
            self.post_to_master_server(game.to_post_data(None));
        } else {
            self.master_server.is_initial_ping.store(false, Ordering::Relaxed);
        }
        if let Some(beacon) = &self.master_server.beacon {
            beacon.set_data(&game.to_post_data(Some(&self.master_server.session_guid)));
        }
        self.master_server.irc.pending = true;
        self.master_server.last_ping = now;
    }

    /// The nick of attempt `attempt`: `ora_srv_` and 8 hex digits.
    fn irc_config(seed: u32) -> IrcConfig {
        IrcConfig {
            channels: vec![irc::SERVERS_CHANNEL.to_string()],
            nick: Box::new(move |attempt| {
                let nick = format!(
                    "{IRC_NICK_PREFIX}{:08x}",
                    seed.wrapping_add(attempt - 1).wrapping_mul(2_654_435_761)
                );
                nick.chars().take(NICK_LEN).collect()
            }),
            real_name: "rustora game server".into(),
            ipv4_only: true,
        }
    }

    /// Keeps the IRC connection up while the game is advertised online, answers queries, and sends the pending
    /// advertisement once the rate limit allows.
    pub(crate) fn irc_tick(&mut self) {
        if !self.settings.advertise_online || !self.master_server.is_multiplayer {
            return;
        }
        let now = self.run_time();
        let seed = self.master_server.nick_seed;
        let advertiser = &mut self.master_server.irc;
        if advertiser.client.is_none() && now >= advertiser.retry_at {
            advertiser.client = Some(IrcClient::connect(Self::irc_config(seed)));
        }
        for event in advertiser.client.as_ref().map(IrcClient::poll).unwrap_or_default() {
            match event {
                IrcEvent::Joined { .. } => {
                    log::info!(target: "server", "Advertising the game in {}", irc::SERVERS_CHANNEL);
                    advertiser.joined = true;
                    advertiser.pending = true;
                }
                IrcEvent::Message {
                    channel: Some(_), text, ..
                } if irc_games::is_query(&text, now_millis()) => {
                    advertiser.pending = true;
                }
                IrcEvent::Disconnected(reason) => {
                    log::info!(target: "server", "IRC: {reason}");
                    advertiser.client = None;
                    advertiser.joined = false;
                    advertiser.retry_at = now + IRC_RETRY_DELAY;
                }
                IrcEvent::Message { .. } | IrcEvent::Users { .. } => {}
            }
        }
        let allowed = advertiser.last_sent.is_none_or(|t| now.saturating_sub(t) >= IRC_RATE_LIMIT);
        if advertiser.joined && advertiser.pending && allowed {
            advertiser.pending = false;
            advertiser.last_sent = Some(now);
            self.send_irc_advertisement();
        }
    }

    fn send_irc_advertisement(&self) {
        let pinger = &self.master_server;
        if let (Some(client), true) = (&pinger.irc.client, pinger.irc.joined) {
            let line = irc_games::advertisement(&self.game_server(), &pinger.session_guid, now_millis());
            client.send(irc::SERVERS_CHANNEL, &line);
        }
    }

    /// `ServerShutdown` of the pinger: removes the game from the lists.
    pub(crate) fn master_server_shutdown(&mut self) {
        if self.settings.advertise_online {
            self.post_to_master_server(self.game_server().to_post_data(None));
        }
        self.send_irc_advertisement();
        self.master_server.irc = IrcAdvertiser::default();
        if let Some(beacon) = self.master_server.beacon.take() {
            beacon.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_responses() {
        assert_eq!(parse_master_response(""), [CONNECTED]);
        assert_eq!(parse_master_response("[0]fine"), [CONNECTED]);
        assert_eq!(parse_master_response("[1]x"), [CONNECTED, NO_PORT_FORWARD, GAME_OFFLINE]);
        assert_eq!(parse_master_response("[-5]warning text"), [CONNECTED, "warning text"]);
        assert_eq!(parse_master_response("no code"), [CONNECTED]);
    }
}
