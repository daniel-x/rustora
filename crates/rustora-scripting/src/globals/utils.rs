// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `UtilsGlobal.cs`, `DateTimeGlobal.cs`, `CoordinateGlobals.cs` and `AngleGlobal.cs`.

use mlua::{Function, Lua, Table, Value};
use rustora_core::world_types::{CPos, CVec, WAngle, WDist, WPos, WVec};
use rustora_mods_common::player::objectives::TimeLimitManager;

use crate::context::world;
use crate::members::Group;
use crate::values::{LuaCPos, LuaCVec, LuaResult, LuaWAngle, LuaWDist, LuaWPos, LuaWVec, error};

/// The timestep of the default game speed of the mod, as `DateGlobal` reads it from the manifest.
const DEFAULT_TIMESTEP: i32 = 40;
const TICKS_PER_SECOND: i32 = 1000 / DEFAULT_TIMESTEP;

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Nil | Value::Boolean(false))
}

fn first_result(f: &Function, c: &Value) -> LuaResult<bool> {
    let ret: mlua::MultiValue = f.call(c.clone())?;
    Ok(ret.front().is_some_and(truthy))
}

fn sequence(lua: &Lua, values: impl IntoIterator<Item = Value>) -> LuaResult<Table> {
    lua.create_sequence_from(values)
}

/// `WidgetUtils.FormatTime`.
pub fn format_time(ticks: i32, leading_minute_zero: bool, timestep: i32) -> String {
    let seconds = (ticks.wrapping_mul(timestep) as f32 / 1000.0).ceil() as i32;
    let minutes = seconds / 60;
    if minutes >= 60 {
        format!("{}:{:02}:{:02}", minutes / 60, minutes % 60, seconds % 60)
    } else if leading_minute_zero {
        format!("{minutes:02}:{:02}", seconds % 60)
    } else {
        format!("{minutes}:{:02}", seconds % 60)
    }
}

