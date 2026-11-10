// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Data that only the client uses: its settings, the map cache, the content installer, replays and the game
//! information they carry. The server never reads them.

pub mod cab;
pub mod game_information;
pub mod installer;
pub mod map_cache;
pub mod replay;
pub mod settings;
pub mod skirmish_file;
