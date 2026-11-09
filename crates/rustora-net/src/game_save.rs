// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Game saves (`OpenRA.Game/Network/GameSave.cs`): an `.orasav` file holds the order stream of a game, the last sync
//! packet, the lobby state and the data of `IGameSaveTraitData` traits. Loading replays the orders.

use std::path::Path;

use indexmap::IndexMap;
use rustora_core::fieldloader::{Color, load};
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode, nodes_to_string};
use rustora_derive::FieldLoad;

use crate::binary::Reader;
use crate::order::{RawOrder, SYNC_HASH_ORDER_LENGTH, order_type};
use crate::session::{Client, Global, Session, Slot, fmt_bool, fmt_color, fmt_opt, yaml};

const EOF_MARKER: i32 = -2;
const METADATA_MARKER: i32 = -1;
const TRAIT_DATA_MARKER: i32 = -3;
const IMMEDIATE_ORDERS_MARKER: u8 = 0xFE;
const TRAILER_LENGTH: usize = 12;

/// `SlotClient`: the state of a client that matters to the game.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct SlotClient {
    pub color: Color,
    pub faction: Option<String>,
    pub spawn_point: i32,
    pub team: i32,
    pub handicap: i32,
    pub slot: Option<String>,
    pub bot: Option<String>,
    pub is_admin: bool,
    pub bot_name: Option<String>,
}

impl SlotClient {
    fn new(client: &Client) -> SlotClient {
        SlotClient {
            color: client.color,
            faction: client.faction.clone(),
            spawn_point: client.spawn_point,
            team: client.team,
            handicap: client.handicap,
            slot: client.slot.clone(),
            bot: client.bot.clone(),
            is_admin: client.is_admin,
            bot_name: client.bot.as_ref().and_then(|_| client.name.clone()),
        }
    }

    pub fn apply_to(&self, client: &mut Client) {
        client.color = self.color;
        client.faction = self.faction.clone();
        client.spawn_point = self.spawn_point;
        client.team = self.team;
        client.handicap = self.handicap;
        client.slot = self.slot.clone();
        client.bot = self.bot.clone();
        client.is_admin = self.is_admin;
        if self.bot.is_some() {
            client.name = self.bot_name.clone();
        }
    }

    fn serialize(&self, key: &str) -> MiniYamlNode {
        MiniYamlNode::new(
            format!("SlotClient@{key}"),
            yaml(vec![
                ("Color", fmt_color(self.color)),
                ("Faction", fmt_opt(&self.faction)),
                ("SpawnPoint", self.spawn_point.to_string()),
                ("Team", self.team.to_string()),
                ("Handicap", self.handicap.to_string()),
                ("Slot", fmt_opt(&self.slot)),
                ("Bot", fmt_opt(&self.bot)),
                ("IsAdmin", fmt_bool(self.is_admin)),
                ("BotName", fmt_opt(&self.bot_name)),
            ]),
        )
    }
}

/// A packet to send to the clients when a game save starts: the frame, the client and the data.
pub type SavedPacket = (i32, i32, Vec<u8>);

/// `GameSave`.
#[derive(Debug)]
pub struct GameSave {
    orders: Vec<u8>,
    pub last_orders_frame: i32,
    pub last_sync_frame: i32,
    last_sync_packet: Vec<u8>,
    pub global_settings: Global,
    pub slots: IndexMap<String, Slot>,
    pub slot_clients: IndexMap<String, SlotClient>,
    trait_data: IndexMap<i32, MiniYaml>,
    clients_by_slot_index: Vec<i32>,
    first_bot_slot_index: i32,
}

impl Default for GameSave {
    fn default() -> Self {
        GameSave {
            orders: Vec::new(),
            last_orders_frame: -1,
            last_sync_frame: 0,
            last_sync_packet: Vec::new(),
            global_settings: Global::default(),
            slots: IndexMap::new(),
            slot_clients: IndexMap::new(),
            trait_data: IndexMap::new(),
            clients_by_slot_index: Vec::new(),
            first_bot_slot_index: -1,
        }
    }
}

fn read_text<'a>(r: &mut Reader<'a>) -> Result<&'a str, String> {
    let length = r.i32().map_err(|e| e.to_string())?;
    let length = usize::try_from(length).map_err(|_| "Invalid orasav file".to_string())?;
    let bytes = r.bytes(length).map_err(|e| e.to_string())?;
    std::str::from_utf8(bytes).map_err(|e| e.to_string())
}

fn write_text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as i32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

