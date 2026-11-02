// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! OpenRA network protocol: order packets, replays and the lobby session.

pub mod binary;
pub mod order;
pub mod replay;
pub mod session;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
