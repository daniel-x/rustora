// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Save games: the full simulation state of a world between two ticks.
//!
//! Unlike the OpenRA game saves, which replay the orders of the game from its start, a save game holds the state
//! itself. A file consists of
//! - the header: magic, version, mod, map uid, lobby, world tick and net frame ([`SaveHeader`]),
//! - the actor directory: id, type, inits and flags of every actor that is in the world or referenced,
//! - the body: world fields, player fields, effects, and per actor its owner, flags, conditions, the states of its
//!   traits and its activities.
//!
//! [`restore`] expects a world that was created for the lobby of the header with [`World::set_restoring`] set
//! before `load_complete`, so that the map actors and starting units are not spawned. It recreates the actors with
//! their ids and inits, initializes them, and then overwrites all state with the saved one.

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use crate::actor::{Actor, ActorRef};
use crate::actor_init::{ActorInit, ActorInits};
use crate::effect::EffectRef;
use crate::persist::{LoadFailure, Persist, SaveReader, SaveWriter};
use crate::player::WinState;
use crate::world::World;

const MAGIC: &[u8; 12] = b"RUSTORA-SAVE";
const VERSION: u32 = 1;

/// The header of a save game, enough to create the world to restore it into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveHeader {
    pub mod_id: String,
    pub map_uid: String,
    /// The serialized lobby `Session`.
    pub lobby: String,
    pub world_tick: i32,
    /// `OrderManager.NetFrameNumber` at the time of the save.
    pub net_frame: i32,
}

impl SaveHeader {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        for s in [&self.mod_id, &self.map_uid, &self.lobby] {
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        out.extend_from_slice(&self.world_tick.to_le_bytes());
        out.extend_from_slice(&self.net_frame.to_le_bytes());
    }

    /// Parses the header and returns it with the length of its bytes.
    pub fn read(data: &[u8]) -> Result<(SaveHeader, usize), String> {
        fn take<'a>(data: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], String> {
            let s = data.get(*pos..*pos + n).ok_or("the save game is truncated")?;
            *pos += n;
            Ok(s)
        }
        fn int(data: &[u8], pos: &mut usize) -> Result<u32, String> {
            Ok(u32::from_le_bytes(take(data, pos, 4)?.try_into().expect("4 bytes")))
        }
        fn string(data: &[u8], pos: &mut usize) -> Result<String, String> {
            let n = int(data, pos)? as usize;
            String::from_utf8(take(data, pos, n)?.to_vec()).map_err(|e| e.to_string())
        }
        let mut pos = 0;
        if take(data, &mut pos, MAGIC.len())? != MAGIC {
            return Err("not a rustora save game".into());
        }
        let version = int(data, &mut pos)?;
        if version != VERSION {
            return Err(format!("unsupported save game version {version}, expected {VERSION}"));
        }
        let header = SaveHeader {
            mod_id: string(data, &mut pos)?,
            map_uid: string(data, &mut pos)?,
            lobby: string(data, &mut pos)?,
            world_tick: int(data, &mut pos)? as i32,
            net_frame: int(data, &mut pos)? as i32,
        };
        Ok((header, pos))
    }
}

/// A trait that prevents saving, such as a Lua script whose state cannot be saved.
pub trait IPreventsSaveGame {
    /// The reason why the game cannot be saved, or `None` if it can.
    fn save_game_blocker(&self) -> Option<String>;
}

