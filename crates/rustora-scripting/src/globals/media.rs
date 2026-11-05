// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `MediaGlobal.cs`, `UserInterfaceGlobal.cs`, `CameraGlobal.cs`, `BeaconGlobal.cs`, `RadarGlobal.cs`,
//! `LightingGlobal.cs` and `ColorGlobal.cs`. The parts outside the simulation go through the [`ScriptHost`].
//!
//! [`ScriptHost`]: crate::host::ScriptHost

use std::rc::{Rc, Weak};

use mlua::{Function, Lua, Table, Value};
use rustora_core::fieldloader::Color;
use rustora_core::world_types::WPos;
use rustora_engine::player::PlayerRef;
use rustora_engine::sound::SoundType;
use rustora_mods_common::radar::{PingVisibility, RadarPings};

use crate::context::{ScriptContext, ctx, world};
use crate::host::{MessageArg, host};
use crate::members::Group;
use crate::values::{FromScript, LuaColor, LuaResult, LuaWPos, error, type_name};

const WHITE: Color = Color::from_argb(255, 255, 255, 255);

/// `WrapOnPlayComplete`: a callback that calls `f` once and reports errors as fatal.
fn on_complete(context: Weak<ScriptContext>, f: Option<Function>) -> Box<dyn FnOnce()> {
    Box::new(move || {
        if let (Some(c), Some(f)) = (context.upgrade(), f) {
            c.call(&f, ());
        }
    })
}

fn notification(lua: &Lua, player: Option<PlayerRef>, kind: &str, name: &str) -> LuaResult<()> {
    let w = world(lua)?;
    let faction = player.as_ref().map(|p| p.faction.clone());
    rustora_engine::sound::play_notification(&w, player.as_ref(), kind, name, faction.as_deref());
    Ok(())
}

fn display_message(text: Option<String>, prefix: String, color: Option<Color>) {
    let Some(text) = text.filter(|t| !t.is_empty()) else { return };
    host().add_text_line("Mission", &prefix, &text, Some(color.unwrap_or(WHITE)));
}

/// A `Color?` parameter: omission gives `None`, an explicit `nil` is rejected as for nullable value types.
fn opt_color(a: &crate::values::Args, i: usize) -> LuaResult<Option<Color>> {
    match a.raw(i) {
        None | Some(mlua::Value::Nil) => Ok(None),
        Some(v) => match Color::from_script(v) {
            Some(c) => Ok(Some(c)),
            None => error(format!("Unable to convert parameter {i} to Color")),
        },
    }
}

pub fn media() -> Group<()> {
    Group::new("Media", &[])
        .method("PlaySpeechNotification", |lua, _, a| {
            notification(lua, a.get(0)?, "Speech", &a.get::<String>(1)?)
        })
        .method("PlaySoundNotification", |lua, _, a| {
            notification(lua, a.get(0)?, "Sounds", &a.get::<String>(1)?)
        })
        .method("PlaySound", |lua, _, a| {
            let file: String = a.get(0)?;
            rustora_engine::sound::play(&*world(lua)?, SoundType::World, &file, 1.0);
            Ok(())
        })
        .method("PlayMusic", |lua, _, a| {
            let track: Option<String> = a.opt(0, None)?;
            let f: Option<Function> = a.opt(1, None)?;
            let done = on_complete(Rc::downgrade(&ctx(lua)?), f);
            host().play_music(track.as_deref().filter(|t| !t.is_empty()), done);
            Ok(())
        })
        .method("SetBackgroundMusic", |_, _, a| {
            let track: Option<String> = a.opt(0, None)?;
            host().music("SetBackgroundMusic", track.as_deref().filter(|t| !t.is_empty()));
            Ok(())
        })
        .method("StopMusic", |_, _, _| {
            host().music("StopMusic", None);
            Ok(())
        })
        .method("PlayMovieFullscreen", |lua, _, a| play_movie(lua, a, true))
        .method("PlayMovieInRadar", |lua, _, a| play_movie(lua, a, false))
        .method("DisplayMessage", |_, _, a| {
            display_message(a.get(0)?, a.opt(1, "Mission".to_string())?, opt_color(a, 2)?);
            Ok(())
        })
        .method("DisplayMessageToPlayer", |lua, _, a| {
            let player: Option<PlayerRef> = a.get(0)?;
            let local = world(lua)?.sound.local_player();
            let same = match (&local, &player) {
                (Some(l), Some(p)) => Rc::ptr_eq(l, p),
                (None, None) => true,
                _ => false,
            };
            if same {
                display_message(a.get(1)?, a.opt(2, "Mission".to_string())?, opt_color(a, 3)?);
            }
            Ok(())
        })
        .method("DisplaySystemMessage", |_, _, a| {
            let text: Option<String> = a.get(0)?;
            let prefix: Option<String> = a.opt(1, None)?;
            if let Some(text) = text.filter(|t| !t.is_empty()) {
                host().add_text_line("System", prefix.as_deref().unwrap_or(""), &text, None);
            }
            Ok(())
        })
        .method("Debug", |_, _, a| {
            let text: Option<String> = a.get(0)?;
            if let Some(text) = text.filter(|t| !t.is_empty()) {
                host().add_text_line("Debug", "", &text, None);
            }
            Ok(())
        })
        .method("FloatingText", |lua, _, a| {
            let text: Option<String> = a.get(0)?;
            let position: WPos = a.get(1)?;
            let duration: i32 = a.opt(2, 30)?;
            let color = opt_color(a, 3)?.unwrap_or(WHITE);
            let w = world(lua)?;
            let Some(text) = text.filter(|t| !t.is_empty()) else {
                return Ok(());
            };
            if !w.map().contains(w.map().cell_containing(position)) {
                return Ok(());
            }
            host().floating_text(position, color, &text, duration);
            Ok(())
        })
}

