// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The lobby setup that a client hands to the single player server it starts, like the skirmish settings that
//! `SkirmishLogic` restores. The client chooses the setup, and the server applies what is valid when the host joins.

use rustora_core::fieldloader::Color;

/// The settings of a player in a slot, `SkirmishSlot` in C#.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotSetup {
    pub slot: Option<String>,
    pub color: Option<Color>,
    pub faction: Option<String>,
    pub spawn_point: i32,
    pub team: i32,
    pub handicap: i32,
}

/// A bot of the setup.
#[derive(Clone, Debug, PartialEq)]
pub struct BotSetup {
    pub bot_type: String,
    pub slot: SlotSetup,
}

/// The lobby state that the server applies when the host joins.
#[derive(Clone, Debug, PartialEq)]
pub struct LobbySetup {
    pub map: String,
    /// The lobby options as id and value.
    pub options: Vec<(String, String)>,
    pub player: Option<SlotSetup>,
    pub bots: Vec<BotSetup>,
}

/// What the server does to the lobby when the host joins.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum InitialLobby {
    /// The lobby stays as the server set it up.
    #[default]
    Plain,
    /// Applies the setup. Without a setup, or if its map is unknown, the server adds the first bot type of the map
    /// to the first free bot slot instead.
    Skirmish(Option<LobbySetup>),
}