impl GameSave {
    /// Reads an `.orasav` file.
    pub fn read(path: &Path) -> Result<GameSave, String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        GameSave::parse(&data, &path.display().to_string())
    }

    fn parse(data: &[u8], name: &str) -> Result<GameSave, String> {
        const INVALID: &str = "Invalid orasav file";
        let body_end = data.len().checked_sub(TRAILER_LENGTH).ok_or(INVALID)?;
        let mut trailer = Reader::new(&data[body_end..]);
        let metadata_offset = trailer.i32().map_err(|e| e.to_string())?;
        let trait_data_offset = trailer.i32().map_err(|e| e.to_string())?;
        if trailer.i32().map_err(|e| e.to_string())? != EOF_MARKER {
            return Err(INVALID.to_string());
        }
        let metadata_offset = usize::try_from(metadata_offset).map_err(|_| INVALID)?;
        let trait_data_offset = usize::try_from(trait_data_offset).map_err(|_| INVALID)?;
        let metadata = data.get(metadata_offset..body_end).ok_or(INVALID)?;
        let mut r = Reader::new(metadata);
        let int = |r: &mut Reader| r.i32().map_err(|e| e.to_string());
        if int(&mut r)? != METADATA_MARKER {
            return Err(INVALID.to_string());
        }
        let last_orders_frame = int(&mut r)?;
        let last_sync_frame = int(&mut r)?;
        let last_sync_packet = r.bytes(SYNC_HASH_ORDER_LENGTH).map_err(|e| e.to_string())?.to_vec();

        let session = |text: &str, part: &str| Session::deserialize(text, &format!("{name}:{part}")).map_err(|e| e.to_string());
        let global_settings = session(read_text(&mut r)?, "globalSettings")?.global_settings;
        let slots = session(read_text(&mut r)?, "slots")?.slots;

        let text = read_text(&mut r)?;
        let nodes = MiniYaml::from_str(text, &format!("{name}:slotClients"), true).map_err(|e| e.to_string())?;
        let mut slot_clients = IndexMap::new();
        for n in &nodes {
            let client: SlotClient = load(&n.value).map_err(|e| e.to_string())?;
            let slot = client.slot.clone().ok_or(INVALID)?;
            slot_clients.insert(slot, client);
        }

        if metadata_offset + metadata.len() - r.remaining() != trait_data_offset || int(&mut r)? != TRAIT_DATA_MARKER {
            return Err(INVALID.to_string());
        }
        let text = read_text(&mut r)?;
        let nodes = MiniYaml::from_str(text, &format!("{name}:traitData"), true).map_err(|e| e.to_string())?;
        let mut trait_data = IndexMap::new();
        for n in nodes {
            trait_data.insert(n.key.parse::<i32>().map_err(|e| e.to_string())?, n.value);
        }

        Ok(GameSave {
            orders: data[..metadata_offset].to_vec(),
            last_orders_frame,
            last_sync_frame,
            last_sync_packet,
            global_settings,
            slots,
            slot_clients,
            trait_data,
            ..Default::default()
        })
    }

    /// `StartGame`: records the lobby state. `is_playable` tells whether a player reference is playable.
    pub fn start_game(&mut self, lobby: &Session, is_playable: impl Fn(&str) -> bool) {
        self.clients_by_slot_index = lobby
            .slots
            .keys()
            .map(|s| lobby.client_in_slot(s).map_or(-1, |c| c.index))
            .collect();
        self.global_settings = lobby.global_settings.clone();
        self.slots = IndexMap::new();
        self.slot_clients = IndexMap::new();
        for (key, slot) in &lobby.slots {
            self.slots.insert(key.clone(), slot.clone());
            let Some(client) = lobby.client_in_slot(key) else {
                continue;
            };
            if is_playable(&slot.player_reference) {
                self.slot_clients.insert(key.clone(), SlotClient::new(client));
                if client.bot.is_some() && self.first_bot_slot_index < 0 {
                    self.first_bot_slot_index = self.slot_of(client.index);
                }
            }
        }
    }

    fn slot_of(&self, client_index: i32) -> i32 {
        self.clients_by_slot_index
            .iter()
            .position(|c| *c == client_index)
            .map_or(-1, |i| i as i32)
    }

    /// `DispatchOrders`: appends the orders of a frame. Orders of spectators count as orders of the first bot,
    /// like in the C# game, because a spectating host controls the bots of a skirmish.
    pub fn dispatch_orders(&mut self, player_index: i32, frame: i32, data: &[u8]) {
        if data.first() == Some(&order_type::SYNC_HASH) && frame > self.last_sync_frame {
            if data.len() != SYNC_HASH_ORDER_LENGTH {
                return;
            }
            self.last_sync_frame = frame;
            self.last_sync_packet = data.to_vec();
        }
        if frame <= self.last_orders_frame || data.first() == Some(&IMMEDIATE_ORDERS_MARKER) {
            return;
        }
        let slot = match self.slot_of(player_index) {
            -1 => self.first_bot_slot_index,
            s => s,
        };
        self.orders.extend_from_slice(&(data.len() as i32 + 8).to_le_bytes());
        self.orders.extend_from_slice(&frame.to_le_bytes());
        self.orders.extend_from_slice(&slot.to_le_bytes());
        self.orders.extend_from_slice(data);
        self.last_orders_frame = frame;
    }

    /// `ParseOrders`: the trait data, the saved orders and the last sync packet, in sending order.
    pub fn parse_orders(&self, lobby: &Session) -> Result<Vec<SavedPacket>, String> {
        let mut packets = Vec::new();
        for (index, data) in &self.trait_data {
            let text = nodes_to_string(&[MiniYamlNode::new(index.to_string(), data.clone())]);
            packets.push((0, 0, RawOrder::from_target_string("SaveTraitData", &text, true).serialize()));
        }
        let mut r = Reader::new(&self.orders);
        while r.remaining() > 0 {
            let length = r.i32().map_err(|e| e.to_string())? - 8;
            let frame = r.i32().map_err(|e| e.to_string())?;
            let slot = r.i32().map_err(|e| e.to_string())?;
            let length = usize::try_from(length).map_err(|_| "Invalid orasav file")?;
            let data = r.bytes(length).map_err(|e| e.to_string())?;
            let mut client_index = usize::try_from(slot)
                .ok()
                .and_then(|s| self.clients_by_slot_index.get(s).copied())
                .ok_or_else(|| format!("Saved orders of frame {frame} refer to the empty slot {slot}"))?;
            if let Some(controller) = lobby
                .client_with_index(client_index)
                .filter(|c| c.bot.is_some())
                .map(|c| c.bot_controller_client_index)
            {
                client_index = controller;
            }
            packets.push((frame, client_index, data.to_vec()));
        }
        packets.push((self.last_sync_frame, 0, self.last_sync_packet.clone()));
        Ok(packets)
    }

    /// `AddTraitData`.
    pub fn add_trait_data(&mut self, trait_index: i32, data: MiniYaml) {
        self.trait_data.insert(trait_index, data);
    }

    /// The bytes of the `.orasav` file. Fails before the first sync packet arrived.
    pub fn serialize(&self) -> Result<Vec<u8>, String> {
        if self.last_sync_packet.len() != SYNC_HASH_ORDER_LENGTH {
            return Err("The game has no sync packet to save yet".to_string());
        }
        let mut out = self.orders.clone();
        out.extend_from_slice(&METADATA_MARKER.to_le_bytes());
        out.extend_from_slice(&self.last_orders_frame.to_le_bytes());
        out.extend_from_slice(&self.last_sync_frame.to_le_bytes());
        out.extend_from_slice(&self.last_sync_packet);
        write_text(&mut out, &nodes_to_string(&[self.global_settings.serialize()]));
        let slots: Vec<MiniYamlNode> = self.slots.values().map(Slot::serialize).collect();
        write_text(&mut out, &nodes_to_string(&slots));
        let clients: Vec<MiniYamlNode> = self.slot_clients.iter().map(|(k, c)| c.serialize(k)).collect();
        write_text(&mut out, &nodes_to_string(&clients));
        let trait_data_offset = out.len() as i32;
        out.extend_from_slice(&TRAIT_DATA_MARKER.to_le_bytes());
        let traits: Vec<MiniYamlNode> = self
            .trait_data
            .iter()
            .map(|(k, v)| MiniYamlNode::new(k.to_string(), v.clone()))
            .collect();
        write_text(&mut out, &nodes_to_string(&traits));
        out.extend_from_slice(&(self.orders.len() as i32).to_le_bytes());
        out.extend_from_slice(&trait_data_offset.to_le_bytes());
        out.extend_from_slice(&EOF_MARKER.to_le_bytes());
        Ok(out)
    }

    /// `Save`.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.serialize()?).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Removes the characters that are not allowed in a file name on any supported platform, because the name comes
