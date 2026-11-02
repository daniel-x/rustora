// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of the synced parts of `World.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use rustora_core::world_types::MersenneTwister;
use rustora_net::session::Session;

use crate::actor::{Actor, ActorRef};
use crate::actor_init::ActorInits;
use crate::effect::EffectRef;
use crate::map::Map;
use crate::player::PlayerRef;
use crate::rules::Ruleset;
use crate::trait_dict::TraitDict;
use crate::traits::*;

pub type FrameEndTask = Box<dyn FnOnce(&Rc<World>)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldType {
    Regular,
    Shellmap,
    Editor,
}

pub struct World {
    trait_dict: TraitDict,
    actors: RefCell<BTreeMap<u32, ActorRef>>,
    effects: RefCell<Vec<EffectRef>>,
    synced_effects: RefCell<Vec<EffectRef>>,
    frame_end_actions: RefCell<VecDeque<FrameEndTask>>,
    next_aid: Cell<u32>,
    pub shared_random: RefCell<MersenneTwister>,
    pub local_random: RefCell<MersenneTwister>,
    players: RefCell<Vec<PlayerRef>>,
    player_bits: RefCell<Vec<String>>,
    world_actor: OnceCell<ActorRef>,
    map: Rc<Map>,
    rules: Rc<Ruleset>,
    pub lobby_info: Session,
    pub world_type: WorldType,
    pub timestep: i32,
    pub order_latency: i32,
    world_tick: Cell<i32>,
    paused: Cell<bool>,
    is_game_over: Cell<bool>,
    pub disposing: Cell<bool>,
    sequences: RefCell<Vec<rustora_core::miniyaml::MiniYamlNode>>,
}

impl World {
    /// Creates the world and the world actor, then runs `ICreatePlayers`.
    pub fn new(
        map: Rc<Map>,
        rules: Rc<Ruleset>,
        lobby_info: Session,
        world_type: WorldType,
        timestep: i32,
        order_latency: i32,
    ) -> Rc<World> {
        let seed = lobby_info.global_settings.random_seed;
        let world = Rc::new(World {
            trait_dict: TraitDict::default(),
            actors: RefCell::new(BTreeMap::new()),
            effects: RefCell::new(Vec::new()),
            synced_effects: RefCell::new(Vec::new()),
            frame_end_actions: RefCell::new(VecDeque::new()),
            next_aid: Cell::new(0),
            shared_random: RefCell::new(MersenneTwister::new(seed)),
            local_random: RefCell::new(MersenneTwister::new(0)),
            players: RefCell::new(Vec::new()),
            player_bits: RefCell::new(Vec::new()),
            world_actor: OnceCell::new(),
            map,
            rules,
            lobby_info,
            world_type,
            timestep,
            order_latency,
            world_tick: Cell::new(0),
            paused: Cell::new(false),
            is_game_over: Cell::new(false),
            disposing: Cell::new(false),
            sequences: RefCell::new(Vec::new()),
        });

        let name = if world_type == WorldType::Editor { "editorworld" } else { "world" };
        let wa = world.create_actor(name, ActorInits::new());
        let _ = world.world_actor.set(wa.clone());

        world.player_bits.borrow_mut().clear();
        let mut player_random = MersenneTwister::new(seed);
        for cmp in wa.traits_implementing::<dyn ICreatePlayers>() {
            cmp.create_players(&world, &mut player_random);
        }
        world
    }

    pub fn trait_dict(&self) -> &TraitDict {
        &self.trait_dict
    }

    pub fn map(&self) -> &Rc<Map> {
        &self.map
    }

    pub fn rules(&self) -> &Rc<Ruleset> {
        &self.rules
    }

    pub fn world_actor(&self) -> &ActorRef {
        self.world_actor.get().expect("world actor created")
    }

    /// Merged sprite sequence definitions (images with their sequences).
    pub fn set_sequences(&self, nodes: Vec<rustora_core::miniyaml::MiniYamlNode>) {
        *self.sequences.borrow_mut() = nodes;
    }

