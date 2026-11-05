// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! A skirmish hosted by rustora itself: the lobby a local C# server builds for a spectating host and bots
//! (`LobbyCommands` slot_bot, faction and spawn), and a local connection with the singleplayer order latency.

use std::collections::VecDeque;
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_engine::order_manager::{Connection, Incoming, OrderManager};
use rustora_engine::world::World;
use rustora_game::map::MapPackage;
use rustora_mods_common::world::{PlayerReference, make_slot, map_players, sync_client_to_player_reference};
use rustora_net::order::{OrderPacket, RawOrder};
use rustora_net::session::{Client, ClientState, Global, Session, Slot};

use crate::Step;

/// One bot of `Slot:BotType[:Faction[:Spawn]]`, the format of the C# `Harness.Bots` setting.
#[derive(Clone, Debug)]
pub struct BotSpec {
    pub slot: String,
    pub bot: String,
    pub faction: Option<String>,
    pub spawn: Option<i32>,
}

/// Parses a comma separated list of bot specs.
pub fn parse_bots(s: &str) -> Result<Vec<BotSpec>, String> {
    s.split(',')
        .map(|b| {
            let p: Vec<&str> = b.split(':').collect();
            if p.len() < 2 {
                return Err(format!("invalid bot spec {b}"));
            }
            Ok(BotSpec {
                slot: p[0].to_string(),
                bot: p[1].to_string(),
                faction: p.get(2).map(|f| f.to_string()),
                spawn: p
                    .get(3)
                    .map(|s| s.parse().map_err(|_| format!("invalid spawn in {b}")))
                    .transpose()?,
            })
        })
        .collect()
}

/// The lobby at game start of a local server with a spectating host (client 0) and `bots`.
/// Lobby options stay unset, so every option takes the default of its trait like on a fresh C# server.
/// Bot colors are not random like on the C# server; they do not affect the simulation.
pub fn skirmish_session(map: &MapPackage, bots: &[BotSpec], seed: i32) -> Result<Session, String> {
    let players: IndexMap<String, PlayerReference> = map_players(map.section_nodes("Players"));
    let slots: IndexMap<String, Slot> = players
        .values()
        .filter_map(make_slot)
        .map(|s| (s.player_reference.clone(), s))
        .collect();
    let mut clients = vec![Client {
        index: 0,
        faction: Some("Random".into()),
        name: Some("Commander".into()),
        state: ClientState::Invalid,
        is_admin: true,
        ..Default::default()
    }];
    for (i, b) in bots.iter().enumerate() {
        let pr = players.get(&b.slot).ok_or_else(|| format!("unknown slot {}", b.slot))?;
        if !slots.get(&b.slot).is_some_and(|s| s.allow_bots) {
            return Err(format!("slot {} does not allow bots", b.slot));
        }
        let mut c = Client {
            index: i as i32 + 1,
            name: Some(format!("bot-{}-ai.name", b.bot)),
            bot: Some(b.bot.clone()),
            slot: Some(b.slot.clone()),
            faction: Some("Random".into()),
            state: ClientState::NotReady,
            bot_controller_client_index: 0,
            ..Default::default()
        };
        sync_client_to_player_reference(&mut c, pr);
        if let Some(f) = &b.faction {
            c.faction = Some(f.clone());
        }
        if let Some(s) = b.spawn {
            c.spawn_point = s;
        }
        clients.push(c);
    }
    Ok(Session {
        clients,
        slots,
        disabled_spawn_points: Default::default(),
        global_settings: Global {
            server_name: Some("Skirmish Game".into()),
            map: Some(map.uid.clone()),
            random_seed: seed,
            enable_singleplayer: true,
            ..Default::default()
        },
    })
}

/// The singleplayer server's order latency (`Server.OrderLatency` is only raised for multiplayer games).
const LOCAL_ORDER_LATENCY: i32 = 1;

/// The local client's view of a local server: its orders come back `LOCAL_ORDER_LATENCY` frames later.
#[derive(Default)]
pub struct LocalConnection {
    incoming: VecDeque<Incoming>,
}

impl Connection for LocalConnection {
    fn local_client_id(&self) -> i32 {
        0
    }

    /// `Server.StartGame` injects an empty packet for each frame skipped by the order latency.
    fn start_game(&mut self) {
        for i in 0..LOCAL_ORDER_LATENCY {
            self.incoming.push_back(Incoming::Orders {
                client_id: 0,
                orders: OrderPacket {
                    frame: 1 + i,
                    orders: Vec::new(),
                },
            });
        }
    }

    fn send(&mut self, frame: i32, orders: Vec<RawOrder>) {
        self.incoming.push_back(Incoming::Orders {
            client_id: 0,
            orders: OrderPacket {
                frame: frame + LOCAL_ORDER_LATENCY,
                orders,
            },
        });
    }

    fn send_sync(&mut self, _frame: i32, _sync_hash: i32, _defeat_state: u64) {}

    fn receive(&mut self, _net_frame_number: i32) -> Vec<Incoming> {
        self.incoming.drain(..).collect()
    }
}

/// Runs a skirmish: activates the bots and feeds the orders they issue back through the local connection.
/// Game over requests are served after their delay in client frames, which also pass while another client's game over
/// has paused the world. The C# client measures the delay on its clock, whose timestep the server scales by its real
/// time order timing, so C# ends the game a few ticks earlier or later.
pub struct SkirmishDriver {
    pub orders: OrderManager,
    world: Rc<World>,
    end_game_frame: Option<i32>,
}

impl SkirmishDriver {
    pub fn new(world: &Rc<World>) -> SkirmishDriver {
        SkirmishDriver::with_connection(world, Box::new(LocalConnection::default()), true)
    }

    /// Drives the game over `connection`. Only the host runs the bots (`Game.IsHost` in the C# `Player`).
    pub fn with_connection(world: &Rc<World>, connection: Box<dyn Connection>, is_host: bool) -> SkirmishDriver {
        if is_host {
            rustora_mods_common::bots::activate_bots(world);
        }
        let mut orders = OrderManager::new(world.clone(), connection);
        orders.start_game();
        SkirmishDriver {
            orders,
            world: world.clone(),
            end_game_frame: None,
        }
    }

    /// Advances the world by one tick if the orders for it are available.
    pub fn step(&mut self) -> Step {
        self.orders.tick_immediate();
        if self.world.is_game_over() {
            return Step::Ended;
        }
        if !self.orders.try_tick() {
            return if self.orders.connection.is_connected() {
                Step::Waiting
            } else {
                Step::Ended
            };
        }
        self.world.tick();
        let frame = self.orders.local_frame_number;
        for delay in self.world.take_end_game_requests() {
            let due = frame + (delay + self.world.timestep - 1) / self.world.timestep;
            self.end_game_frame = Some(self.end_game_frame.map_or(due, |t| t.min(due)));
        }
        if self.end_game_frame.is_some_and(|t| frame >= t) {
            self.orders.issue_order(RawOrder::from_target_string("PauseGame", "Pause", false));
            self.world.end_game();
        }
        for (_, o) in self.world.take_issued_orders() {
            self.orders.issue_order(o.to_raw());
        }
        Step::Ticked
    }
}
