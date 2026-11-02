// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `.orarep` replay files: recorded packets followed by a metadata block.

use crate::NetError;
use crate::binary::Reader;
use crate::order::{OrderPacket, Packet};
use crate::session::Session;

pub const META_START_MARKER: i32 = -1;
pub const META_END_MARKER: i32 = -2;
pub const META_VERSION: i32 = 1;

/// One recorded packet.
#[derive(Clone, Debug)]
pub struct RecordedPacket {
    pub client_id: i32,
    pub data: Vec<u8>,
}

impl RecordedPacket {
    pub fn frame(&self) -> i32 {
        i32::from_le_bytes(self.data[0..4].try_into().unwrap())
    }
}

/// Packets of one chunk: everything recorded up to and including the first packet of `frame`.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub frame: i32,
    pub packets: Vec<RecordedPacket>,
}

/// A parsed replay, structured like `ReplayConnection`.
pub struct Replay {
    pub chunks: Vec<Chunk>,
    pub lobby_info: Option<Session>,
    pub is_valid: bool,
    pub tick_count: i32,
    /// The `GameInformation` yaml of the metadata block, if present.
    pub metadata: Option<String>,
}

impl Replay {
    pub fn parse(data: &[u8]) -> Result<Replay, NetError> {
        let mut r = Reader::new(data);
        let mut packets = Vec::new();
        let mut chunks = Vec::new();
        let mut lobby_info = None;
        let mut is_valid = false;
        let mut tick_count = 0;
        while r.remaining() > 0 {
            let client_id = r.i32()?;
            if client_id == META_START_MARKER {
                break;
            }
            let len = r.i32()? as usize;
            let packet = RecordedPacket {
                client_id,
                data: r.bytes(len)?.to_vec(),
            };
            let frame = packet.frame();
            let ty = packet.data.get(4).copied();
            packets.push(packet);
            if packets.last().unwrap().data.len() > 4
                && matches!(
                    ty,
                    Some(crate::order::order_type::DISCONNECT) | Some(crate::order::order_type::SYNC_HASH)
                )
            {
                continue;
            }
            if frame == 0 {
                if let Ok(Packet::Orders(OrderPacket { orders, .. })) = Packet::parse(client_id, &packets.last().unwrap().data) {
                    for o in orders {
                        if o.order_string == "StartGame" {
                            is_valid = true;
                        } else if o.order_string == "SyncInfo" && !is_valid {
                            lobby_info = Some(Session::deserialize(o.target_string.as_deref().unwrap_or(""), "SyncInfo")?);
                        }
                    }
                }
            } else {
                chunks.push(Chunk {
                    frame,
                    packets: std::mem::take(&mut packets),
                });
                tick_count = tick_count.max(frame);
            }
        }
        Ok(Replay {
            chunks,
            lobby_info,
            is_valid,
            tick_count,
            metadata: read_metadata(data),
        })
    }
}

fn read_metadata(data: &[u8]) -> Option<String> {
    if data.len() < 20 {
        return None;
    }
    let n = data.len();
    let end = i32::from_le_bytes(data[n - 4..].try_into().ok()?);
    let len = i32::from_le_bytes(data[n - 8..n - 4].try_into().ok()?) as usize;
    if end != META_END_MARKER || len + 16 > n {
        return None;
    }
    let start = n - 8 - len - 8;
    let mut r = Reader::new(&data[start..]);
    if r.i32().ok()? != META_START_MARKER || r.i32().ok()? != META_VERSION {
        return None;
    }
    let slen = r.i32().ok()? as usize;
    Some(String::from_utf8_lossy(r.bytes(slen).ok()?).into_owned())
}
