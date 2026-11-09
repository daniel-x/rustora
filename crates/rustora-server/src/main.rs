// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The rustora game server, the counterpart of `OpenRA.Server`. It reads the RA mod from the OpenRA engine directory,
//! by default the one the client finds too, and it uses the support directory of OpenRA by default.
//! By default it is independent: it runs on its own and starts a new server instance after each game, like a C#
//! dedicated server. `--dependent <local|skirmish|multiplayer>` makes it the server that a client starts for its
//! game, like `Game.CreateLocalServer` and `Game.CreateServer`: it serves one game, and it exits when its standard
//! input closes, which happens when the client is gone.
//! Usage: `rustora-server [--engine-dir <dir>] [--support-dir <dir>] [--mod ra] [--map <uid or file>] [--port 1234]
//! [--name <text>] [--password <text>] [--seed <int>] [--dependent <local|skirmish|multiplayer>]
//! [--enable-singleplayer] [--enable-sync-reports] [--advertise-online] [--discover-nat-devices]
//! [--require-authentication] [--profile-whitelist <id,id>] [--profile-blacklist <id,id>]
//! [--player-profile-url <url>] [--server-advertise-url <url>]`.

use std::path::PathBuf;

use rustora_core::log_channels;
use rustora_server::mod_info::ServerMod;
use rustora_server::server::{Server, ServerSettings, ServerType};

fn usage(problem: &str) -> ! {
    eprintln!("{problem}");
    eprintln!(
        "usage: rustora-server [--engine-dir <dir>] [--support-dir <dir>] [--mod ra] [--map <uid or file>] [--port 1234] \
         [--name <text>] [--password <text>] [--seed <int>] [--dependent <local|skirmish|multiplayer>] \
         [--enable-singleplayer] [--enable-sync-reports] [--advertise-online] [--discover-nat-devices] [--require-authentication] [--profile-whitelist <id,id>] \
         [--profile-blacklist <id,id>] [--player-profile-url <url>] [--server-advertise-url <url>]"
    );
    std::process::exit(2);
}

fn parse_ids(text: &str) -> Vec<i32> {
    text.split(',')
        .map(|s| s.trim().parse().unwrap_or_else(|_| usage("invalid profile id")))
        .collect()
}

/// Exits the process once standard input reaches its end, which happens when the parent process is gone.
fn exit_on_stdin_close() {
    std::thread::spawn(|| {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
        log::logger().flush();
        std::process::exit(0);
    });
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
            "--dependent" => {
                settings.server_type = match value().as_str() {
                    "local" => ServerType::Local,
                    "skirmish" => ServerType::Skirmish,
                    "multiplayer" => ServerType::Multiplayer,
                    _ => usage("--dependent needs local, skirmish or multiplayer"),
                };
                exit_on_stdin_close();
            }
            "--enable-sync-reports" => settings.enable_sync_reports = true,
            "--advertise-online" => settings.advertise_online = true,
            "--discover-nat-devices" => settings.discover_nat_devices = true,
            "--require-authentication" => settings.require_authentication = true,
            "--profile-whitelist" => settings.profile_id_whitelist = parse_ids(&value()),
            "--profile-blacklist" => settings.profile_id_blacklist = parse_ids(&value()),
            "--player-profile-url" => settings.player_profile_url = value(),
            "--server-advertise-url" => settings.server_advertise_url = value(),
            _ => usage(&format!("unknown argument {a}")),
        }
    }
    let engine = engine
        .or_else(rustora_fs::Platform::find_engine_dir)
        .unwrap_or_else(|| usage("no RA mod data (mods/ra/mod.yaml) found next to the executable; pass --engine-dir"));
    let support = support.unwrap_or_else(|| rustora_fs::Platform::user_support_dir(&engine).to_string_lossy().into_owned());
    settings.support_dir = PathBuf::from(&support);
    let independent = settings.server_type == ServerType::Independent;
    let channels = if independent {
        log_channels::INDEPENDENT_SERVER_CHANNELS
    } else {
        log_channels::DEPENDENT_SERVER_CHANNELS
    };
    log_channels::init(&settings.support_dir, channels);
    log_channels::install_panic_hook(&settings.support_dir);
    if independent {
        println!("Starting independent server for mod: {mod_id}");
    }
    loop {
        let result = ServerMod::load(&engine, &support, &mod_id)
            .and_then(|game_mod| Server::new(settings.clone(), game_mod))
            .map(|mut server| server.run());
        if let Err(e) = result {
            log::error!(target: "server", "server failed: {e}");
            log::logger().flush();
            eprintln!("server failed: {e}");
            std::process::exit(1);
        }
        if !independent {
            log::logger().flush();
            return;
        }
        println!("Starting a new server instance...");
    }
}