thread_local! {
    static PANIC_LOCATION: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` and turns a panic into an error with its message and, unless it is a [`LoadFailure`], its location,
/// without printing it.
fn catch<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        let mut location = info.location().map(|l| format!("{}:{}", l.file(), l.line()));
        if std::env::var_os("RUSTORA_SAVE_BACKTRACE").is_some() {
            location = Some(format!(
                "{}\n{}",
                location.unwrap_or_default(),
                std::backtrace::Backtrace::force_capture()
            ));
        }
        PANIC_LOCATION.with(|p| *p.borrow_mut() = location);
    }));
    let result = std::panic::catch_unwind(AssertUnwindSafe(f));
    std::panic::set_hook(hook);
    result.map_err(|e| {
        let location = PANIC_LOCATION.with(|p| p.borrow_mut().take());
        if let Some(f) = e.downcast_ref::<LoadFailure>() {
            return f.0.clone();
        }
        let msg = if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else if let Some(s) = e.downcast_ref::<&str>() {
            s.to_string()
        } else {
            "unknown error".into()
        };
        match location {
            Some(l) => format!("{msg} (at {l})"),
            None => msg,
        }
    })
}

/// Writes the state of `world` with the header fields `mod_id` and `map_uid`.
pub fn save(world: &Rc<World>, mod_id: &str) -> Result<Vec<u8>, String> {
    if world.frame_end_task_count() > 0 {
        return Err("a save game can only be written between two ticks".into());
    }
    for (_, t) in world.actors_with_trait::<dyn IPreventsSaveGame>() {
        if let Some(reason) = t.save_game_blocker() {
            return Err(reason);
        }
    }
    let header = SaveHeader {
        mod_id: mod_id.to_string(),
        map_uid: world.map().uid.clone(),
        lobby: world.lobby_info.serialize(),
        world_tick: world.world_tick(),
        net_frame: world.net_frame_number(),
    };
    let (directory, body) = catch(|| write_state(world))?;
    let mut out = Vec::new();
    header.write(&mut out);
    out.extend_from_slice(&directory);
    out.extend_from_slice(&body);
    Ok(out)
}

/// The bytes of the save game of `world` split into labeled blocks (world fields, effects, and per actor its core
/// state and each trait), for finding the first state that differs between two worlds.
pub fn debug_blocks(world: &Rc<World>) -> Result<Vec<(String, Vec<u8>)>, String> {
    catch(|| {
        let mut blocks = Vec::new();
        let mut w = SaveWriter::new(world);
        write_world(&mut w, world);
        blocks.push(("world".to_string(), w.take_bytes()));
        w.queue_actor(world.world_actor());
        for p in world.players() {
            w.queue_actor(p.player_actor());
        }
        for a in world.actors() {
            w.queue_actor(&a);
        }
        while let Some(a) = w.next_actor() {
            let label = format!("actor {} {}", a.id, a.info.name);
            w.current_actor = Some(a.clone());
            w.u32(a.id);
            a.save_core(&mut w);
            blocks.push((format!("{label} core"), w.take_bytes()));
            for (i, t) in a.trait_objects().iter().enumerate() {
                t.save_state(&mut w);
                blocks.push((format!("{label} trait {i} {}", t.type_name()), w.take_bytes()));
            }
        }
        blocks
    })
}

fn write_world(w: &mut SaveWriter, world: &Rc<World>) {
    world.world_tick().save(w);
    world.peek_next_aid().save(w);
    world.shared_random.borrow().save(w);
    world.local_random.borrow().save(w);
    world.bot_random.borrow().save(w);
    world.paused().save(w);
    world.is_game_over().save(w);
    world.all_players_mask.get().save(w);
    world.net_frame_number().save(w);
    world.local_frame_number().save(w);
    crate::persist::PersistState::save_state(&**world.map(), w);
    for p in world.players() {
        p.win_state.save(w);
        p.has_objectives.save(w);
        p.allied_players_mask.save(w);
        p.enemy_players_mask.save(w);
    }
    let effects: Vec<EffectRef> = world.effects().into_iter().filter(|e| !e.is_cosmetic()).collect();
    effects.save(w);
}

fn write_state(world: &Rc<World>) -> (Vec<u8>, Vec<u8>) {
    let mut w = SaveWriter::new(world);
    write_world(&mut w, world);

    w.queue_actor(world.world_actor());
    for p in world.players() {
        w.queue_actor(p.player_actor());
    }
    for a in world.actors() {
        w.queue_actor(&a);
    }
    let mut body = Vec::new();
    let mut seen: Vec<ActorRef> = Vec::new();
    loop {
        while let Some(a) = w.next_actor() {
            write_actor(&mut w, &a);
            seen.push(a);
        }
        body.extend(w.take_bytes());
        if let Some(skipped) = &w.skipped {
            for t in skipped {
                eprintln!("persist audit: skipped {t}");
            }
        }
        seen.sort_by_key(|a| a.id);
        let objects_before = w.object_count();
        write_directory(&mut w, &seen);
        assert_eq!(objects_before, w.object_count(), "actor inits must not hold shared objects");
        let directory = w.take_bytes();
        if w.next_actor_pending() {
            continue;
        }
        return (directory, body);
    }
}

fn write_directory(w: &mut SaveWriter, actors: &[ActorRef]) {
    let next_aid = w.world().peek_next_aid();
    w.u32(next_aid);
    w.len_prefix(actors.len());
    for a in actors {
        w.u32(a.id);
        w.str(&a.info.name);
        a.inits().0.save(w);
        w.bool(a.is_in_world());
        w.bool(a.disposed());
    }
}

fn write_actor(w: &mut SaveWriter, a: &ActorRef) {
    w.current_actor = Some(a.clone());
    w.u32(a.id);
    a.save_core(w);
    let traits = a.trait_objects();
    w.len_prefix(traits.len());
    for t in &traits {
        w.str(t.type_name());
        let start = w.begin_len();
        t.save_state(w);
        w.end_len(start);
    }
}

/// Restores the state of `data` into `world`, see the module documentation.
pub fn restore(world: &Rc<World>, data: &[u8]) -> Result<(), String> {
    let (_, header_len) = SaveHeader::read(data)?;
    let result = catch(|| read_state(world, &data[header_len..]));
    world.set_restoring(false);
    result
}

struct Entry {
    actor: ActorRef,
    created: bool,
    in_world: bool,
    disposed: bool,
}

fn read_state(world: &Rc<World>, data: &[u8]) {
    let mut r = SaveReader::new(world, data);
    let existing: HashMap<u32, ActorRef> = world.registered_actors().into_iter().map(|a| (a.id, a)).collect();
    let saved_next_aid = r.u32();
    world.set_next_aid(world.peek_next_aid().max(saved_next_aid));
    let n = r.len_prefix();
    let mut entries = Vec::with_capacity(n);
    let mut ids = HashSet::new();
    for _ in 0..n {
        let id = r.u32();
        let name = r.string();
        let inits: Vec<Rc<dyn ActorInit>> = Persist::load(&mut r);
        let in_world = r.bool();
        let disposed = r.bool();
        let (actor, created) = match existing.get(&id) {
            Some(a) if a.info.name == name => (a.clone(), false),
            Some(a) => r.fail(format!(
                "actor {id} is a {} in the save game, but {} in the world",
                name, a.info.name
            )),
            None => (Actor::new_with_id(world, &name, id, ActorInits(inits)), true),
        };
        ids.insert(id);
        r.add_actor(actor.clone());
        entries.push(Entry {
            actor,
            created,
            in_world,
            disposed,
        });
    }
    let max_id = entries.iter().map(|e| e.actor.id).max().unwrap_or(0);
    world.set_next_aid(world.peek_next_aid().max(max_id + 1));
    for e in entries.iter().filter(|e| e.created) {
        e.actor.initialize(false);
    }
    for e in entries.iter().filter(|e| e.created && e.in_world && !e.disposed) {
        world.add(&e.actor);
    }
    for a in world.registered_actors() {
        if ids.contains(&a.id) {
            continue;
        }
        if a.is_in_world() {
            world.remove(&a);
        }
        world.trait_dict().remove_actor(a.id);
        a.mark_disposed();
    }
    for e in entries.iter().filter(|e| e.disposed) {
        if e.actor.is_in_world() {
            world.remove(&e.actor);
        }
        world.trait_dict().remove_actor(e.actor.id);
    }

    world.set_world_tick(Persist::load(&mut r));
    let next_aid: u32 = Persist::load(&mut r);
    *world.shared_random.borrow_mut() = Persist::load(&mut r);
    *world.local_random.borrow_mut() = Persist::load(&mut r);
    *world.bot_random.borrow_mut() = Persist::load(&mut r);
    world.set_paused(Persist::load(&mut r));
    world.set_game_over(Persist::load(&mut r));
    world.all_players_mask.set(Persist::load(&mut r));
    world.set_net_frame_number(Persist::load(&mut r));
    world.set_local_frame_number(Persist::load(&mut r));
    crate::persist::PersistState::load_state(&**world.map(), &mut r);
    for p in world.players() {
        p.win_state.set(Persist::load(&mut r));
        p.has_objectives.set(Persist::load(&mut r));
        p.allied_players_mask.set(Persist::load(&mut r));
        p.enemy_players_mask.set(Persist::load(&mut r));
    }
    let effects: Vec<EffectRef> = Persist::load(&mut r);
    world.replace_effects(effects);

    while !r.at_end() {
        let actor = r.actor();
        read_actor(&mut r, &actor);
    }
    world.set_next_aid(next_aid);
    world.clear_frame_end_tasks();
    world.set_in_world_actors(entries.iter().filter(|e| e.actor.is_in_world()).map(|e| e.actor.clone()).collect());
}

fn read_actor(r: &mut SaveReader, a: &ActorRef) {
    r.set_current_actor(Some(a.clone()));
    a.load_core(r);
    let traits = a.trait_objects();
    let n = r.len_prefix();
    if n != traits.len() {
        r.fail(format!("actor {a:?} has {} traits, the save game {n}", traits.len()));
    }
    for t in &traits {
        let name = r.string();
        if name != t.type_name() {
            r.fail(format!(
                "actor {a:?}: trait {} in the save game is {} in the world",
                name,
                t.type_name()
            ));
        }
        let len = r.u32() as usize;
        let start = r.position();
        t.load_state(r);
        if r.position() - start != len {
            r.fail(format!(
                "actor {a:?}: trait {name} read {} bytes of its {len}",
                r.position() - start
            ));
        }
    }
}

impl Persist for WinState {
    fn save(&self, w: &mut SaveWriter) {
        w.u8(match self {
            WinState::Undefined => 0,
            WinState::Won => 1,
            WinState::Lost => 2,
        })
    }
    fn load(r: &mut SaveReader) -> Self {
        match r.u8() {
            0 => WinState::Undefined,
            1 => WinState::Won,
            2 => WinState::Lost,
            v => r.fail(format!("invalid win state {v}")),
        }
    }
}
