// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Dedicated rustora server, the counterpart of `OpenRA.Server`. It reads the RA mod from the OpenRA engine directory.
//! Usage: `rustora-server --engine-dir <dir> [--support-dir <dir>] [--mod ra] [--map <uid or file>] [--port 1234]
//! [--name <text>] [--password <text>] [--seed <int>] [--enable-singleplayer] [--enable-sync-reports]
//! [--advertise-online] [--discover-nat-devices] [--require-authentication] [--profile-whitelist <id,id>]
//! [--profile-blacklist <id,id>]`.

use std::path::PathBuf;

use rustora_server::mod_info::ServerMod;
use rustora_server::server::{Server, ServerSettings};

fn usage(problem: &str) -> ! {
    eprintln!("{problem}");
    eprintln!(
        "usage: rustora-server --engine-dir <dir> [--support-dir <dir>] [--mod ra] [--map <uid or file>] [--port 1234] \
         [--name <text>] [--password <text>] [--seed <int>] [--enable-singleplayer] [--enable-sync-reports] \
         [--advertise-online] [--discover-nat-devices] [--require-authentication] [--profile-whitelist <id,id>] \
         [--profile-blacklist <id,id>]"
    );
    std::process::exit(2);
}

fn parse_ids(text: &str) -> Vec<i32> {
    text.split(',')
        .map(|s| s.trim().parse().unwrap_or_else(|_| usage("invalid profile id")))
        .collect()
}

fn main() {
    let mut engine = None;
    let mut support = None;
    let mut mod_id = "ra".to_string();
    let mut settings = ServerSettings::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage(&format!("{a} needs a value")));
        match a.as_str() {
            "--engine-dir" => engine = Some(PathBuf::from(value())),
            "--support-dir" => support = Some(value()),
            "--mod" => mod_id = value(),
            "--map" => settings.map = Some(value()),
            "--port" => settings.listen_port = value().parse().unwrap_or_else(|_| usage("invalid port")),
            "--name" => settings.name = value(),
            "--password" => settings.password = value(),
            "--seed" => settings.seed = Some(value().parse().unwrap_or_else(|_| usage("invalid seed"))),
            "--enable-singleplayer" => settings.enable_singleplayer = true,
            "--enable-sync-reports" => settings.enable_sync_reports = true,
            "--advertise-online" => settings.advertise_online = true,
            "--discover-nat-devices" => settings.discover_nat_devices = true,
            "--require-authentication" => settings.require_authentication = true,
            "--profile-whitelist" => settings.profile_id_whitelist = parse_ids(&value()),
            "--profile-blacklist" => settings.profile_id_blacklist = parse_ids(&value()),
            _ => usage(&format!("unknown argument {a}")),
        }
    }
    let engine = engine.unwrap_or_else(|| usage("--engine-dir is required"));
    let support = support.unwrap_or_else(|| engine.join("Support").to_string_lossy().into_owned());
    settings.support_dir = PathBuf::from(&support);
    println!("Starting dedicated server for mod: {mod_id}");
    loop {
        let result = ServerMod::load(&engine, &support, &mod_id)
            .and_then(|game_mod| Server::new(settings.clone(), game_mod))
            .map(|mut server| server.run());
        if let Err(e) = result {
            eprintln!("server failed: {e}");
            std::process::exit(1);
        }
        println!("Starting a new server instance...");
    }
}
