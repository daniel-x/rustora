// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Serialized orders (`Order.Serialize`/`Deserialize`) and order packets (`OrderIO`).
//!
//! Orders are kept in their wire form with actor IDs; the engine resolves IDs to actors when it
//! processes them, like `Order.Deserialize(world, ...)` does lazily in C#.

use rustora_core::world_types::{CPos, WPos};

use crate::NetError;
use crate::binary::{Reader, Writer};

pub mod order_type {
    pub const ACK: u8 = 0x10;
    pub const PING: u8 = 0x20;
    pub const SYNC_HASH: u8 = 0x65;
    pub const TICK_SCALE: u8 = 0x76;
    pub const DISCONNECT: u8 = 0xBF;
    pub const HANDSHAKE: u8 = 0xFE;
    pub const FIELDS: u8 = 0xFF;
}

mod fields {
    pub const TARGET: i16 = 0x01;
    pub const EXTRA_ACTORS: i16 = 0x02;
    pub const TARGET_STRING: i16 = 0x04;
    pub const QUEUED: i16 = 0x08;
    pub const EXTRA_LOCATION: i16 = 0x10;
    pub const EXTRA_DATA: i16 = 0x20;
    pub const TARGET_IS_CELL: i16 = 0x40;
    pub const SUBJECT: i16 = 0x80;
    pub const GROUPED: i16 = 0x100;
}

pub const SYNC_HASH_ORDER_LENGTH: usize = 13;
pub const DISCONNECT_ORDER_LENGTH: usize = 5;
pub const NO_ACTOR: u32 = u32::MAX;

/// Target in wire form (`TargetType` 0 = Invalid, 1 = Actor, 2 = Terrain, 3 = FrozenActor).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum RawTarget {
    #[default]
    Invalid,
    Actor {
        id: u32,
        generation: i32,
    },
    FrozenActor {
        player_actor_id: u32,
        frozen_id: u32,
    },
    Cell {
        cell: CPos,
        subcell: u8,
    },
    Pos {
        pos: WPos,
        terrain_positions: Option<Vec<WPos>>,
    },
}

/// An order as it travels over the network.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RawOrder {
    pub ty: u8,
    pub order_string: String,
    pub subject: Option<u32>,
    pub target: RawTarget,
    pub target_string: Option<String>,
    pub queued: bool,
    pub extra_actors: Option<Vec<u32>>,
    pub extra_location: CPos,
    pub extra_data: u32,
    pub grouped_actors: Option<Vec<u32>>,
    /// Set locally for immediate orders; not serialized.
    pub is_immediate: bool,
}

impl RawOrder {
    pub fn new(order_string: &str) -> Self {
        RawOrder {
            ty: order_type::FIELDS,
            order_string: order_string.to_string(),
            ..Default::default()
        }
    }

    pub fn command(text: &str) -> Self {
        RawOrder {
            target_string: Some(text.to_string()),
            is_immediate: true,
            ..RawOrder::new("Command")
        }
    }

    /// `Order.FromTargetString`.
    pub fn from_target_string(order_string: &str, target_string: &str, is_immediate: bool) -> Self {
        RawOrder {
            target_string: Some(target_string.to_string()),
            is_immediate,
            ..RawOrder::new(order_string)
        }
    }

    pub fn handshake(name: &str, data: &str) -> Self {
        RawOrder {
            ty: order_type::HANDSHAKE,
            order_string: name.to_string(),
            target_string: Some(data.to_string()),
            ..Default::default()
        }
    }

