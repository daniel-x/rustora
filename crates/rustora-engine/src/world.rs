// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of the synced parts of `World.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode, nodes_to_string};
use rustora_core::world_types::MersenneTwister;
use rustora_net::order::RawOrder;
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
    /// `World.BotRandom` of the reference harness: the random numbers of bot modules, seeded by [`bot_seed`].
    pub bot_random: RefCell<MersenneTwister>,
    /// The `Game.Sound` calls of this world, drained by the client.
    pub sound: crate::sound::SoundQueue,
    /// Orders issued by this client (bots) with the world tick they were issued in, drained by the order manager.
    issued_orders: RefCell<Vec<(i32, crate::order::Order)>>,
    /// Called with every issued order at issue time, for order dumps.
    order_observer: RefCell<Option<Rc<dyn Fn(&World, &crate::order::Order)>>>,
    players: RefCell<Vec<PlayerRef>>,
    player_bits: RefCell<Vec<String>>,
    world_actor: OnceCell<ActorRef>,
    map: Rc<Map>,
    rules: Rc<Ruleset>,
    pub lobby_info: Session,
    pub world_type: WorldType,
    /// `Game.Settings.Game.PauseShellmap`.
    pub pause_shellmap: Cell<bool>,
    pub timestep: i32,
    /// `World.ReplayTimestep`: the playback interval of a replay in milliseconds, 0 while paused.
    pub replay_timestep: Cell<i32>,
    pub order_latency: i32,
    world_tick: Cell<i32>,
    paused: Cell<bool>,
    is_game_over: Cell<bool>,
    end_game_requests: RefCell<Vec<i32>>,
    pub disposing: Cell<bool>,
    pub all_players_mask: Cell<u64>,
    actor_added: RefCell<Vec<Rc<dyn Fn(&ActorRef)>>>,
    actor_removed: RefCell<Vec<Rc<dyn Fn(&ActorRef)>>>,
    /// Cache for the actor map trait of the mod crate.
    pub actor_map_slot: OnceCell<Rc<dyn std::any::Any>>,
    is_replay: Cell<bool>,
    net_frame_number: Cell<i32>,
    game_save_last_frame: Cell<i32>,
    game_save_last_sync_frame: Cell<i32>,
    was_loading_game_save: Cell<bool>,
    game_save_trait_data: RefCell<IndexMap<i32, MiniYaml>>,
}

/// `RustoraHarness.BotSeed`: the seed of the bot random numbers, derived from the lobby random seed.
pub fn bot_seed(random_seed: i32) -> i32 {
    random_seed ^ 0x2545_F491
}

impl World {
    /// `World.IssueOrder`: queues an order of the local client.
    pub fn issue_order(&self, order: crate::order::Order) {
        let observer = self.order_observer.borrow().clone();
        if let Some(f) = observer {
            f(self, &order);
        }
        self.issued_orders.borrow_mut().push((self.world_tick.get(), order));
    }

    pub fn set_order_observer(&self, f: Rc<dyn Fn(&World, &crate::order::Order)>) {
        *self.order_observer.borrow_mut() = Some(f);
    }

    /// Takes the orders issued since the last call, with their world ticks.
    pub fn take_issued_orders(&self) -> Vec<(i32, crate::order::Order)> {
        std::mem::take(&mut *self.issued_orders.borrow_mut())
    }

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
            bot_random: RefCell::new(MersenneTwister::new(bot_seed(seed))),
            sound: Default::default(),
            issued_orders: RefCell::new(Vec::new()),
            order_observer: RefCell::new(None),
            players: RefCell::new(Vec::new()),
            player_bits: RefCell::new(Vec::new()),
            world_actor: OnceCell::new(),
            map,
            rules,
            lobby_info,
            world_type,
            timestep,
            replay_timestep: Cell::new(timestep),
            order_latency,
            world_tick: Cell::new(0),
            pause_shellmap: Cell::new(false),
            paused: Cell::new(false),
            is_game_over: Cell::new(false),
            end_game_requests: RefCell::new(Vec::new()),
            disposing: Cell::new(false),
            all_players_mask: Cell::new(0),
            actor_added: RefCell::new(Vec::new()),
            actor_removed: RefCell::new(Vec::new()),
            actor_map_slot: OnceCell::new(),
            is_replay: Cell::new(false),
            net_frame_number: Cell::new(0),
            game_save_last_frame: Cell::new(-1),
            game_save_last_sync_frame: Cell::new(-1),
            was_loading_game_save: Cell::new(false),
            game_save_trait_data: RefCell::new(IndexMap::new()),
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

    /// `Facings` of a sprite sequence (`interpolatedFacings ?? facings`, always positive).
    pub fn sequence_facings(&self, image: &str, sequence: &str) -> Option<i32> {
        let seqs = self.map.sequences();
        if !seqs.has_image(image) || !seqs.has_sequence(image, sequence) {
            return None;
        }
        Some(seqs.get_sequence(image, sequence).facings_or_interpolated())
    }

    pub fn set_world_owner(&self, p: &PlayerRef) {
        self.world_actor().set_owner(Some(p.clone()));
    }

