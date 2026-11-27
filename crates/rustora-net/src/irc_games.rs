// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Game advertisements in the IRC channel `irc::SERVERS_CHANNEL`, a rustora addition to the master server and the
//! LAN beacon. A server sends one line per advertisement with the fields of `GameServer.ToPOSTData`:
//!
//! `Game: Protocol=2;Id=<guid>;Name=My game;Port=1234;Mod=ra;Version=release-20250330;ModTitle=Red Alert;Map=<uid>;`
//! `State=1;MaxPlayers=6;Players=1;Spectators=1;Bots=1;Protected=True;Authentication=True;DisabledSpawnPoints=2,3;`
//! `Clients=Name,RRGGBB,Faction,Team,SpawnPoint,Flags|...;Time=<UTC milliseconds>;Hash=<16 hex digits>`
//!
//! The fields from `Spectators` to `Clients` are left out when they are 0, `False` or empty. Flags hold `A` for an admin, `S` for a spectator and `B` for a bot. A backslash escapes `\`, `;`, `,` and `|` in
//! the values. The address of the game is the host that the IRC server shows for the sender, with the port of the
//! line. The client list holds as many clients as fit into one IRC line; the counts are always complete. A client
//! that joins the channel sends `Query:;Time=...;Hash=...`, which servers answer with an advertisement. Servers
//! advertise at least once a minute; clients drop a game that was silent for 90 seconds.
//!
//! Every line ends with the time of sending and the first 16 hex digits of the SHA-1 of the line before `;Hash=`.
//! A receiver drops lines with a wrong hash, lines older than `MAX_AGE` or further in the future, and
//! advertisements that are not newer than the last one of the same game, so that a copy of an old line does
//! not count. The hash has no secret, so it detects altered copies, not forged lines.

use rustora_core::fieldloader::Color;
use sha1::{Digest, Sha1};

use crate::game_server::{GameClient, GameServer, PROTOCOL_VERSION};
use crate::session::fmt_color;

pub const GAME_PREFIX: &str = "Game: ";
pub const QUERY_PREFIX: &str = "Query:";
/// The longest advertisement: 512 bytes minus the line end and the longest source prefix and command that the IRC
/// server puts in front when it passes the line on.
pub const MAX_LINE_BYTES: usize = 380;
/// `GameServer.State` of a server that shuts down.
pub const STATE_SHUTTING_DOWN: i32 = 3;
/// What the server list shows as the location of a game from IRC.
pub const LOCATION: &str = "IRC";
/// The longest server name in an advertisement, in bytes.
const MAX_NAME_BYTES: usize = 50;
/// The largest difference in milliseconds between the time of a line and the time of its receiver, for the
/// travel time and for clocks that are off.
pub const MAX_AGE: i64 = 5 * 60 * 1000;
const HASH_FIELD: &str = ";Hash=";
/// The length of `;Time=<13 digits>;Hash=<16 hex digits>`.
const SIGNATURE_BYTES: usize = 6 + 13 + HASH_FIELD.len() + 16;