    /// `Facings` of a sprite sequence (`interpolatedFacings ?? facings`, always positive).
    pub fn sequence_facings(&self, image: &str, sequence: &str) -> Option<i32> {
        let seqs = self.sequences.borrow();
        let img = seqs.iter().find(|n| n.key == image)?;
        let defaults = img.value.nodes.iter().find(|n| n.key == "Defaults");
        let seq = img.value.nodes.iter().find(|n| n.key == sequence)?;
        let field = |k: &str| -> Option<i32> {
            seq.value
                .nodes
                .iter()
                .find(|n| n.key == k)
                .or_else(|| defaults.and_then(|d| d.value.nodes.iter().find(|n| n.key == k)))
                .and_then(|n| rustora_core::world_types::parse_i32(n.value.value_str()))
        };
        if let Some(i) = field("InterpolatedFacings") {
            return Some(i);
        }
        Some(field("Facings").unwrap_or(1).abs())
    }

    pub fn set_world_owner(&self, p: &PlayerRef) {
        self.world_actor().set_owner(Some(p.clone()));
    }

    pub fn world_tick(&self) -> i32 {
        self.world_tick.get()
    }

    pub fn paused(&self) -> bool {
        self.paused.get()
    }

    pub fn set_paused(&self, p: bool) {
        self.paused.set(p);
    }

    pub fn is_game_over(&self) -> bool {
        self.is_game_over.get()
    }

    pub fn players(&self) -> Vec<PlayerRef> {
        self.players.borrow().clone()
    }

    pub fn set_players(&self, players: Vec<PlayerRef>) {
        assert!(self.players.borrow().is_empty(), "Players are fixed once they have been set.");
        *self.players.borrow_mut() = players;
    }

    /// Bit for `LongBitSet<PlayerBitMask>`, allocated in first-use order.
    pub fn allocate_player_bit(&self, name: &str) -> u64 {
        let mut b = self.player_bits.borrow_mut();
        let idx = match b.iter().position(|n| n == name) {
            Some(i) => i,
            None => {
                b.push(name.to_string());
                b.len() - 1
            }
        };
        assert!(idx < 64, "too many players");
        1u64 << idx
    }

    pub(crate) fn next_aid(&self) -> u32 {
        let id = self.next_aid.get();
        self.next_aid.set(id + 1);
        id
    }

    pub fn create_actor(self: &Rc<Self>, name: &str, inits: ActorInits) -> ActorRef {
        self.create_actor_add(true, name, inits)
    }

    pub fn create_actor_add(self: &Rc<Self>, add_to_world: bool, name: &str, inits: ActorInits) -> ActorRef {
        let a = Actor::new(self, name, inits);
        a.initialize(add_to_world);
        a
    }

    pub fn add(&self, a: &ActorRef) {
        a.set_in_world(true);
        self.actors.borrow_mut().insert(a.id, a.clone());
        for t in a.traits_implementing::<dyn INotifyAddedToWorld>() {
            t.added_to_world(a);
        }
    }

    pub fn remove(&self, a: &ActorRef) {
        a.set_in_world(false);
        self.actors.borrow_mut().remove(&a.id);
        for t in a.traits_implementing::<dyn INotifyRemovedFromWorld>() {
            t.removed_from_world(a);
        }
    }

    pub fn add_effect(&self, e: EffectRef) {
        if e.is_sync() {
            self.synced_effects.borrow_mut().push(e.clone());
        }
        self.effects.borrow_mut().push(e);
    }

    pub fn remove_effect(&self, e: &EffectRef) {
        self.effects.borrow_mut().retain(|x| !Rc::ptr_eq(x, e));
        self.synced_effects.borrow_mut().retain(|x| !Rc::ptr_eq(x, e));
    }

    pub fn effects(&self) -> Vec<EffectRef> {
        self.effects.borrow().clone()
    }

    pub fn add_frame_end_task(&self, t: FrameEndTask) {
        self.frame_end_actions.borrow_mut().push_back(t);
    }

    pub fn actors(&self) -> Vec<ActorRef> {
        self.actors.borrow().values().cloned().collect()
    }