pub fn utils() -> Group<()> {
    Group::new("Utils", &[])
        .method("Do", |_, _, a| {
            let collection: Vec<Value> = a.get(0)?;
            let f: Function = a.get(1)?;
            for c in collection {
                f.call::<()>(c)?;
            }
            Ok(())
        })
        .method("Any", |_, _, a| {
            let collection: Vec<Value> = a.get(0)?;
            let f: Function = a.get(1)?;
            for c in &collection {
                if first_result(&f, c)? {
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .method("All", |_, _, a| {
            let collection: Vec<Value> = a.get(0)?;
            let f: Function = a.get(1)?;
            for c in &collection {
                if !first_result(&f, c)? {
                    return Ok(false);
                }
            }
            Ok(true)
        })
        .method("Where", |lua, _, a| {
            let collection: Vec<Value> = a.get(0)?;
            let f: Function = a.get(1)?;
            let mut out = Vec::new();
            for c in collection {
                if first_result(&f, &c)? {
                    out.push(c);
                }
            }
            sequence(lua, out)
        })
        .method("Take", |lua, _, a| {
            let n: i32 = a.get(0)?;
            let source: Vec<Value> = a.get(1)?;
            sequence(lua, source.into_iter().take(n.max(0) as usize))
        })
        .method("Skip", |lua, _, a| {
            let table: Table = a.get(0)?;
            let n: i32 = a.get(1)?;
            let count = table.raw_len() as i32;
            let out = lua.create_table()?;
            for i in n..=count {
                let value: Value = table.get(i)?;
                out.set(out.raw_len() + 1, value)?;
            }
            Ok(out)
        })
        .method("Concat", |lua, _, a| {
            let first: Vec<Value> = a.get(0)?;
            let second: Vec<Value> = a.get(1)?;
            sequence(lua, first.into_iter().chain(second))
        })
        .method("Random", |lua, _, a| {
            let collection: Vec<Value> = a.get(0)?;
            if collection.is_empty() {
                return error("Collection must not be empty");
            }
            let i = world(lua)?.shared_random.borrow_mut().next_max(collection.len() as i32);
            Ok(collection[i as usize].clone())
        })
        .method("Shuffle", |lua, _, a| {
            let collection: Vec<Value> = a.get(0)?;
            let w = world(lua)?;
            let shuffled = rustora_core::world_types::shuffle(&collection, &mut w.shared_random.borrow_mut());
            sequence(lua, shuffled)
        })
        .method("ExpandFootprint", |lua, _, a| {
            let footprint: Vec<CPos> = a.get(0)?;
            let allow_diagonal: bool = a.get(1)?;
            let cells = rustora_mods_common::util::expand_footprint(&footprint, allow_diagonal);
            lua.create_sequence_from(cells.into_iter().map(LuaCPos))
        })
        .method("RandomInteger", |lua, _, a| {
            let low: i32 = a.get(0)?;
            let high: i32 = a.get(1)?;
            if high <= low {
                return Ok(low);
            }
            Ok(world(lua)?.shared_random.borrow_mut().next_range(low, high))
        })
        .method("FormatTime", |_, _, a| {
            let ticks: i32 = a.get(0)?;
            let leading: bool = a.opt(1, true)?;
            Ok(format_time(ticks, leading, 40))
        })
}

fn time_limit_manager(lua: &Lua) -> LuaResult<Option<std::rc::Rc<TimeLimitManager>>> {
    Ok(world(lua)?.world_actor().trait_or_default::<TimeLimitManager>())
}

fn now() -> chrono_lite::DateTime {
    chrono_lite::now()
}

pub fn date_time() -> Group<()> {
    Group::new("DateTime", &[])
        .get("IsHalloween", |_, _| {
            let d = now();
            Ok(d.month == 10 && d.day == 31)
        })
        .get("GameTime", |lua, _| Ok(world(lua)?.world_tick()))
        .method("Seconds", |_, _, a| Ok(a.get::<i32>(0)?.wrapping_mul(TICKS_PER_SECOND)))
        .get("CurrentYear", |_, _| Ok(now().year))
        .get("CurrentMonth", |_, _| Ok(now().month))
        .get("CurrentDay", |_, _| Ok(now().day))
        .get("CurrentHour", |_, _| Ok(now().hour))
        .get("CurrentMinute", |_, _| Ok(now().minute))
        .get("CurrentSecond", |_, _| Ok(now().second))
        .method("Minutes", |_, _, a| {
            Ok(a.get::<i32>(0)?.wrapping_mul(60).wrapping_mul(TICKS_PER_SECOND))
        })
        .get_set(
            "TimeLimit",
            |lua, _| Ok(time_limit_manager(lua)?.map_or(0, |t| t.time_limit.get())),
            |lua, _, value: i32| match time_limit_manager(lua)? {
                Some(t) => {
                    let game_time = world(lua)?.world_tick();
                    t.time_limit.set(if value == 0 { 0 } else { value.wrapping_add(game_time) });
                    Ok(())
                }
                None => error("Cannot set TimeLimit, TimeLimitManager trait is missing."),
            },
        )
        .get_set(
            "TimeLimitNotification",
            |lua, _| Ok(time_limit_manager(lua)?.map(|t| t.notification.borrow().clone())),
            |lua, _, value: Option<String>| match time_limit_manager(lua)? {
                Some(t) => {
                    *t.notification.borrow_mut() = value.unwrap_or_default();
                    Ok(())
                }
                None => error("Cannot set TimeLimitNotification, TimeLimitManager trait is missing."),
            },
        )
}

pub fn coordinates() -> Vec<Group<()>> {
    vec![
        Group::new("CPos", &[])
            .method("New", |_, _, a| Ok(LuaCPos(CPos::new(a.get(0)?, a.get(1)?))))
            .method("NewWithLayer", |lua, _, a| {
                let x: i32 = a.get(0)?;
                let y: i32 = a.get(1)?;
                let layer = a.get::<i32>(2)? as u8;
                if layer != 0 && !crate::globals::map::has_custom_layer(lua, layer)? {
                    return error(format!("Layer {layer} does not exist on this map."));
                }
                Ok(LuaCPos(CPos::with_layer(x, y, layer)))
            })
            .get("Zero", |_, _| Ok(LuaCPos(CPos::ZERO))),
        Group::new("CVec", &[])
            .method("New", |_, _, a| Ok(LuaCVec(CVec::new(a.get(0)?, a.get(1)?))))
            .get("Zero", |_, _| Ok(LuaCVec(CVec::ZERO))),
        Group::new("WPos", &[])
            .method("New", |_, _, a| Ok(LuaWPos(WPos::new(a.get(0)?, a.get(1)?, a.get(2)?))))
            .get("Zero", |_, _| Ok(LuaWPos(WPos::ZERO))),
        Group::new("WVec", &[])
            .method("New", |_, _, a| Ok(LuaWVec(WVec::new(a.get(0)?, a.get(1)?, a.get(2)?))))
            .get("Zero", |_, _| Ok(LuaWVec(WVec::ZERO))),
        Group::new("WDist", &[])
            .method("New", |_, _, a| Ok(LuaWDist(WDist::new(a.get(0)?))))
            .method("FromCells", |_, _, a| Ok(LuaWDist(WDist::from_cells(a.get(0)?)))),
    ]
}

pub fn angle() -> Group<()> {
    let mut g = Group::new("Angle", &[]);
    for (name, a) in [
        ("North", 0),
        ("NorthWest", 128),
        ("West", 256),
        ("SouthWest", 384),
        ("South", 512),
        ("SouthEast", 640),
        ("East", 768),
        ("NorthEast", 896),
    ] {
        g = g.get(name, move |_, _| Ok(LuaWAngle(WAngle::new(a))));
    }
    g.method("New", |_, _, a| Ok(LuaWAngle(WAngle::new(a.get(0)?))))
}

/// The local date and time for the `DateTime.Current*` properties, without a date library dependency.
mod chrono_lite {
    pub struct DateTime {
        pub year: i32,
        pub month: i32,
        pub day: i32,
        pub hour: i32,
        pub minute: i32,
        pub second: i32,
    }

    /// The current UTC date and time. C# reads the local time; the values only feed cosmetic script choices.
    pub fn now() -> DateTime {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let days = secs.div_euclid(86400);
        let rem = secs.rem_euclid(86400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        DateTime {
            year: year as i32,
            month: month as i32,
            day: day as i32,
            hour: (rem / 3600) as i32,
            minute: (rem % 3600 / 60) as i32,
            second: (rem % 60) as i32,
        }
    }
}