/// The milliseconds since the Unix epoch.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The first 16 hex digits of the SHA-1 of `text`.
fn hash(text: &str) -> String {
    Sha1::digest(text.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// `line` with `;Time=<time>;Hash=<hash>` appended.
fn sign(mut line: String, time: i64) -> String {
    line.push_str(&format!(";Time={time}"));
    let hash = hash(&line);
    line.push_str(HASH_FIELD);
    line.push_str(&hash);
    line
}

/// The line without its hash and its time, if the hash fits and the time is within `MAX_AGE` of `now`.
fn verify(line: &str, now: i64) -> Option<(&str, i64)> {
    let (signed, hash_value) = line.rsplit_once(HASH_FIELD)?;
    if hash(signed) != hash_value {
        return None;
    }
    let (body, time) = signed.rsplit_once(";Time=")?;
    let time: i64 = time.parse().ok()?;
    ((now - time).abs() <= MAX_AGE).then_some((body, time))
}

/// The query that a client sends at `time`.
pub fn query(time: i64) -> String {
    sign(QUERY_PREFIX.to_string(), time)
}

/// Whether `line` is a valid query at `now`.
pub fn is_query(line: &str, now: i64) -> bool {
    verify(line, now).is_some_and(|(body, _)| body == QUERY_PREFIX)
}

const ESCAPED: [char; 4] = ['\\', ';', ',', '|'];

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars().filter(|c| !c.is_control()) {
        if ESCAPED.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// Splits at the `separator`s that no backslash escapes; the parts keep their escapes.
fn split_escaped(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            c if c == separator => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn fmt_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

fn client_entry(c: &GameClient) -> String {
    let flags: String = [(c.is_admin, 'A'), (c.is_spectator, 'S'), (c.is_bot, 'B')]
        .into_iter()
        .filter_map(|(set, flag)| set.then_some(flag))
        .collect();
    format!(
        "{},{},{},{},{},{flags}",
        escape(c.name.as_deref().unwrap_or_default()),
        fmt_color(c.color),
        escape(c.faction.as_deref().unwrap_or_default()),
        c.team,
        c.spawn_point,
    )
}

/// `text` cut to at most `bytes` bytes at a character boundary, without a dangling escape.
fn cut(text: &str, bytes: usize) -> &str {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut cut = &text[..end];
    let trailing = cut.len() - cut.trim_end_matches('\\').len();
    if trailing % 2 == 1 {
        cut = &cut[..cut.len() - 1];
    }
    cut
}

/// The advertisement of `game` at `time`, the port of which is the port of its `Address`, with the session `guid`.
pub fn advertisement(game: &GameServer, guid: &str, time: i64) -> String {
    let limit = MAX_LINE_BYTES - SIGNATURE_BYTES;
    let text = |s: &Option<String>| escape(s.as_deref().unwrap_or_default());
    let count = |f: &dyn Fn(&GameClient) -> bool| game.clients.iter().filter(|c| f(c)).count();
    let port = game.address.as_deref().and_then(|a| a.rsplit_once(':')).map_or("", |(_, p)| p);
    let spawns: Vec<String> = game.disabled_spawn_points.iter().map(i32::to_string).collect();
    let head = format!("{GAME_PREFIX}Protocol={PROTOCOL_VERSION};Id={};Name=", escape(guid));
    let mut tail = format!(
        ";Port={port};Mod={};Version={};ModTitle={};Map={};State={};MaxPlayers={};Players={}",
        text(&game.mod_id),
        text(&game.version),
        text(&game.mod_title),
        text(&game.map),
        game.state,
        game.max_players,
        count(&|c| !c.is_bot && !c.is_spectator),
    );
    let optional = [
        ("Spectators", count(&|c| c.is_spectator).to_string(), "0"),
        ("Bots", count(&|c| c.is_bot).to_string(), "0"),
        ("Protected", fmt_bool(game.protected).to_string(), "False"),
        ("Authentication", fmt_bool(game.authentication).to_string(), "False"),
        ("DisabledSpawnPoints", spawns.join(","), ""),
    ];
    for (key, value, default) in optional {
        if value != default {
            tail.push_str(&format!(";{key}={value}"));
        }
    }
    if !game.clients.is_empty() {
        tail.push_str(";Clients=");
    }
    let name = text(&game.name);
    let name = cut(&name, limit.saturating_sub(head.len() + tail.len()).min(MAX_NAME_BYTES));
    let mut line = format!("{head}{name}{tail}");
    for (i, client) in game.clients.iter().enumerate() {
        let entry = client_entry(client);
        let separator = if i == 0 { "" } else { "|" };
        if line.len() + separator.len() + entry.len() > limit {
            break;
        }
        line.push_str(separator);
        line.push_str(&entry);
    }
    sign(line, time)
}

fn parse_client(entry: &str) -> Option<GameClient> {
    let fields = split_escaped(entry, ',');
    let [name, color, faction, team, spawn_point, flags] = fields.as_slice() else {
        return None;
    };
    let text = |s: &str| Some(unescape(s)).filter(|s| !s.is_empty());
    Some(GameClient {
        name: text(name),
        fingerprint: None,
        color: Color::parse(color)?,
        faction: text(faction),
        team: team.parse().ok()?,
        spawn_point: spawn_point.parse().ok()?,
        is_admin: flags.contains('A'),
        is_spectator: flags.contains('S'),
        is_bot: flags.contains('B'),
    })
}

/// The address of a game at `host`, the host part of an IRC source, or `None` for a cloak like `user/name`.
fn address(host: &str, port: u16) -> Option<String> {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        return Some(format!("[{host}]:{port}"));
    }
    let valid = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    valid.then(|| format!("{host}:{port}"))
}

/// An advertisement that a receiver accepted.
#[derive(Debug)]
pub struct Advertisement {
    pub guid: String,
    /// The time of sending, in milliseconds since the Unix epoch.
    pub time: i64,
    pub game: GameServer,
}

/// The advertisement that the sender at `host` sent, if it is valid at `now`.
pub fn parse_advertisement(line: &str, host: &str, now: i64) -> Option<Advertisement> {
    let (line, time) = verify(line, now)?;
    let body = line.strip_prefix(GAME_PREFIX)?;
    let mut game = GameServer {
        id: -1,
        location: Some(LOCATION.to_string()),
        ..Default::default()
    };
    let mut guid = None;
    let mut port = None;
    for field in split_escaped(body, ';') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        let text = || Some(unescape(value));
        let number = || value.parse::<i32>().ok();
        match key {
            "Id" => guid = text(),
            "Name" => game.name = text(),
            "Port" => port = value.parse::<u16>().ok(),
            "Mod" => game.mod_id = text(),
            "Version" => game.version = text(),
            "ModTitle" => game.mod_title = text(),
            "Map" => game.map = text(),
            "State" => game.state = number()?,
            "MaxPlayers" => game.max_players = number()?,
            "Protected" => game.protected = value == "True",
            "Authentication" => game.authentication = value == "True",
            "Players" => game.players = number()?,
            "Spectators" => game.spectators = number()?,
            "Bots" => game.bots = number()?,
            "DisabledSpawnPoints" => {
                game.disabled_spawn_points = value.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            }
            "Clients" if !value.is_empty() => {
                game.clients = split_escaped(value, '|').into_iter().filter_map(parse_client).collect();
            }
            _ => {}
        }
    }
    game.mod_id.as_ref()?;
    game.port = i32::from(port?);
    game.address = Some(address(host, port?)?);
    Some(Advertisement { guid: guid?, time, game })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_764_200_000_000;

    fn game() -> GameServer {
        GameServer {
            name: Some("Test; a, b | c\\".into()),
            address: Some("0.0.0.0:1234".into()),
            state: 1,
            max_players: 6,
            map: Some("0123456789abcdef0123456789abcdef01234567".into()),
            mod_id: Some("ra".into()),
            version: Some("release-20250330".into()),
            mod_title: Some("Red Alert".into()),
            protected: true,
            clients: vec![
                GameClient {
                    name: Some("Ad,min".into()),
                    color: Color::from_argb(255, 1, 2, 3),
                    faction: Some("soviet".into()),
                    team: 1,
                    spawn_point: 2,
                    is_admin: true,
                    ..Default::default()
                },
                GameClient {
                    name: Some("Bot".into()),
                    is_bot: true,
                    is_spectator: false,
                    ..Default::default()
                },
            ],
            disabled_spawn_points: vec![2, 3],
            ..Default::default()
        }
    }

    #[test]
    fn round_trip() {
        let line = advertisement(&game(), "0011223344556677", NOW);
        assert!(line.starts_with("Game: Protocol=2;Id=0011223344556677;Name=Test\\; a\\, b \\| c\\\\;Port=1234;"));
        assert!(!line.contains('\n'));
        let parsed = parse_advertisement(&line, "203.0.113.5", NOW + 1000).unwrap();
        assert_eq!((parsed.guid.as_str(), parsed.time), ("0011223344556677", NOW));
        let back = parsed.game;
        assert_eq!(back.name, game().name);
        assert_eq!(back.address.as_deref(), Some("203.0.113.5:1234"));
        assert_eq!((back.players, back.spectators, back.bots), (1, 0, 1));
        assert_eq!(back.clients.len(), 2);
        assert_eq!(back.clients[0].name.as_deref(), Some("Ad,min"));
        assert_eq!(back.clients[0].color, Color::from_argb(255, 1, 2, 3));
        assert!(back.clients[0].is_admin && back.clients[1].is_bot);
        assert_eq!(back.disabled_spawn_points, [2, 3]);
        assert!(back.protected && !back.authentication);
        assert_eq!(back.mod_title.as_deref(), Some("Red Alert"));
        assert_eq!(back.location.as_deref(), Some(LOCATION));
    }

    #[test]
    fn long_games_fit_into_one_line() {
        let mut game = game();
        game.name = Some("x".repeat(500));
        game.clients = (0..16)
            .map(|i| GameClient {
                name: Some(format!("Player number {i}")),
                ..Default::default()
            })
            .collect();
        let line = advertisement(&game, "0011223344556677", NOW);
        assert!(line.len() <= MAX_LINE_BYTES);
        let back = parse_advertisement(&line, "host.example", NOW).unwrap().game;
        assert_eq!(back.players, 16);
        assert!(!back.clients.is_empty() && back.clients.len() < 16);
        assert_eq!(cut("ab\\\\c", 4), "ab\\\\");
        assert_eq!(cut("ab\\;c", 3), "ab");
    }

    #[test]
    fn hosts() {
        let line = advertisement(&game(), "g", NOW);
        assert_eq!(
            parse_advertisement(&line, "2001:db8::1", NOW).unwrap().game.address.as_deref(),
            Some("[2001:db8::1]:1234")
        );
        assert!(parse_advertisement(&line, "user/someone", NOW).is_none());
        assert!(parse_advertisement("hello", "203.0.113.5", NOW).is_none());
    }

    #[test]
    fn old_and_altered_lines_are_dropped() {
        let line = advertisement(&game(), "g", NOW);
        assert!(parse_advertisement(&line, "h", NOW + MAX_AGE).is_some());
        assert!(parse_advertisement(&line, "h", NOW + MAX_AGE + 1).is_none());
        assert!(parse_advertisement(&line, "h", NOW - MAX_AGE - 1).is_none());
        let altered = line.replace("State=1", "State=2");
        assert!(parse_advertisement(&altered, "h", NOW).is_none());
        let query = query(NOW);
        assert!(query.starts_with("Query:;Time=1764200000000;Hash="));
        assert!(is_query(&query, NOW));
        assert!(!is_query(&query, NOW + MAX_AGE + 1));
        assert!(!is_query(&line, NOW));
    }
}
