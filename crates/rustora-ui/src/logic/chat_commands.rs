// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `ChatCommands.cs`, `HelpCommand.cs`, `PlayerCommands.cs`, `DevCommands.cs` and the command of
//! `TerrainGeometryOverlay`. The commands of the other debug overlays (`DebugVisualizationCommands` and the overlay
//! traits) are not registered, since those overlays are not ported.

use std::collections::BTreeMap;
use std::rc::Rc;

use rustora_engine::order::Order;
use rustora_engine::player::WinState;
use rustora_engine::target::Target;
use rustora_engine::world::World;
use rustora_mods_common::base::DeveloperMode;
use rustora_mods_common::experience::GainsExperienceInfo;

use super::ingame::Ingame;
use super::ingame_menu::set_pause_state;
use super::{message, message_with, services};
use crate::fluent::MessageArg;
use crate::net::{SYSTEM_CLIENT_ID, TextNotification, TextNotificationPool};
use crate::widget::Ui;

#[derive(Clone, Copy)]
enum Command {
    Help,
    Pause,
    Surrender,
    TerrainGeometry,
    Dev(DevCommand),
}

#[derive(Clone, Copy)]
enum DevCommand {
    Order(&'static str),
    Cash(&'static str),
    Crash,
    LevelUp,
    PlayerExperience,
    PowerOutage,
    Kill,
    Dispose,
}

const DEV_COMMANDS: &[(&str, &str, DevCommand)] = &[
    ("visibility", "description-toggle-visibility", DevCommand::Order("DevVisibility")),
    ("give-cash", "description-give-cash", DevCommand::Cash("DevGiveCash")),
    ("give-cash-all", "description-give-cash-all", DevCommand::Cash("DevGiveCashAll")),
    ("instant-build", "description-instant-building", DevCommand::Order("DevFastBuild")),
    (
        "build-anywhere",
        "description-build-anywhere",
        DevCommand::Order("DevBuildAnywhere"),
    ),
    (
        "unlimited-power",
        "description-unlimited-power",
        DevCommand::Order("DevUnlimitedPower"),
    ),
    ("enable-tech", "description-enable-tech", DevCommand::Order("DevEnableTech")),
    ("fast-charge", "description-fast-charge", DevCommand::Order("DevFastCharge")),
    ("all", "description-dev-cheat-all", DevCommand::Order("DevAll")),
    ("crash", "description-dev-crash", DevCommand::Crash),
    ("levelup", "description-levelup-actor", DevCommand::LevelUp),
    ("player-experience", "description-player-experience", DevCommand::PlayerExperience),
    ("power-outage", "description-power-outage", DevCommand::PowerOutage),
    ("kill", "description-kill-selected-actors", DevCommand::Kill),
    ("dispose", "description-dispose-selected-actors", DevCommand::Dispose),
];

/// The registered commands with their help message keys, as `ChatCommands.Commands` and
/// `HelpCommand.helpDescriptions` hold them after `WorldLoaded`.
fn commands(world: &World) -> BTreeMap<&'static str, (Command, &'static str)> {
    let info = &world.world_actor().info;
    let mut commands = BTreeMap::new();
    if info.has_trait_info_named("HelpCommand") {
        commands.insert("help", (Command::Help, "description-help-description"));
    }
    if info.has_trait_info_named("PlayerCommands") {
        commands.insert("pause", (Command::Pause, "description-pause-description"));
        commands.insert("surrender", (Command::Surrender, "description-surrender-description"));
    }
    if info.has_trait_info_named("TerrainGeometryOverlay") {
        commands.insert("terrain-geometry", (Command::TerrainGeometry, "description-terrain-geometry-overlay"));
    }
    if info.has_trait_info_named("DevCommands") {
        for &(name, description, command) in DEV_COMMANDS {
            commands.insert(name, (Command::Dev(command), description));
        }
    }
    commands
}

/// `TextNotificationsManager.Debug`.
pub fn add_debug_line(ui: &Ui, text: String) {
    services(ui).net.borrow_mut().notifications.push(TextNotification {
        pool: TextNotificationPool::System,
        client_id: SYSTEM_CLIENT_ID,
        prefix: Some("Debug".to_string()),
        text,
        prefix_color: None,
        text_color: None,
        time: crate::net::local_time(),
    });
}

/// `ChatCommands.OnChat` for a chat line that starts with `/`.
pub fn on_chat(ui: &mut Ui, ingame: &Rc<Ingame>, text: &str) {
    let Some(rest) = text.strip_prefix('/') else { return };
    if !ingame.world.world_actor().info.has_trait_info_named("ChatCommands") {
        return;
    }
    let name = rest.split(' ').next().unwrap_or("").to_lowercase();
    let arg = rest.get(name.len()..).unwrap_or("").trim();
    let commands = commands(&ingame.world);
    let Some(&(command, _)) = commands.get(name.as_str()) else {
        let text = message_with(ui, "notification-invalid-command", &[("name", MessageArg::Text(&name))]);
        add_debug_line(ui, text);
        return;
    };
    match command {
        Command::Help => {
            add_debug_line(ui, message(ui, "notification-available-commands"));
            for (key, (_, description)) in &commands {
                let text = format!("{key}: {}", message(ui, description));
                add_debug_line(ui, text);
            }
        }
        Command::TerrainGeometry => ingame.terrain_geometry.set(!ingame.terrain_geometry.get()),
        Command::Pause => {
            let world = &ingame.world;
            let is_host = services(ui).net.borrow().is_host();
            if is_host || ingame.local_player.as_ref().is_some_and(|p| p.win_state.get() != WinState::Lost) {
                set_pause_state(world, !world.paused());
            }
        }
        Command::Surrender => {
            if let Some(player) = &ingame.local_player {
                ingame
                    .world
                    .issue_order(Order::new("Surrender", Some(player.player_actor().clone()), Target::Invalid, false));
            }
        }
        Command::Dev(command) => invoke_dev_command(ui, ingame, command, arg),
    }
}

/// `DevCommands.InvokeCommand`.
fn invoke_dev_command(ui: &mut Ui, ingame: &Ingame, command: DevCommand, arg: &str) {
    let Some(player) = &ingame.local_player else { return };
    let player_actor = player.player_actor().clone();
    if !player_actor.trait_or_default::<DeveloperMode>().is_some_and(|d| d.enabled()) {
        add_debug_line(ui, message(ui, "notification-cheats-disabled"));
        return;
    }
    let world = &ingame.world;
    let selected = || ingame.selection.actors();
    let selected_owners = || {
        let mut owners: Vec<_> = Vec::new();
        for actor in selected() {
            let owner = actor.owner().player_actor().clone();
            if !owners.iter().any(|o| Rc::ptr_eq(o, &owner)) {
                owners.push(owner);
            }
        }
        owners
    };
    match command {
        DevCommand::Order(order) => world.issue_order(Order::new(order, Some(player_actor), Target::Invalid, false)),
        DevCommand::Cash(order) => {
            let cash = if arg.is_empty() { Some(0) } else { arg.parse::<i32>().ok() };
            match cash {
                Some(cash) => {
                    let mut o = Order::new(order, Some(player_actor), Target::Invalid, false);
                    o.extra_data = cash as u32;
                    world.issue_order(o);
                }
                None => add_debug_line(ui, message(ui, "notification-invalid-cash-amount")),
            }
        }
        DevCommand::Crash => panic!("DevException"),
        DevCommand::LevelUp => {
            for actor in selected() {
                if actor.is_dead() {
                    continue;
                }
                let mut o = Order::new("DevLevelUp", Some(actor.clone()), Target::Invalid, false);
                if let Ok(level) = arg.parse::<i32>() {
                    o.extra_data = level as u32;
                }
                if actor.info.has_trait_info::<GainsExperienceInfo>() {
                    world.issue_order(o);
                }
            }
        }
        DevCommand::PlayerExperience => {
            let Ok(experience) = arg.parse::<i32>() else { return };
            for owner in selected_owners() {
                let mut o = Order::new("DevPlayerExperience", Some(owner), Target::Invalid, false);
                o.extra_data = experience as u32;
                world.issue_order(o);
            }
        }
        DevCommand::PowerOutage => {
            for owner in selected_owners() {
                let mut o = Order::new("PowerOutage", Some(owner), Target::Invalid, false);
                o.extra_data = 250;
                world.issue_order(o);
            }
        }
        DevCommand::Kill => {
            for actor in selected() {
                if actor.is_dead() {
                    continue;
                }
                let mut o = Order::new("DevKill", Some(player_actor.clone()), Target::from_actor(Some(&actor)), false);
                o.target_string = Some(arg.to_string());
                world.issue_order(o);
            }
        }
        DevCommand::Dispose => {
            for actor in selected() {
                if actor.disposed() {
                    continue;
                }
                world.issue_order(Order::new(
                    "DevDispose",
                    Some(player_actor.clone()),
                    Target::from_actor(Some(&actor)),
                    false,
                ));
            }
        }
    }
}
