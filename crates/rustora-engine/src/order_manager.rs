// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Network/OrderManager.cs`, `Network/UnitOrders.cs` (world orders) and the replay connection.

use std::rc::Rc;

use rustora_net::order::{OrderPacket, Packet, RawOrder};
use rustora_net::replay::Chunk;

use crate::net_collections::NetMap;
use crate::order::Order;
use crate::player::WinState;
use crate::world::World;

/// Something received from a connection.
pub enum Incoming {
    Sync { frame: i32, sync_hash: i32, defeat_state: u64 },
    Disconnect { client_id: i32, frame: i32 },
    Immediate { client_id: i32, orders: OrderPacket },
    Orders { client_id: i32, orders: OrderPacket },
}

/// `IConnection`.
pub trait Connection {
    fn local_client_id(&self) -> i32;
    fn start_game(&mut self) {}
    fn send(&mut self, _frame: i32, _orders: Vec<RawOrder>) {}
    fn send_immediate(&mut self, _orders: Vec<RawOrder>) {}
    fn send_sync(&mut self, frame: i32, sync_hash: i32, defeat_state: u64);
    fn receive(&mut self, net_frame_number: i32) -> Vec<Incoming>;
}

/// `ReplayConnection`: feeds recorded packets with the original order latency.
pub struct ReplayConnection {
    chunks: std::collections::VecDeque<Chunk>,
    sync: std::collections::VecDeque<(i32, i32, u64)>,
    order_latency: i32,
}

impl ReplayConnection {
    pub fn new(chunks: Vec<Chunk>, order_latency: i32) -> Self {
        ReplayConnection {
            chunks: chunks.into(),
            sync: Default::default(),
            order_latency,
        }
    }
}

impl Connection for ReplayConnection {
    fn local_client_id(&self) -> i32 {
        -1
    }

    fn send_sync(&mut self, frame: i32, sync_hash: i32, defeat_state: u64) {
        self.sync.push_back((frame, sync_hash, defeat_state));
    }

    fn receive(&mut self, net_frame_number: i32) -> Vec<Incoming> {
        let mut out = Vec::new();
        while let Some((frame, sync_hash, defeat_state)) = self.sync.pop_front() {
            out.push(Incoming::Sync {
                frame,
                sync_hash,
                defeat_state,
            });
        }
        while self
            .chunks
            .front()
            .is_some_and(|c| c.frame <= net_frame_number + self.order_latency)
        {
            let chunk = self.chunks.pop_front().expect("chunk");
            for p in chunk.packets {
                match Packet::parse(p.client_id, &p.data) {
                    Ok(Packet::Disconnect { frame, client_id }) => out.push(Incoming::Disconnect { client_id, frame }),
                    Ok(Packet::Sync {
                        frame,
                        sync_hash,
                        defeat_state,
                    }) => out.push(Incoming::Sync {
                        frame,
                        sync_hash,
                        defeat_state,
                    }),
                    Ok(Packet::Orders(orders)) => {
                        if orders.frame == 0 {
                            out.push(Incoming::Immediate {
                                client_id: p.client_id,
                                orders,
                            });
                        } else {
                            out.push(Incoming::Orders {
                                client_id: p.client_id,
                                orders,
                            });
                        }
                    }
                    Ok(_) => {}
                    Err(e) => panic!("Received unknown packet from client {}: {e}", p.client_id),
                }
            }
        }
        out
    }
}

pub struct OrderManager {
    pub world: Rc<World>,
    pub connection: Box<dyn Connection>,
    pending_orders: NetMap<i32, std::collections::VecDeque<(i32, Option<OrderPacket>)>>,
    sync_for_frame: std::collections::HashMap<i32, (i32, u64)>,
    pub net_frame_number: i32,
    pub local_frame_number: i32,
    sent_orders_frame: i32,
    local_orders: Vec<RawOrder>,
    local_immediate_orders: Vec<RawOrder>,
    net_frame_interval: i32,
    /// First frame whose synced state diverged, if any.
    pub out_of_sync: Option<i32>,
}

impl OrderManager {
    pub fn new(world: Rc<World>, connection: Box<dyn Connection>) -> Self {
        let net_frame_interval = world.lobby_info.global_settings.net_frame_interval;
        OrderManager {
            world,
            connection,
            pending_orders: NetMap::new(),
            sync_for_frame: Default::default(),
            net_frame_number: 0,
            local_frame_number: 0,
            sent_orders_frame: 0,
            local_orders: Vec::new(),
            local_immediate_orders: Vec::new(),
            net_frame_interval,
            out_of_sync: None,
        }
    }

    pub fn game_started(&self) -> bool {
        self.net_frame_number != 0
    }

    pub fn start_game(&mut self) {
        if self.game_started() {
            return;
        }
        for c in &self.world.lobby_info.clients {
            if !c.is_bot() {
                self.pending_orders.insert(c.index, Default::default());
            }
        }
        self.net_frame_number = 1;
        self.local_frame_number = 0;
        self.connection.start_game();
    }

    pub fn issue_order(&mut self, order: RawOrder) {
        if order.is_immediate {
            self.local_immediate_orders.push(order);
        } else {
            self.local_orders.push(order);
        }
    }