    /// Returns `None` for unknown order types (C# logs and skips them).
    pub fn deserialize(r: &mut Reader<'_>) -> Result<Option<RawOrder>, NetError> {
        let ty = r.u8()?;
        match ty {
            order_type::FIELDS => {
                let order_string = r.string()?;
                let f = r.i16()?;
                let has = |b: i16| f & b != 0;
                let subject = if has(fields::SUBJECT) { Some(r.u32()?) } else { None };
                let mut target = RawTarget::Invalid;
                if has(fields::TARGET) {
                    match r.u8()? {
                        1 => {
                            target = RawTarget::Actor {
                                id: r.u32()?,
                                generation: r.i32()?,
                            }
                        }
                        3 => {
                            target = RawTarget::FrozenActor {
                                player_actor_id: r.u32()?,
                                frozen_id: r.u32()?,
                            }
                        }
                        2 => {
                            if has(fields::TARGET_IS_CELL) {
                                target = RawTarget::Cell {
                                    cell: CPos::from_bits(r.i32()?),
                                    subcell: r.u8()?,
                                };
                            } else {
                                let pos = WPos::new(r.i32()?, r.i32()?, r.i32()?);
                                let n = r.i16()?;
                                let terrain_positions = if n == -1 {
                                    None
                                } else {
                                    let mut v = Vec::with_capacity(n.max(0) as usize);
                                    for _ in 0..n {
                                        v.push(WPos::new(r.i32()?, r.i32()?, r.i32()?));
                                    }
                                    Some(v)
                                };
                                target = RawTarget::Pos { pos, terrain_positions };
                            }
                        }
                        _ => {}
                    }
                }
                let target_string = if has(fields::TARGET_STRING) { Some(r.string()?) } else { None };
                let queued = has(fields::QUEUED);
                let read_ids = |r: &mut Reader<'_>| -> Result<Vec<u32>, NetError> {
                    let n = r.i32()?;
                    (0..n.max(0)).map(|_| r.u32()).collect()
                };
                let extra_actors = if has(fields::EXTRA_ACTORS) { Some(read_ids(r)?) } else { None };
                let extra_location = if has(fields::EXTRA_LOCATION) {
                    CPos::from_bits(r.i32()?)
                } else {
                    CPos::ZERO
                };
                let extra_data = if has(fields::EXTRA_DATA) { r.u32()? } else { 0 };
                let grouped_actors = if has(fields::GROUPED) { Some(read_ids(r)?) } else { None };
                Ok(Some(RawOrder {
                    ty,
                    order_string,
                    subject,
                    target,
                    target_string,
                    queued,
                    extra_actors,
                    extra_location,
                    extra_data,
                    grouped_actors,
                    is_immediate: false,
                }))
            }
            order_type::HANDSHAKE => {
                let order_string = r.string()?;
                let target_string = r.string()?;
                Ok(Some(RawOrder {
                    ty,
                    order_string,
                    target_string: Some(target_string),
                    ..Default::default()
                }))
            }
            _ => Ok(None),
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.u8(self.ty);
        w.string(&self.order_string);
        if self.ty == order_type::HANDSHAKE {
            w.string(self.target_string.as_deref().unwrap_or(""));
            return w.data;
        }

        let mut f: i16 = 0;
        if self.subject.is_some() {
            f |= fields::SUBJECT;
        }
        if self.target_string.is_some() {
            f |= fields::TARGET_STRING;
        }
        if self.extra_data != 0 {
            f |= fields::EXTRA_DATA;
        }
        if self.target != RawTarget::Invalid {
            f |= fields::TARGET;
        }
        if self.queued {
            f |= fields::QUEUED;
        }
        if self.grouped_actors.is_some() {
            f |= fields::GROUPED;
        }
        if self.extra_actors.is_some() {
            f |= fields::EXTRA_ACTORS;
        }
        if self.extra_location != CPos::ZERO {
            f |= fields::EXTRA_LOCATION;
        }
        if matches!(self.target, RawTarget::Cell { .. }) {
            f |= fields::TARGET_IS_CELL;
        }
        w.i16(f);
        if let Some(s) = self.subject {
            w.u32(s);
        }
        match &self.target {
            RawTarget::Invalid => {}
            RawTarget::Actor { id, generation } => {
                w.u8(1);
                w.u32(*id);
                w.i32(*generation);
            }
            RawTarget::FrozenActor {
                player_actor_id,
                frozen_id,
            } => {
                w.u8(3);
                w.u32(*player_actor_id);
                w.u32(*frozen_id);
            }
            RawTarget::Cell { cell, subcell } => {
                w.u8(2);
                w.i32(cell.bits);
                w.u8(*subcell);
            }
            RawTarget::Pos { pos, terrain_positions } => {
                w.u8(2);
                w.i32(pos.x);
                w.i32(pos.y);
                w.i32(pos.z);
                match terrain_positions {
                    None => w.i16(-1),
                    Some(tp) => {
                        w.i16(tp.len() as i16);
                        for p in tp {
                            w.i32(p.x);
                            w.i32(p.y);
                            w.i32(p.z);
                        }
                    }
                }
            }
        }
        if let Some(s) = &self.target_string {
            w.string(s);
        }
        if let Some(a) = &self.extra_actors {
            w.i32(a.len() as i32);
            for id in a {
                w.u32(*id);
            }
        }
        if self.extra_location != CPos::ZERO {
            w.i32(self.extra_location.bits);
        }
        if self.extra_data != 0 {
            w.u32(self.extra_data);
        }
        if let Some(a) = &self.grouped_actors {
            w.i32(a.len() as i32);
            for id in a {
                w.u32(*id);
            }
        }
        w.data
    }
}

