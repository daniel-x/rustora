// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Parses the replay corpus recorded by the C# reference.

use std::path::PathBuf;

use rustora_net::order::Packet;
use rustora_net::replay::Replay;

#[test]
fn corpus_replays_parse_and_round_trip() {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let Ok(dir) = std::fs::read_dir(home.join("testdata/replays")) else {
        return;
    };
    let mut n = 0;
    for e in dir.flatten() {
        let data = std::fs::read(e.path()).unwrap();
        let replay = Replay::parse(&data).unwrap();
        assert!(replay.is_valid, "{:?}", e.path());
        let lobby = replay.lobby_info.as_ref().unwrap();
        assert!(lobby.clients.iter().any(|c| c.bot.is_some()));
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
