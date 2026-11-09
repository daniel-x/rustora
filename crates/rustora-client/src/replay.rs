// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `.orarep` replay files: recorded packets followed by a metadata block.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::game_information::GameInformation;
use rustora_net::NetError;
use rustora_net::binary::Reader;
use rustora_net::order::{OrderPacket, Packet};
use rustora_net::session::Session;

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
        while r.remaining() >= 4 {
            let client_id = r.i32()?;
            if client_id == META_START_MARKER {
                break;
            }
            // A game that crashed or lost power leaves a truncated last packet; keep the packets before it.
            if r.remaining() < 4 {
                break;
            }
            let len = r.i32()? as usize;
            if len > r.remaining() {
                break;
            }
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
                    Some(rustora_net::order::order_type::DISCONNECT) | Some(rustora_net::order::order_type::SYNC_HASH)
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

/// The `GameInformation` yaml of the metadata block at the end of a replay file.
pub fn read_metadata(data: &[u8]) -> Option<String> {
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

/// `ReplayMetadata.Write`: the metadata block for the end of a replay file.
pub fn write_metadata(info: &GameInformation) -> Vec<u8> {
    let yaml = info.serialize();
    let mut out = Vec::with_capacity(yaml.len() + 20);
    out.extend_from_slice(&META_START_MARKER.to_le_bytes());
    out.extend_from_slice(&META_VERSION.to_le_bytes());
    out.extend_from_slice(&(yaml.len() as i32).to_le_bytes());
    out.extend_from_slice(yaml.as_bytes());
    out.extend_from_slice(&(yaml.len() as i32 + 4).to_le_bytes());
    out.extend_from_slice(&META_END_MARKER.to_le_bytes());
    out
}

/// `ReplayRecorder.CreateReplayFileMaxRetryCount`.
const CREATE_REPLAY_FILE_MAX_RETRY_COUNT: i32 = 128;

/// `ReplayRecorder`: packets are buffered until the game starts, then written to a new file in `directory`
/// named `filename`. Dropping it appends the metadata block, if the game information was set.
pub struct ReplayRecorder {
    directory: PathBuf,
    filename: String,
    pre_start_buffer: Option<Vec<u8>>,
    file: Option<BufWriter<File>>,
    path: Option<PathBuf>,
    /// The game information that the world keeps up to date; the metadata block is written from it.
    pub metadata: Option<Arc<Mutex<GameInformation>>>,
}

fn is_game_start(data: &[u8]) -> bool {
    matches!(Packet::parse(1, data), Ok(Packet::Orders(OrderPacket { frame: 0, orders }))
        if orders.iter().any(|o| o.order_string == "StartGame"))
}

impl ReplayRecorder {
    pub fn new(directory: PathBuf, filename: String) -> ReplayRecorder {
        ReplayRecorder {
            directory,
            filename,
            pre_start_buffer: Some(Vec::new()),
            file: None,
            path: None,
            metadata: None,
        }
    }

    /// The path of the replay file once the game has started.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn start_saving_replay(&mut self, initial_content: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        let mut id = -1;
        let file = loop {
            let name = if id < 0 {
                format!("{}.orarep", self.filename)
            } else {
                format!("{}-{id}.orarep", self.filename)
            };
            let path = self.directory.join(name);
            id += 1;
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(f) => {
                    self.path = Some(path);
                    break f;
                }
                Err(e) if id > CREATE_REPLAY_FILE_MAX_RETRY_COUNT => return Err(e),
                Err(_) => {}
            }
        };
        let mut writer = BufWriter::new(file);
        writer.write_all(initial_content)?;
        self.file = Some(writer);
        Ok(())
    }

    /// `ReplayRecorder.Receive`.
    pub fn receive(&mut self, client_id: i32, data: &[u8]) {
        if self.pre_start_buffer.is_some() && is_game_start(data) {
            let buffer = self.pre_start_buffer.take().unwrap_or_default();
            if let Err(e) = self.start_saving_replay(&buffer) {
                log::error!("Error creating replay file \"{}.orarep\": {e}", self.filename);
            }
        }
        let mut record = Vec::with_capacity(8 + data.len());
        record.extend_from_slice(&client_id.to_le_bytes());
        record.extend_from_slice(&(data.len() as i32).to_le_bytes());
        record.extend_from_slice(data);
        if let Some(buffer) = &mut self.pre_start_buffer {
            buffer.extend_from_slice(&record);
        } else if let Some(file) = &mut self.file {
            let _ = file.write_all(&record);
        }
    }
}

impl Drop for ReplayRecorder {
    /// `ReplayRecorder.Dispose`.
    fn drop(&mut self) {
        let Some(file) = &mut self.file else { return };
        if let Some(metadata) = &self.metadata {
            let mut info = metadata.lock().unwrap_or_else(|e| e.into_inner());
            info.root.end_time_utc = Some(crate::game_information::now_utc());
            let _ = file.write_all(&write_metadata(&info));
        }
        let _ = file.flush();
    }
}