    pub fn get_actor_by_id(&self, id: u32) -> Option<ActorRef> {
        self.actors.borrow().get(&id).cloned()
    }

    pub fn actors_with_trait<T: ?Sized + 'static>(&self) -> Vec<(ActorRef, Rc<T>)> {
        self.trait_dict.actors_with_trait::<T>()
    }

    pub fn actors_having_trait<T: ?Sized + 'static>(&self) -> Vec<ActorRef> {
        self.trait_dict.actors_having_trait::<T>()
    }

    /// `World.LoadComplete` (sim part): `IWorldLoaded` on the world actor, then on player actors.
    pub fn load_complete(self: &Rc<Self>) {
        for t in self.world_actor().traits_implementing::<dyn IWorldLoaded>() {
            t.world_loaded(self);
        }
        for p in self.players() {
            for t in p.player_actor().traits_implementing::<dyn IWorldLoaded>() {
                t.world_loaded(self);
            }
        }
        for t in self.world_actor().traits_implementing::<dyn IPostWorldLoaded>() {
            t.post_world_loaded(self);
        }
        for p in self.players() {
            for t in p.player_actor().traits_implementing::<dyn IPostWorldLoaded>() {
                t.post_world_loaded(self);
            }
        }
    }

    /// Per-actor and per-trait sync values in the format of the C# harness detail dump.
    pub fn sync_detail(&self) -> Vec<String> {
        let mut out = vec![format!(
            "# random {} {}",
            self.shared_random.borrow().last,
            self.shared_random.borrow().total_count
        )];
        for a in self.actors.borrow().values() {
            let owner = a.owner_opt().map(|o| o.internal_name.clone()).unwrap_or_default();
            out.push(format!("# actor {} {} {} {}", a.id, a.info.name, owner, a.sync_hash_id()));
        }
        for a in self.trait_dict.actors_having_trait::<dyn ISync>() {
            for (s, name) in a.cache().sync_hashes.iter().zip(&a.cache().sync_names) {
                out.push(format!("# trait {} {} {}", a.id, name, s.sync_hash()));
            }
        }
        out
    }

    pub fn end_game(self: &Rc<Self>) {
        if self.is_game_over.get() {
            return;
        }
        self.is_game_over.set(true);
        for t in self.world_actor().traits_implementing::<dyn IGameOver>() {
            t.game_over(self);
        }
    }

    /// `World.Tick`: actors, then `ITick` traits, then effects, then frame end tasks.
    pub fn tick(self: &Rc<Self>) {
        if !self.paused.get() && (self.world_type != WorldType::Shellmap || self.world_tick.get() == 0) {
            self.world_tick.set(self.world_tick.get() + 1);
            for a in self.actors() {
                a.tick();
            }
            self.trait_dict.apply_to_all::<dyn ITick>(|a, t| t.tick(a));
            for e in self.effects() {
                e.tick(self);
            }
        }
        loop {
            let next = self.frame_end_actions.borrow_mut().pop_front();
            match next {
                Some(t) => t(self),
                None => break,
            }
        }
    }

    /// `World.SyncHash`.
    pub fn sync_hash(&self) -> i32 {
        let mut n: i32 = 0;
        let mut ret: i32 = 0;
        for a in self.actors.borrow().values() {
            ret = ret.wrapping_add(n.wrapping_mul(1i32.wrapping_add(a.id as i32)).wrapping_mul(a.sync_hash_id()));
            n += 1;
        }
        for a in self.trait_dict.actors_having_trait::<dyn ISync>() {
            for s in &a.cache().sync_hashes {
                ret = ret.wrapping_add(n.wrapping_mul(1i32.wrapping_add(a.id as i32)).wrapping_mul(s.sync_hash()));
                n += 1;
            }
        }
        for e in self.synced_effects.borrow().iter() {
            ret = ret.wrapping_add(n.wrapping_mul(e.sync_hash()));
            n += 1;
        }
        ret = ret.wrapping_add(self.shared_random.borrow().last);
        for p in self.players.borrow().iter() {
            if p.unlocked_render_player() {
                ret = ret.wrapping_add(p.sync_hash());
            }
        }
        ret
    }
}
