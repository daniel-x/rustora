// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! State snapshots beside the OpenRA game saves. When the server writes a game save, the client writes the full
//! simulation state next to it. Loading the save still runs through the server, which sends the orders of the save,
//! but the client restores the snapshot and skips the orders before it instead of replaying them.
//!
//! A snapshot file holds a magic and a `rustora_engine::savegame` save. The format version in its header decides
//! whether this build can read it; a snapshot that it cannot read or restore leaves the game to load by replaying
//! the orders.

use std::path::Path;
use std::rc::Rc;

use rustora_engine::savegame::SaveHeader;
use rustora_engine::world::World;
use rustora_net::session::Session;

const MAGIC: &[u8; 14] = b"RUSTORA-SNAP-2";

/// Writes the snapshot of `world` for the game save `save`. Without a snapshot, for example for a world with Lua
/// scripts, an older snapshot of that save is removed.
pub fn write(world: &Rc<World>, save: &Path) {
    let path = rustora_net::game_save::snapshot_path(save);
    let written = rustora_launch::save_game(world).and_then(|state| {
        let mut data = Vec::with_capacity(MAGIC.len() + state.len());
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&state);
        std::fs::write(&path, data).map_err(|e| e.to_string())
    });
    if let Err(e) = written {
        log::info!("No state snapshot for {}: {e}", save.display());
        let _ = std::fs::remove_file(&path);
    }
}

/// The save game in the snapshot of the game save `save`, if there is one in a format version this build reads that
/// fits a game started with `lobby` whose saved orders end at net frame `last_orders_frame`.
pub fn read(save: &Path, lobby: &Session, last_orders_frame: i32) -> Option<Vec<u8>> {
    let data = std::fs::read(rustora_net::game_save::snapshot_path(save)).ok()?;
    let Some(state) = data.strip_prefix(MAGIC.as_slice()) else {
        log::info!("The state snapshot of {} has an unknown format", save.display());
        return None;
    };
    let header = match SaveHeader::read(state) {
        Ok((header, _)) => header,
        Err(e) => {
            log::info!("The state snapshot of {} cannot be read: {e}", save.display());
            return None;
        }
    };
    let saved = Session::deserialize(&header.lobby, "save game").ok()?;
    let fits = header.net_frame <= last_orders_frame + 1
        && saved.global_settings.map == lobby.global_settings.map
        && client_slots(&saved) == client_slots(lobby);
    fits.then(|| state.to_vec())
}

/// The index and slot of each human client, which decide who controls which player. Bots may get other indices when
/// a save loads; their orders come from the host, so the restored lobby keeps working for them.
fn client_slots(lobby: &Session) -> Vec<(i32, Option<String>)> {
    let mut slots: Vec<_> = lobby
        .clients
        .iter()
        .filter(|c| !c.is_bot())
        .map(|c| (c.index, c.slot.clone()))
        .collect();
    slots.sort();
    slots
}
