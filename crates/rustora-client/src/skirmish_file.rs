// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The skirmish file `skirmish.<mod>.yaml` of the support dir, the client part of `SkirmishLogic.cs`: the last
//! skirmish setup, saved after each lobby change and restored when the next skirmish starts.

use std::path::{Path, PathBuf};

use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode, nodes_to_string};
use rustora_net::lobby_setup::{BotSetup, LobbySetup, SlotSetup};
use rustora_net::session::{Client, Session, fmt_color};

/// The skirmish file of the mod `mod_id`.
pub fn path(support: &Path, mod_id: &str) -> PathBuf {
    support.join(format!("skirmish.{mod_id}.yaml"))
}

/// `SkirmishSlot` as yaml: the slot is the value, the rest are child nodes.
fn slot_yaml(client: &Client) -> MiniYaml {
    let field = |key: &str, value: String| MiniYamlNode::with_value_str(key, value);
    MiniYaml::new(
        client.slot.clone(),
        vec![
            field("Color", fmt_color(client.color)),
            field("Faction", client.faction.clone().unwrap_or_default()),
            field("SpawnPoint", client.spawn_point.to_string()),
            field("Team", client.team.to_string()),
            field("Handicap", client.handicap.to_string()),
        ],
    )
}

fn slot_setup(yaml: &MiniYaml) -> SlotSetup {
    let field = |key: &str| yaml.nodes.iter().find(|n| n.key == key).map(|n| n.value.value_str().trim());
    let number = |key: &str| field(key).and_then(|v| v.parse().ok()).unwrap_or(0);
    SlotSetup {
        slot: yaml.value.clone().filter(|s| !s.is_empty()),
        color: field("Color").and_then(Color::parse),
        faction: field("Faction").filter(|f| !f.is_empty()).map(str::to_string),
        spawn_point: number("SpawnPoint"),
        team: number("Team"),
        handicap: number("Handicap"),
    }
}

/// The setup of the skirmish file at `path`, or `None` if it is missing, unreadable or names no map.
pub fn read(path: &Path) -> Option<LobbySetup> {
    let data = std::fs::read(path).ok()?;
    let nodes = MiniYaml::from_bytes(&data, "skirmish", true).ok()?;
    let section = |key: &str| nodes.iter().find(|n| n.key == key).map(|n| &n.value);
    let map = section("Map")?.value_str().to_string();
    let options = section("Options").map_or_else(Vec::new, |o| {
        o.nodes.iter().map(|n| (n.key.clone(), n.value.value_str().to_string())).collect()
    });
    let bots = section("Bots").map_or_else(Vec::new, |b| {
        b.nodes
            .iter()
            .map(|n| BotSetup {
                bot_type: n.key.clone(),
                slot: slot_setup(&n.value),
            })
            .collect()
    });
    Some(LobbySetup {
        map,
        options,
        player: section("Player").map(slot_setup),
        bots,
    })
}

/// `LobbyInfoSynced`: writes the setup of `session` to `path`.
pub fn write(path: &Path, session: &Session) {
    let Some(player) = session.non_bot_clients().next() else {
        return;
    };
    let global = &session.global_settings;
    let options = global
        .lobby_options
        .iter()
        .map(|(k, v)| MiniYamlNode::with_value_str(k.clone(), v.value.clone().unwrap_or_default()))
        .collect();
    let bots = session
        .clients
        .iter()
        .filter(|c| c.is_bot())
        .map(|b| MiniYamlNode::new(b.bot.clone().unwrap_or_default(), slot_yaml(b)))
        .collect();
    let nodes = vec![
        MiniYamlNode::with_value_str("Map", global.map.clone().unwrap_or_default()),
        MiniYamlNode::new("Options", MiniYaml::new(None, options)),
        MiniYamlNode::new("Player", slot_yaml(player)),
        MiniYamlNode::new("Bots", MiniYaml::new(None, bots)),
    ];
    if let Err(e) = std::fs::write(path, nodes_to_string(&nodes)) {
        log::info!(target: "client", "Cannot write the skirmish settings: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_setup_reads_back() {
        let mut session = Session::default();
        session.global_settings.map = Some("abc".into());
        session.clients.push(Client {
            index: 0,
            slot: Some("Multi0".into()),
            color: Color::from_argb(255, 200, 32, 32),
            faction: Some("england".into()),
            team: 1,
            ..Default::default()
        });
        session.clients.push(Client {
            index: 1,
            slot: Some("Multi1".into()),
            bot: Some("normal".into()),
            spawn_point: 2,
            ..Default::default()
        });
        let file = std::env::temp_dir().join(format!("rustora-skirmish-{}.yaml", std::process::id()));
        write(&file, &session);
        let setup = read(&file).unwrap();
        let _ = std::fs::remove_file(&file);
        assert_eq!(setup.map, "abc");
        let player = setup.player.unwrap();
        assert_eq!(player.slot.as_deref(), Some("Multi0"));
        assert_eq!(player.color, Some(Color::from_argb(255, 200, 32, 32)));
        assert_eq!(player.faction.as_deref(), Some("england"));
        assert_eq!(player.team, 1);
        assert_eq!(setup.bots.len(), 1);
        assert_eq!(setup.bots[0].bot_type, "normal");
        assert_eq!(setup.bots[0].slot.spawn_point, 2);
    }
}