    /// Subscribes to `ActorAdded`.
    pub fn on_actor_added(&self, f: Rc<dyn Fn(&ActorRef)>) {
        self.actor_added.borrow_mut().push(f);
    }

    /// Subscribes to `ActorRemoved`.
    pub fn on_actor_removed(&self, f: Rc<dyn Fn(&ActorRef)>) {
        self.actor_removed.borrow_mut().push(f);
    }

    /// Subscribes to both `ActorAdded` and `ActorRemoved`.
    pub fn on_actor_changed(&self, f: Rc<dyn Fn(&ActorRef)>) {
        self.actor_added.borrow_mut().push(f.clone());
        self.actor_removed.borrow_mut().push(f);
    }

    pub fn order_validators(&self) -> Vec<Rc<dyn IValidateOrder>> {
        self.world_actor().traits_implementing::<dyn IValidateOrder>()
    }

    /// `IsLoadingGameSave`: true while the orders of a game save are replayed.
    pub fn is_loading_game_save(&self) -> bool {
        self.net_frame_number.get() <= self.game_save_last_frame.get()
    }

    /// `OrderManager.NetFrameNumber`.
    pub fn net_frame_number(&self) -> i32 {
        self.net_frame_number.get()
    }

    pub fn set_net_frame_number(&self, frame: i32) {
        self.net_frame_number.set(frame);
    }

    /// Marks the world as restoring a game save, from the `SaveLastOrdersFrame` and `SaveSyncFrame` of `StartGame`.
    pub fn begin_loading_game_save(&self, last_frame: i32, last_sync_frame: i32) {
        self.game_save_last_frame.set(last_frame);
        self.game_save_last_sync_frame.set(last_sync_frame);
        self.was_loading_game_save.set(self.is_loading_game_save());
    }

    pub fn game_save_last_frame(&self) -> i32 {
        self.game_save_last_frame.get()
    }

    pub fn game_save_last_sync_frame(&self) -> i32 {
        self.game_save_last_sync_frame.get()
    }

    /// `AddGameSaveTraitData`.
    pub fn add_game_save_trait_data(&self, trait_index: i32, data: MiniYaml) {
        self.game_save_trait_data.borrow_mut().insert(trait_index, data);
    }

    fn resolve_game_save_trait_data(self: &Rc<Self>) {
        let traits = self.actors_with_trait::<dyn IGameSaveTraitData>();
        for (index, data) in std::mem::take(&mut *self.game_save_trait_data.borrow_mut()) {
            let Some((actor, t)) = usize::try_from(index).ok().and_then(|i| traits.get(i)) else {
                break;
            };
            t.resolve_trait_data(actor, &data);
        }
    }

    /// `RequestGameSave`: the orders that make the server write a game save.
    pub fn request_game_save(&self, filename: &str) -> Vec<RawOrder> {
        let mut orders = Vec::new();
        for (i, (actor, t)) in self.actors_with_trait::<dyn IGameSaveTraitData>().iter().enumerate() {
            if let Some(data) = t.issue_trait_data(actor) {
                let node = MiniYamlNode::new(i.to_string(), MiniYaml::new(Some(String::new()), data));
                orders.push(RawOrder::from_target_string("GameSaveTraitData", &nodes_to_string(&[node]), true));
            }
        }
        orders.push(RawOrder::from_target_string("CreateGameSave", filename, true));
        orders
    }

    pub fn is_replay(&self) -> bool {
        self.is_replay.get()
    }

    pub fn set_is_replay(&self, v: bool) {
        self.is_replay.set(v);
    }

    /// `World.OnClientDisconnected`.
    pub fn on_client_disconnected(&self, client_id: i32) {
        for player in self.players() {
            if player.client_index != client_id || !player.playable {
                continue;
            }
            for n in self.world_actor().traits_implementing::<dyn INotifyPlayerDisconnected>() {
                n.player_disconnected(self.world_actor(), &player);
            }
            for p in self.players() {
                p.player_disconnected(&player);
            }
        }
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
        let l = self.actor_added.borrow().clone();
        for f in l {
            f(a);
        }
        for t in a.traits_implementing::<dyn INotifyAddedToWorld>() {
            t.added_to_world(a);
        }
    }

    pub fn remove(&self, a: &ActorRef) {
        a.set_in_world(false);
        self.actors.borrow_mut().remove(&a.id);
        let l = self.actor_removed.borrow().clone();
        for f in l {
            f(a);
        }
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

    /// Asks the host loop to call [`World::end_game`] after `delay_ms` (the C# `Game.RunAfterDelay`).
    pub fn request_end_game(&self, delay_ms: i32) {
        self.end_game_requests.borrow_mut().push(delay_ms);
    }

    /// Takes the delays of the end game requests filed since the last call.
    pub fn take_end_game_requests(&self) -> Vec<i32> {
        std::mem::take(&mut *self.end_game_requests.borrow_mut())
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
        if self.was_loading_game_save.get() && !self.is_loading_game_save() {
            self.resolve_game_save_trait_data();
            self.was_loading_game_save.set(false);
        }
        if !self.paused.get() && (self.world_type != WorldType::Shellmap || !self.pause_shellmap.get() || self.world_tick.get() == 0) {
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
