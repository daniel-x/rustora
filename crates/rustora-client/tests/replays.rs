// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Parses the replay corpus recorded by the C# reference. Mission runs (prefixes m, n and k) have no bots.

use std::path::PathBuf;

use rustora_client::replay::Replay;
use rustora_net::order::Packet;

/// The replays of the outer project's `testdata/replays`, when present.
#[test]
fn local_replays_parse_and_round_trip() {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    if let Ok(dir) = std::fs::read_dir(home.join("testdata/replays")) {
        check_replays(dir);
    }
}

/// The committed corpus in `testdata/corpus` of this repository.
#[test]
fn committed_corpus_parses_and_round_trips() {
    let dir = std::fs::read_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/corpus")).unwrap();
    check_replays(dir);
}

fn check_replays(dir: std::fs::ReadDir) {
    let mut n = 0;
    for e in dir.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "orarep")) {
        let data = std::fs::read(e.path()).unwrap();
        let replay = Replay::parse(&data).unwrap();
        assert!(replay.is_valid, "{:?}", e.path());
        let lobby = replay.lobby_info.as_ref().unwrap();
        let is_mission = e.file_name().to_string_lossy().starts_with(['m', 'n', 'k', 'j', 'p']);
        assert!(is_mission || lobby.clients.iter().any(|c| c.bot.is_some()));
        assert!(replay.metadata.is_some());
        for c in &replay.chunks {
            for p in &c.packets {
                if let Ok(Packet::Orders(op)) = Packet::parse(p.client_id, &p.data) {
                    assert_eq!(
                        Packet::serialize_orders(op.frame, &op.orders),
                        p.data,
                        "re-serialized packet differs"
                    );
                }
            }
        }
        n += 1;
    }
    assert!(n > 0);
}
