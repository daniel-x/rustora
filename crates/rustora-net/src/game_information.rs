// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `GameInformation`: the summary of a game that the metadata block of a replay holds.

use indexmap::IndexSet;
use rustora_core::field_enum;
use rustora_core::fieldloader::{Color, LoadError, load};
use rustora_core::miniyaml::MiniYaml;
use rustora_derive::FieldLoad;

use crate::NetError;

/// `WinState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WinState {
    #[default]
    Undefined,
    Won,
    Lost,
}
field_enum!(WinState, [Undefined, Won, Lost]);

/// `GameInformation.Player`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GamePlayer {
    pub client_index: i32,
    pub name: String,
    pub is_human: bool,
    pub is_bot: bool,
    pub bot_type: Option<String>,
    pub faction_name: Option<String>,
    pub faction_id: Option<String>,
    pub color: Color,
    pub display_faction_name: Option<String>,
    pub display_faction_id: Option<String>,
    pub team: i32,
    pub spawn_point: i32,
    pub handicap: i32,
    pub is_random_faction: bool,
    pub is_random_spawn_point: bool,
    pub fingerprint: Option<String>,
    pub outcome: WinState,
    pub outcome_timestamp_utc: Option<String>,
    pub disconnect_frame: i32,
}

/// The `Root` node of `GameInformation`. Times are `yyyy-MM-dd HH-mm-ss` strings as `FieldSaver` writes them.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GameRoot {
    #[field(name = "Mod")]
    pub mod_id: Option<String>,
    pub version: Option<String>,
    pub map_uid: Option<String>,
    pub map_title: Option<String>,
    pub final_game_tick: i32,
    pub start_time_utc: Option<String>,
    pub end_time_utc: Option<String>,
    pub disabled_spawn_points: IndexSet<i32>,
}

/// `GameInformation`.
#[derive(Clone, Debug, Default)]
pub struct GameInformation {
    pub root: GameRoot,
    pub players: Vec<GamePlayer>,
}

impl GameInformation {
    /// `GameInformation.Deserialize`.
    pub fn deserialize(data: &str, path: &str) -> Result<GameInformation, NetError> {
        let err = |e: LoadError| NetError::Invalid(format!("GameInformation deserialized invalid MiniYaml: {e}"));
        let nodes = MiniYaml::from_str(data, path, true).map_err(|e| err(e.into()))?;
        let mut info = GameInformation::default();
        for n in &nodes {
            match n.key.split('@').next().unwrap_or("") {
                "Root" => info.root = load(&n.value).map_err(err)?,
                "Player" => info.players.push(load(&n.value).map_err(err)?),
                _ => {}
            }
        }
        Ok(info)
    }

    /// The start time in seconds since the Unix epoch.
    pub fn start_time(&self) -> Option<i64> {
        self.root.start_time_utc.as_deref().and_then(parse_utc)
    }

    /// `GameInformation.Duration` in seconds.
    pub fn duration_seconds(&self) -> i64 {
        let end = self.root.end_time_utc.as_deref().and_then(parse_utc);
        match (self.start_time(), end) {
            (Some(s), Some(e)) if e > s => e - s,
            _ => 0,
        }
    }

    /// `GameInformation.IsSinglePlayer`.
    pub fn is_single_player(&self) -> bool {
        self.players.iter().filter(|p| p.is_human).count() == 1
    }

    /// The bot number of `ResolvedPlayerName`: the position among the bots of the same type, from 1.
    pub fn bot_number(&self, player: &GamePlayer) -> usize {
        self.players
            .iter()
            .filter(|p| p.bot_type == player.bot_type)
            .position(|p| std::ptr::eq(p, player))
            .map_or(0, |i| i + 1)
    }
}

/// Parses a `yyyy-MM-dd HH-mm-ss` time of `FieldSaver` to seconds since the Unix epoch.
pub fn parse_utc(text: &str) -> Option<i64> {
    let (date, time) = text.trim().split_once(' ')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(['-', ':']).map(|x| x.parse::<i64>().ok());
    let (h, min, s) = (t.next()??, t.next()??, t.next()??);
    Some(days_from_civil(y, m, day) * 86400 + h * 3600 + min * 60 + s)
}

/// The days since 1970-01-01 of a date in the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `ReplayMetadata`: the path of a replay file and its game information.
#[derive(Clone, Debug)]
pub struct ReplayMetadata {
    pub path: std::path::PathBuf,
    pub game_info: GameInformation,
}

impl ReplayMetadata {
    /// `ReplayMetadata.Read`: `None` for a file without a readable metadata block.
    pub fn read(path: &std::path::Path) -> Option<ReplayMetadata> {
        let data = std::fs::read(path).ok()?;
        let yaml = crate::replay::read_metadata(&data)?;
        let game_info = GameInformation::deserialize(&yaml, &path.to_string_lossy()).ok()?;
        Some(ReplayMetadata {
            path: path.to_path_buf(),
            game_info,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_field_saver_times() {
        assert_eq!(parse_utc("1970-01-01 00-00-00"), Some(0));
        assert_eq!(parse_utc("2026-11-02 19-13-29"), Some(1_793_646_809));
        assert_eq!(parse_utc("0001-01-01 00-00-00"), Some(-62_135_596_800));
    }

    #[test]
    fn deserializes_players() {
        let yaml = "Root:\n\tMod: ra\n\tVersion: v\n\tMapTitle: Ore Lord\n\tStartTimeUtc: 2026-11-02 19-13-29\n\tEndTimeUtc: 2026-11-02 19-13-46\n\tDisabledSpawnPoints:\nPlayer@0:\n\tName: a\n\tIsHuman: True\n\tColor: 200738\n\tOutcome: Won\n\tTeam: 2\n";
        let info = GameInformation::deserialize(yaml, "test").unwrap();
        assert_eq!(info.root.mod_id.as_deref(), Some("ra"));
        assert_eq!(info.duration_seconds(), 17);
        assert_eq!(info.players[0].outcome, WinState::Won);
        assert_eq!(info.players[0].team, 2);
        assert!(info.is_single_player());
    }
}
