// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! OpenRA network protocol: order packets, replays and the lobby session.

pub mod beacon;
pub mod binary;
pub mod game_save;
pub mod game_server;
pub mod handshake;
pub mod lobby_setup;
pub mod order;
pub mod session;
pub mod stream;
pub mod web;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