    fn receive_all_orders_and_check_sync(&mut self) {
        for inc in self.connection.receive(self.net_frame_number) {
            match inc {
                Incoming::Sync {
                    frame,
                    sync_hash,
                    defeat_state,
                } => match self.sync_for_frame.get(&frame) {
                    Some(&(h, d)) => {
                        if (h != sync_hash || d != defeat_state) && self.out_of_sync.is_none() {
                            self.out_of_sync = Some(frame);
                        }
                    }
                    None => {
                        self.sync_for_frame.insert(frame, (sync_hash, defeat_state));
                    }
                },
                Incoming::Disconnect { client_id, frame } => {
                    if self.game_started() {
                        self.enqueue(client_id, frame, None);
                    }
                }
                Incoming::Immediate { client_id, orders } => {
                    for raw in &orders.orders {
                        self.process_order(client_id, raw);
                    }
                }
                Incoming::Orders { client_id, orders } => {
                    self.enqueue(client_id, orders.frame, Some(orders));
                }
            }
        }
    }

    fn enqueue(&mut self, client_id: i32, frame: i32, orders: Option<OrderPacket>) {
        self.pending_orders
            .get_mut(&client_id)
            .unwrap_or_else(|| panic!("Received packet from disconnected client '{client_id}'"))
            .push_back((frame, orders));
    }

    fn is_ready_for_next_frame(&self) -> bool {
        self.game_started() && self.pending_orders.values().all(|q| !q.is_empty())
    }

    fn process_orders(&mut self) {
        let clients: Vec<i32> = self.pending_orders.keys().copied().collect();
        let mut to_remove = Vec::new();
        for client_id in clients {
            let (frame_number, orders) = self.pending_orders.get_mut(&client_id).unwrap().pop_front().expect("ready");
            assert!(
                frame_number == self.net_frame_number,
                "Attempted to process orders from client {client_id} for frame {frame_number} on frame {}",
                self.net_frame_number
            );
            let Some(orders) = orders else {
                to_remove.push(client_id);
                self.world.on_client_disconnected(client_id);
                continue;
            };
            for raw in &orders.orders {
                self.process_order(client_id, raw);
            }
        }
        for c in to_remove {
            self.pending_orders.remove(&c);
        }
        let mut defeat = 0u64;
        for (i, p) in self.world.players().iter().enumerate() {
            if p.win_state.get() == WinState::Lost {
                defeat |= 1u64 << i;
            }
        }
        let hash = self.world.sync_hash();
        self.connection.send_sync(self.net_frame_number, hash, defeat);
        self.net_frame_number += 1;
    }

    /// `UnitOrders.ProcessOrder`: world orders and pause; other orders concern the client UI.
    fn process_order(&mut self, client_id: i32, raw: &RawOrder) {
        match raw.order_string.as_str() {
            "Message"
            | "FluentMessage"
            | "DisableChatEntry"
            | "StartKickVote"
            | "EndKickVote"
            | "Chat"
            | "TeamChat"
            | "StartGame"
            | "HandshakeRequest"
            | "ServerError"
            | "AuthenticationError"
            | "SyncInfo"
            | "SyncLobbyClients"
            | "SyncLobbySlots"
            | "SyncLobbyGlobal"
            | "SyncClientPings"
            | "SyncConnectionQuality"
            | "SyncMapPool"
            | "GameSaved"
            | "Kick" => {}
            "PauseGame" => {
                if self.world.lobby_info.client_with_index(client_id).is_some() {
                    let pause = raw.target_string.as_deref() == Some("Pause");
                    if !(self.world.is_game_over() && !pause) {
                        self.world.set_paused(pause);
                    }
                }
            }
            _ => {
                let Some(order) = Order::resolve(&self.world, raw) else { return };
                match &order.grouped_actors {
                    None => self.resolve_order(client_id, &order),
                    Some(group) => {
                        for subject in group.clone() {
                            self.resolve_order(client_id, &order.from_grouped(subject));
                        }
                    }
                }
            }
        }
    }

    fn resolve_order(&self, client_id: i32, order: &Order) {
        let Some(subject) = &order.subject else { return };
        if subject.is_dead() {
            return;
        }
        if self
            .world
            .order_validators()
            .iter()
            .all(|v| v.order_validation(&self.world, client_id, order))
        {
            subject.resolve_order(order);
        }
    }

    pub fn tick_immediate(&mut self) {
        if !self.local_immediate_orders.is_empty() {
            let o = std::mem::take(&mut self.local_immediate_orders);
            self.connection.send_immediate(o);
        }
        self.receive_all_orders_and_check_sync();
    }

    fn is_net_frame(&self) -> bool {
        self.local_frame_number % self.net_frame_interval == 0
    }

    pub fn try_tick(&mut self) -> bool {
        let mut should_tick = true;
        if self.is_net_frame() {
            let local = self.connection.local_client_id();
            should_tick = self.pending_orders.iter().all(|(k, q)| *k == local || !q.is_empty());
            if should_tick && self.game_started() && self.sent_orders_frame < self.net_frame_number {
                let o = std::mem::take(&mut self.local_orders);
                self.connection.send(self.net_frame_number, o);
                self.sent_orders_frame = self.net_frame_number;
            }
        }
        let mut will_tick = should_tick;
        if will_tick && self.is_net_frame() {
            will_tick = self.is_ready_for_next_frame();
            if will_tick {
                self.process_orders();
            }
        }
        if will_tick {
            self.local_frame_number += 1;
        }
        will_tick
    }
}
