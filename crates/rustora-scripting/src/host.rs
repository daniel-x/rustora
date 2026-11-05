// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The parts of the script globals that reach outside the simulation (messages, camera, movies, the user
//! interface). The client installs a [`ScriptHost`]; without one, as in the headless binaries, the defaults apply.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::Lua;
use rustora_core::fieldloader::Color;
use rustora_core::world_types::WPos;
use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;

/// A Fluent message argument passed from Lua.
#[derive(Clone, Debug)]
pub enum MessageArg {
    Text(String),
    Number(f64),
}

pub trait ScriptHost {
    /// `FluentProvider.GetMessage` with the map messages.
    fn fluent_message(&self, key: &str, args: &[(String, MessageArg)]) -> String {
        let _ = args;
        key.to_string()
    }

    /// `World.RenderPlayer ?? World.LocalPlayer`, which only the client knows.
    fn render_or_local_player(&self, world: &World) -> Option<PlayerRef> {
        let _ = world;
        None
    }

    /// `Player.ResolvedPlayerName`.
    fn resolved_player_name(&self, world: &World, player: &PlayerRef) -> String {
        let _ = world;
        player.bot_type.clone().unwrap_or_else(|| player.player_name.clone())
    }

    /// `TextNotificationsManager.AddMissionLine` and friends: `kind` is `Mission`, `System`, `Debug` or `Chat`.
    fn add_text_line(&self, kind: &str, prefix: &str, text: &str, color: Option<Color>) {
        let _ = (kind, prefix, text, color);
    }

    fn camera_position(&self) -> Option<WPos> {
        None
    }

    fn set_camera_position(&self, pos: WPos) {
        let _ = pos;
    }

    /// `Media.PlayMovieFullscreen` and `PlayMovieInRadar`: plays `movie` and calls `on_complete` when it ends or
    /// cannot be played.
    fn play_movie(&self, movie: &str, fullscreen: bool, on_complete: Box<dyn FnOnce()>) {
        let _ = (movie, fullscreen);
        on_complete();
    }

    /// `Media.PlayMusic`: plays `track`, or the next song without one, and calls `on_complete` when it ends.
    /// Without available music nothing plays and `on_complete` is never called.
    fn play_music(&self, track: Option<&str>, on_complete: Box<dyn FnOnce()>) {
        let _ = (track, on_complete);
    }

    /// `Media.StopMusic` and `SetBackgroundMusic`.
    fn music(&self, command: &str, track: Option<&str>) {
        let _ = (command, track);
    }

    /// `Media.FloatingText`.
    fn floating_text(&self, pos: WPos, color: Color, text: &str, duration: i32) {
        let _ = (pos, color, text, duration);
    }

    /// The `Beacon` effect of `Beacon.New`.
    fn beacon(&self, owner: &PlayerRef, pos: WPos, duration: i32) {
        let _ = (owner, pos, duration);
    }

    /// `Lighting.Flash`.
    fn flash(&self, ty: Option<&str>, ticks: i32) {
        let _ = (ty, ticks);
    }

    /// The red, green, blue and ambient values of the `TintPostProcessEffect`, if there is one.
    fn tint(&self) -> Option<[f32; 4]> {
        None
    }

    fn set_tint(&self, tint: [f32; 4]) {
        let _ = tint;
    }

    /// `UserInterface.SetMissionText`.
    fn set_mission_text(&self, text: &str, color: Option<Color>) {
        let _ = (text, color);
    }
}

struct DefaultHost;

impl ScriptHost for DefaultHost {}

thread_local! {
    static HOST: RefCell<Rc<dyn ScriptHost>> = RefCell::new(Rc::new(DefaultHost));
}

/// Installs the host of the scripts of this thread.
pub fn set_host(host: Rc<dyn ScriptHost>) {
    HOST.with(|h| *h.borrow_mut() = host);
}

pub fn host() -> Rc<dyn ScriptHost> {
    HOST.with(|h| h.borrow().clone())
}

pub fn resolved_player_name(lua: &Lua, player: &PlayerRef) -> String {
    match crate::context::world(lua) {
        Ok(world) => host().resolved_player_name(&world, player),
        Err(_) => player.player_name.clone(),
    }
}

/// `Platform.EngineDir`.
pub fn engine_dir() -> String {
    let mut dir = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    if !dir.ends_with(std::path::MAIN_SEPARATOR) {
        dir.push(std::path::MAIN_SEPARATOR);
    }
    dir
}