/// A packet of orders for one net frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrderPacket {
    pub frame: i32,
    pub orders: Vec<RawOrder>,
}

/// Packet contents as classified by `OrderIO.TryParse*`.
#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Orders(OrderPacket),
    Sync { frame: i32, sync_hash: i32, defeat_state: u64 },
    Disconnect { frame: i32, client_id: i32 },
    Ack { frame: i32, count: u8 },
    Ping { timestamp: i64 },
    TickScale(f32),
}

impl Packet {
    /// Parses a packet from `from_client` (0 is the server).
    pub fn parse(from_client: i32, data: &[u8]) -> Result<Packet, NetError> {
        if data.len() < 4 {
            return Err(NetError::Invalid("packet too short".into()));
        }
        let frame = i32::from_le_bytes(data[0..4].try_into().unwrap());
        let ty = data.get(4).copied();
        let mut r = Reader::new(&data[5.min(data.len())..]);
        match ty {
            Some(order_type::DISCONNECT) if from_client == 0 && data.len() == DISCONNECT_ORDER_LENGTH + 4 => {
                return Ok(Packet::Disconnect {
                    frame,
                    client_id: r.i32()?,
                });
            }
            Some(order_type::SYNC_HASH) if data.len() == 4 + SYNC_HASH_ORDER_LENGTH => {
                return Ok(Packet::Sync {
                    frame,
                    sync_hash: r.i32()?,
                    defeat_state: r.u64()?,
                });
            }
            Some(order_type::ACK) if from_client == 0 && data.len() == 6 => return Ok(Packet::Ack { frame, count: data[5] }),
            Some(order_type::PING) if from_client == 0 && data.len() == 13 && frame == 0 => {
                return Ok(Packet::Ping { timestamp: r.i64()? });
            }
            Some(order_type::TICK_SCALE) if from_client == 0 && data.len() == 9 && frame == 0 => return Ok(Packet::TickScale(r.f32()?)),
            Some(order_type::DISCONNECT) | Some(order_type::SYNC_HASH) => return Err(NetError::Invalid("malformed packet".into())),
            _ => {}
        }
        let mut r = Reader::new(&data[4..]);
        let mut orders = Vec::new();
        while r.remaining() > 0 {
            if let Some(o) = RawOrder::deserialize(&mut r)? {
                orders.push(o);
            }
        }
        Ok(Packet::Orders(OrderPacket { frame, orders }))
    }

    pub fn serialize_orders(frame: i32, orders: &[RawOrder]) -> Vec<u8> {
        let mut out = frame.to_le_bytes().to_vec();
        for o in orders {
            out.extend(o.serialize());
        }
        out
    }

    pub fn serialize_sync(frame: i32, sync_hash: i32, defeat_state: u64) -> Vec<u8> {
        let mut w = Writer::default();
        w.i32(frame);
        w.u8(order_type::SYNC_HASH);
        w.i32(sync_hash);
        w.u64(defeat_state);
        w.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_round_trip() {
        let o = RawOrder {
            subject: Some(5),
            target: RawTarget::Cell {
                cell: CPos::new(3, 4),
                subcell: 0,
            },
            target_string: Some("e1".into()),
            queued: true,
            extra_data: 7,
            ..RawOrder::new("Move")
        };
        let data = Packet::serialize_orders(10, std::slice::from_ref(&o));
        let Packet::Orders(p) = Packet::parse(1, &data).unwrap() else {
            panic!()
        };
        assert_eq!(p.frame, 10);
        assert_eq!(p.orders, vec![o]);
    }
}
