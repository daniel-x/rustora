// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! OpenRA compatible game server (`OpenRA.Game/Server`).

pub mod auth;
pub mod connection;
pub mod geoip;
pub mod lobby;
pub mod master_server;
pub mod mod_info;
pub mod nat;
pub mod order_buffer;
pub mod server;
mod skirmish_logic;
pub mod vote_kick;