fn play_movie(lua: &Lua, a: &crate::values::Args, fullscreen: bool) -> LuaResult<()> {
    let movie: String = a.get(0)?;
    let f: Option<Function> = a.opt(1, None)?;
    let context = Rc::downgrade(&ctx(lua)?);
    let done = on_complete(context, f);
    host().play_movie(&movie, fullscreen, done);
    Ok(())
}

pub fn user_interface() -> Group<()> {
    Group::new("UserInterface", &[])
        .method("SetMissionText", |_, _, a| {
            let text: Option<String> = a.get(0)?;
            host().set_mission_text(text.as_deref().unwrap_or(""), Some(opt_color(a, 1)?.unwrap_or(WHITE)));
            Ok(())
        })
        .method("GetFluentMessage", |_, _, a| {
            let key: String = a.get(0)?;
            let args: Option<Table> = a.opt(1, None)?;
            let mut list = Vec::new();
            if let Some(t) = args {
                for pair in t.pairs::<Value, Value>() {
                    let (k, v) = pair?;
                    let value = match &v {
                        Value::Integer(i) => Some(MessageArg::Number(*i as f64)),
                        Value::Number(n) => Some(MessageArg::Number(*n)),
                        Value::String(s) => Some(MessageArg::Text(s.to_string_lossy())),
                        Value::Boolean(b) => Some(MessageArg::Text(if *b { "True" } else { "False" }.into())),
                        _ => None,
                    };
                    match (&k, value) {
                        (Value::String(k), Some(value)) => list.push((k.to_string_lossy(), value)),
                        _ => {
                            return error(format!(
                                "String arguments requires a table of [\"string\"]=value pairs. Received {},{}",
                                type_name(&k),
                                type_name(&v)
                            ));
                        }
                    }
                }
            }
            Ok(host().fluent_message(&key, &list))
        })
}

pub fn camera() -> Group<()> {
    Group::new("Camera", &[]).get_set(
        "Position",
        |_, _| Ok(LuaWPos(host().camera_position().unwrap_or(WPos::ZERO))),
        |_, _, pos: WPos| {
            host().set_camera_position(pos);
            Ok(())
        },
    )
}

fn radar_pings(lua: &Lua) -> LuaResult<Option<Rc<RadarPings>>> {
    Ok(world(lua)?.world_actor().trait_or_default::<RadarPings>())
}

pub fn beacon() -> Group<()> {
    Group::new("Beacon", &[]).method("New", |lua, _, a| {
        let owner: PlayerRef = a.get(0)?;
        let position: WPos = a.get(1)?;
        let duration: i32 = a.opt(2, 750)?;
        let show_pings: bool = a.opt(3, true)?;
        if !owner.player_actor().info.has_trait_info_assignable_to("PlaceBeaconInfo") {
            return error("The player actor has no 'PlaceBeacon' trait.");
        }
        host().beacon(&owner, position, duration);
        if show_pings && let Some(pings) = radar_pings(lua)? {
            let viewer = world(lua)?.sound.local_player();
            pings.add(
                PingVisibility::AlliesOf(owner.clone()),
                viewer.as_ref(),
                position,
                owner.color,
                duration,
            );
        }
        Ok(())
    })
}

pub fn radar() -> Group<()> {
    Group::new("Radar", &[]).method("Ping", |lua, _, a| {
        let player: PlayerRef = a.get(0)?;
        let position: WPos = a.get(1)?;
        let color: Color = a.get(2)?;
        let duration: i32 = a.opt(3, 750)?;
        if let Some(pings) = radar_pings(lua)? {
            let viewer = world(lua)?.sound.local_player();
            pings.add(PingVisibility::Only(player), viewer.as_ref(), position, color, duration);
        }
        Ok(())
    })
}