/// from a client. `GetInvalidFileNameChars` depends on the platform in C#. Returns `None` if no usable name remains.
pub fn sanitized_file_name(name: &str) -> Option<String> {
    let clean: String = name.chars().filter(|c| !c.is_control() && !"<>:\"/\\|?*".contains(*c)).collect();
    (!clean.trim_matches(['.', ' ']).is_empty()).then_some(clean)
}

/// The directory of the saves of a mod version, see [`rustora_core::own_files::saves_dir`].
pub fn saves_dir(support_dir: &Path, mod_id: &str, version: &str) -> std::path::PathBuf {
    rustora_core::own_files::saves_dir(support_dir, mod_id, version)
}

/// The rustora state snapshot that belongs to the game save `save`. It lets the client restore the game at once
/// instead of replaying the orders of the save.
pub fn snapshot_path(save: &Path) -> std::path::PathBuf {
    save.with_extension("rsnap")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lobby() -> Session {
        let mut lobby = Session {
            clients: vec![
                Client {
                    index: 0,
                    is_admin: true,
                    ..Default::default()
                },
                Client {
                    index: 1,
                    bot: Some("rush".to_string()),
                    name: Some("Rush".to_string()),
                    slot: Some("Multi0".to_string()),
                    bot_controller_client_index: 0,
                    spawn_point: 2,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        lobby.slots.insert(
            "Multi0".to_string(),
            Slot {
                player_reference: "Multi0".to_string(),
                ..Default::default()
            },
        );
        lobby.global_settings.map = Some("abc".to_string());
        lobby
    }

    fn sync_packet(hash: i32) -> Vec<u8> {
        let mut p = vec![order_type::SYNC_HASH];
        p.extend_from_slice(&hash.to_le_bytes());
        p.extend_from_slice(&0u64.to_le_bytes());
        p
    }

    fn saved_game() -> GameSave {
        let mut save = GameSave::default();
        save.start_game(&lobby(), |_| true);
        save.dispatch_orders(0, 2, &[]);
        save.dispatch_orders(0, 1, &sync_packet(7));
        save.dispatch_orders(0, 3, &[1, 2, 3]);
        save.add_trait_data(4, MiniYaml::new(None, vec![MiniYamlNode::with_value_str("A", "b")]));
        save
    }

    #[test]
    fn file_round_trip_is_bit_identical() {
        let bytes = saved_game().serialize().unwrap();
        let read = GameSave::parse(&bytes, "test").unwrap();
        assert_eq!(read.last_orders_frame, 3);
        assert_eq!(read.last_sync_frame, 1);
        assert_eq!(read.slot_clients["Multi0"].bot_name.as_deref(), Some("Rush"));
        assert_eq!(read.slot_clients["Multi0"].spawn_point, 2);
        assert_eq!(read.serialize().unwrap(), bytes);
    }

    #[test]
    fn spectator_orders_replay_through_the_bot_controller() {
        let read = GameSave::parse(&saved_game().serialize().unwrap(), "test").unwrap();
        let mut restored = read;
        restored.start_game(&lobby(), |_| true);
        let packets = restored.parse_orders(&lobby()).unwrap();
        assert_eq!(packets.len(), 4);
        assert_eq!(packets[0].0, 0);
        assert_eq!((packets[1].0, packets[1].1, packets[1].2.len()), (2, 0, 0));
        assert_eq!((packets[2].0, packets[2].1, packets[2].2.clone()), (3, 0, vec![1, 2, 3]));
        assert_eq!((packets[3].0, packets[3].2.clone()), (1, sync_packet(7)));
    }

    #[test]
    fn old_frames_and_immediate_orders_are_not_saved() {
        let mut save = saved_game();
        let before = save.orders.len();
        save.dispatch_orders(0, 3, &[9]);
        save.dispatch_orders(0, 4, &[IMMEDIATE_ORDERS_MARKER, 1]);
        assert_eq!(save.orders.len(), before);
    }

    #[test]
    fn invalid_files_are_rejected() {
        let bytes = saved_game().serialize().unwrap();
        assert!(GameSave::parse(&bytes[..4], "test").is_err());
        assert!(GameSave::parse(&bytes[1..], "test").is_err());
        let mut bad = bytes;
        let n = bad.len();
        bad[n - 12..n - 8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(GameSave::parse(&bad, "test").is_err());
    }

    #[test]
    fn file_names_cannot_leave_the_saves_directory() {
        assert_eq!(sanitized_file_name("../../a/b.orasav").as_deref(), Some("....ab.orasav"));
        assert_eq!(sanitized_file_name("..").as_deref(), None);
        assert_eq!(sanitized_file_name("").as_deref(), None);
        assert_eq!(sanitized_file_name("my game.orasav").as_deref(), Some("my game.orasav"));
    }
}