pub fn lighting() -> Group<()> {
    let mut g = Group::new("Lighting", &[]).method("Flash", |_, _, a| {
        let ty: Option<String> = a.opt(0, None)?;
        let ticks: i32 = a.opt(1, -1)?;
        host().flash(ty.as_deref(), ticks);
        Ok(())
    });
    for (i, name) in ["Red", "Green", "Blue", "Ambient"].into_iter().enumerate() {
        g = g.get_set(
            name,
            move |_, _| Ok(host().tint().map_or(1.0, |t| t[i] as f64)),
            move |_, _, v: f64| {
                if let Some(mut t) = host().tint() {
                    t[i] = v as f32;
                    host().set_tint(t);
                }
                Ok(())
            },
        );
    }
    g
}

/// `Color.HsvToRgb`.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let px = (h * 6.0 - 3.0).abs();
    let py = ((h + 2.0 / 3.0) % 1.0 * 6.0 - 3.0).abs();
    let pz = ((h + 1.0 / 3.0) % 1.0 * 6.0 - 3.0).abs();
    (
        v * lerp(1.0, (px - 1.0).clamp(0.0, 1.0), s),
        v * lerp(1.0, (py - 1.0).clamp(0.0, 1.0), s),
        v * lerp(1.0, (pz - 1.0).clamp(0.0, 1.0), s),
    )
}

/// `Color.FromAhsl`.
pub fn from_ahsl(alpha: u8, h: f32, s: f32, l: f32) -> Color {
    let v = l + s * l.min(1.0 - l);
    let sv = if v > 0.0 { 2.0 * (1.0 - l / v) } else { 0.0 };
    let (r, g, b) = hsv_to_rgb(h, sv, v);
    let byte = |x: f32| ((255.0 * x) as f64).round_ties_even() as u8;
    Color::from_argb(alpha, byte(r), byte(g), byte(b))
}

const NAMED_COLORS: &[(&str, u32)] = &[
    ("Aqua", 0xFF00FFFF),
    ("Black", 0xFF000000),
    ("Blue", 0xFF0000FF),
    ("Brown", 0xFFA52A2A),
    ("Cyan", 0xFF00FFFF),
    ("DarkBlue", 0xFF00008B),
    ("DarkCyan", 0xFF008B8B),
    ("DarkGray", 0xFFA9A9A9),
    ("DarkGreen", 0xFF006400),
    ("DarkOrange", 0xFFFF8C00),
    ("DarkRed", 0xFF8B0000),
    ("Fuchsia", 0xFFFF00FF),
    ("Gold", 0xFFFFD700),
    ("Gray", 0xFF808080),
    ("Green", 0xFF008000),
    ("LawnGreen", 0xFF7CFC00),
    ("LightBlue", 0xFFADD8E6),
    ("LightCyan", 0xFFE0FFFF),
    ("LightGray", 0xFFD3D3D3),
    ("LightGreen", 0xFF90EE90),
    ("LightYellow", 0xFFFFFFE0),
    ("Lime", 0xFF00FF00),
    ("LimeGreen", 0xFF32CD32),
    ("Magenta", 0xFFFF00FF),
    ("Maroon", 0xFF800000),
    ("Navy", 0xFF000080),
    ("Olive", 0xFF808000),
    ("Orange", 0xFFFFA500),
    ("OrangeRed", 0xFFFF4500),
    ("Purple", 0xFF800080),
    ("Red", 0xFFFF0000),
    ("Salmon", 0xFFFA8072),
    ("SkyBlue", 0xFF87CEEB),
    ("Teal", 0xFF008080),
    ("Yellow", 0xFFFFFF00),
    ("White", 0xFFFFFFFF),
];

fn argb(v: u32) -> Color {
    Color::from_argb((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn hsl_color() -> Group<()> {
    let mut g = Group::new("HSLColor", &[])
        .method("New", |_, _, a| {
            let h = a.get::<i32>(0)?.clamp(0, 255) as f32;
            let s = a.get::<i32>(1)?.clamp(0, 255) as f32;
            let l = a.get::<i32>(2)?.clamp(0, 255) as f32;
            Ok(LuaColor(from_ahsl(255, h / 255.0, s / 255.0, l / 255.0)))
        })
        .method("FromRGB", |_, _, a| {
            let c = |v: i32| v.clamp(0, 255) as u8;
            let r = c(a.get(0)?);
            let g = c(a.get(1)?);
            let b = c(a.get(2)?);
            let alpha = c(a.opt(3, 255)?);
            Ok(LuaColor(Color::from_argb(alpha, r, g, b)))
        })
        .method("FromHex", |_, _, a| {
            let value: String = a.get(0)?;
            match Color::parse(&value) {
                Some(c) => Ok(LuaColor(c)),
                None => error("Invalid rrggbb[aa] hex string."),
            }
        });
    for &(name, value) in NAMED_COLORS {
        g = g.get(name, move |_, _| Ok(LuaColor(argb(value))));
    }
    g
}
