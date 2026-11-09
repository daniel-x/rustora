// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `SupportPowerBotModule.cs` and `BotModuleLogic/SupportPowerDecision.cs`.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_core::fieldloader::{LoadError, load};
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_core::world_types::{CPos, WDist, WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::frozen::FrozenActorRef;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::Target;
use rustora_engine::terrain::TargetableType;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use super::*;
use crate::actor_map::WorldExt;
use crate::health::HealthInfo;
use crate::production::ValuedInfo;
use crate::support_powers::{SupportPowerInstance, SupportPowerManager};

pub fn register(r: &mut TraitRegistry) {
    r.register("SupportPowerBotModule", |n| load_info!(SupportPowerBotModuleInfo, n));
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, rustora_engine::persist::Persist)]
#[persist(value)]
pub enum DecisionMetric {
    Health,
    Value,
    #[default]
    None,
}

rustora_core::field_enum!(DecisionMetric, [Health, Value, None]);

/// `SupportPowerDecision.Consideration`: how to evaluate a target.
#[derive(Clone, Debug, FieldLoad, rustora_engine::persist::Persist)]
#[persist(value)]
pub struct Consideration {
    pub against: PlayerRelationship,
    pub types: BitSet<TargetableType>,
    pub attractiveness: i32,
    pub target_metric: DecisionMetric,
    pub check_radius: WDist,
}

impl Default for Consideration {
    fn default() -> Self {
        Consideration {
            against: PlayerRelationship::ENEMY,
            types: BitSet::new(&["Air", "Ground", "Water"]),
            attractiveness: 100,
            target_metric: DecisionMetric::None,
            check_radius: WDist::from_cells(5),
        }
    }
}

impl Consideration {
    fn actor_attractiveness(&self, a: &ActorRef, stance: PlayerRelationship, fired_by: &PlayerRef) -> i32 {
        if stance != self.against {
            return 0;
        }
        if !rustora_engine::actor_ext::is_targetable_by(a, fired_by.player_actor()) || !a.can_be_viewed_by_player(Some(fired_by)) {
            return 0;
        }
        if !self.types.overlaps(&rustora_engine::actor_ext::enabled_target_types(a)) {
            return 0;
        }
        match self.target_metric {
            DecisionMetric::Value => a.info.trait_info::<ValuedInfo>().map_or(0, |v| v.cost * self.attractiveness),
            DecisionMetric::Health => a
                .trait_or_default::<dyn IHealth>()
                .map_or(0, |h| (h.hp() as i64 * self.attractiveness as i64 / h.max_hp() as i64) as i32),
            DecisionMetric::None => self.attractiveness,
        }
    }

    fn frozen_attractiveness(&self, fa: &FrozenActorRef, stance: PlayerRelationship) -> i32 {
        if stance != self.against || !fa.is_valid() || !fa.visible() {
            return 0;
        }
        if !self.types.overlaps(&fa.target_types()) {
            return 0;
        }
        match self.target_metric {
            DecisionMetric::Value => fa.info().trait_info::<ValuedInfo>().map_or(0, |v| v.cost * self.attractiveness),
            DecisionMetric::Health => fa
                .info()
                .trait_info::<HealthInfo>()
                .map_or(0, |h| fa.hp() * self.attractiveness / h.hp),
            DecisionMetric::None => self.attractiveness,
        }
    }
}

/// `SupportPowerDecision`: bot metadata for using one support power.
#[derive(Clone, Debug, FieldLoad, rustora_engine::persist::Persist)]
#[persist(value)]
pub struct SupportPowerDecision {
    pub minimum_attractiveness: i32,
    pub order_name: String,
    pub coarse_scan_radius: i32,
    pub fine_scan_radius: i32,
    #[field(loader = "load_considerations")]
    pub considerations: Vec<Consideration>,
    pub minimum_scan_time_interval: i32,
    pub maximum_scan_time_interval: i32,
}

impl Default for SupportPowerDecision {
    fn default() -> Self {
        SupportPowerDecision {
            minimum_attractiveness: 1,
            order_name: "AirstrikePowerInfoOrder".into(),
            coarse_scan_radius: 20,
            fine_scan_radius: 2,
            considerations: Vec::new(),
            minimum_scan_time_interval: 250,
            maximum_scan_time_interval: 262,
        }
    }
}

fn load_considerations(y: &MiniYaml) -> Result<Vec<Consideration>, LoadError> {
    y.nodes
        .iter()
        .filter(|d| d.key.split('@').next() == Some("Consideration"))
        .map(|d| load::<Consideration>(&d.value))
        .collect()
}

fn relationship(fired_by: &PlayerRef, owner: Option<&PlayerRef>) -> PlayerRelationship {
    fired_by.relationship_with(owner.map(|o| &**o))
}

impl SupportPowerDecision {
    /// Evaluates the attractiveness of a position according to all considerations.
    pub fn position_attractiveness(&self, pos: WPos, fired_by: &PlayerRef) -> i32 {
        let world = fired_by.player_actor().world();
        let map = world.map();
        if !map.contains(map.cell_containing(pos)) {
            return 0;
        }
        let layer = fired_by.frozen_actor_layer().expect("FrozenActorLayer");
        let mut answer = 0;
        for c in &self.considerations {
            let r = WDist::new(c.check_radius.length);
            for a in world.find_actors_in_circle(pos, r) {
                answer += c.actor_attractiveness(&a, relationship(fired_by, a.owner_opt().as_ref()), fired_by);
            }
            let delta = WVec::new(r.length, r.length, 0);
            let tl = map.cell_containing(pos - delta);
            let br = map.cell_containing(pos + delta);
            for fa in layer.frozen_actors_in_region(tl, br, true) {
                answer += c.frozen_attractiveness(&fa, relationship(fired_by, fa.owner().as_ref()));
            }
        }
        answer
    }

    /// Evaluates the attractiveness of a group of actors according to all considerations.
    pub fn actors_attractiveness(&self, actors: &[ActorRef], fired_by: &PlayerRef) -> i32 {
        let mut answer = 0;
        for c in &self.considerations {
            for a in actors {
                answer += c.actor_attractiveness(a, relationship(fired_by, a.owner_opt().as_ref()), fired_by);
            }
        }
        answer
    }

    pub fn frozen_actors_attractiveness(&self, frozen: &[FrozenActorRef], fired_by: &PlayerRef) -> i32 {
        let mut answer = 0;
        for c in &self.considerations {
            for fa in frozen {
                if fa.is_valid() && fa.visible() {
                    answer += c.frozen_attractiveness(fa, relationship(fired_by, fa.owner().as_ref()));
                }
            }
        }
        answer
    }

    pub fn next_scan_time(&self, world: &World) -> i32 {
        world
            .bot_random
            .borrow_mut()
            .next_range(self.minimum_scan_time_interval, self.maximum_scan_time_interval)
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct SupportPowerBotModuleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(loader = "load_decisions")]
    pub decisions: Vec<SupportPowerDecision>,
}

fn load_decisions(y: &MiniYaml) -> Result<Vec<SupportPowerDecision>, LoadError> {
    let Some(decisions) = y.node_with_key_or_default("Decisions")? else {
        return Ok(Vec::new());
    };
    decisions
        .value
        .nodes
        .iter()
        .map(|d| load::<SupportPowerDecision>(&d.value))
        .collect()
}

/// `SupportPowerBotModule`: fires ready support powers at the most attractive locations.
#[derive(rustora_engine::persist::Persist)]
pub struct SupportPowerBotModule {
    info: Rc<SupportPowerBotModuleInfo>,
    state: ConditionalState,
    self_: ActorRef,
    waiting_powers: RefCell<Vec<(Rc<SupportPowerInstance>, i32)>>,
    power_decisions: RefCell<IndexMap<String, SupportPowerDecision>>,
}

register_traits!(SupportPowerBotModule: IGameSaveTraitData, IBotTick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(SupportPowerBotModuleInfo => |info, init| Some(Rc::new(SupportPowerBotModule {
    state: ConditionalState::new(&info.base),
    self_: init.self_.clone(),
    waiting_powers: RefCell::new(Vec::new()),
    power_decisions: RefCell::new(IndexMap::new()),
    info,
})));

impl ConditionalHooks for SupportPowerBotModule {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, _self_: &ActorRef) {
        let mut d = self.power_decisions.borrow_mut();
        for decision in &self.info.decisions {
            assert!(
                d.insert(decision.order_name.clone(), decision.clone()).is_none(),
                "duplicate decision {}",
                decision.order_name
            );
        }
    }
}
rustora_engine::conditional_plumbing!(SupportPowerBotModule);

impl IGameSaveTraitData for SupportPowerBotModule {
    fn issue_trait_data(&self, _actor: &ActorRef) -> Option<Vec<MiniYamlNode>> {
        if self.state.is_trait_disabled() {
            return None;
        }
        let powers = self
            .waiting_powers
            .borrow()
            .iter()
            .map(|(p, ticks)| MiniYamlNode::with_value_str(p.key.clone(), ticks.to_string()))
            .collect();
        Some(vec![MiniYamlNode::new("WaitingPowers", MiniYaml::new(Some(String::new()), powers))])
    }

    fn resolve_trait_data(&self, self_: &ActorRef, data: &MiniYaml) {
        if self_.world().is_replay() {
            return;
        }
        let Ok(Some(node)) = data.node_with_key_or_default("WaitingPowers") else {
            return;
        };
        let manager = self.self_.owner().player_actor().trait_::<SupportPowerManager>();
        let powers = manager.powers.borrow();
        let mut waiting = self.waiting_powers.borrow_mut();
        for n in &node.value.nodes {
            let (Some(instance), Ok(ticks)) = (powers.get(&n.key), n.value.value_str().parse()) else {
                continue;
            };
            match waiting.iter_mut().find(|(p, _)| Rc::ptr_eq(p, instance)) {
                Some(e) => e.1 = ticks,
                None => waiting.push((instance.clone(), ticks)),
            }
        }
    }
}

impl INotifyCreated for SupportPowerBotModule {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl IBotTick for SupportPowerBotModule {
    fn bot_tick(&self, bot: &ModularBot) {
        let manager = self.self_.owner().player_actor().trait_::<SupportPowerManager>();
        let world = self.self_.world();
        let powers: Vec<Rc<SupportPowerInstance>> = manager.powers.borrow().values().cloned().collect();
        for sp in powers {
            if sp.disabled() {
                continue;
            }
            let waiting = self.tick_waiting(&sp);
            if !sp.ready() || waiting > 0 {
                continue;
            }
            let Some(order_name) = sp.info().map(|i| i.order_name.clone()) else {
                continue;
            };
            let Some(decision) = self.power_decisions.borrow().get(&order_name).cloned() else {
                continue;
            };
            let location = self
                .find_coarse_attack_location(&decision)
                .and_then(|c| self.find_fine_attack_location(&decision, c, 1));
            let Some(location) = location else {
                let t = decision.next_scan_time(&world);
                self.add_waiting(&sp, t);
                continue;
            };
            self.add_waiting(&sp, 10);
            let mut order = Order::new(
                &sp.key,
                Some(manager.self_.clone()),
                Target::from_cell(&world, location, FULL_CELL),
                false,
            );
            order.extra_data = u32::MAX;
            bot.queue_order(order);
        }
        let current = manager.powers.borrow();
        self.waiting_powers.borrow_mut().retain(|(p, _)| current.contains_key(&p.key));
    }

    fn bot_module_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }
}

impl SupportPowerBotModule {
    /// Adds `sp` to the waiting powers if needed, counts its delay down and returns the remaining delay.
    fn tick_waiting(&self, sp: &Rc<SupportPowerInstance>) -> i32 {
        let mut w = self.waiting_powers.borrow_mut();
        let i = match w.iter().position(|(p, _)| Rc::ptr_eq(p, sp)) {
            Some(i) => i,
            None => {
                w.push((sp.clone(), 0));
                w.len() - 1
            }
        };
        if w[i].1 > 0 {
            w[i].1 -= 1;
        }
        w[i].1
    }

    fn add_waiting(&self, sp: &Rc<SupportPowerInstance>, ticks: i32) {
        if let Some(e) = self.waiting_powers.borrow_mut().iter_mut().find(|(p, _)| Rc::ptr_eq(p, sp)) {
            e.1 += ticks;
        }
    }

    /// `FindCoarseAttackLocationToSupportPower`: scans the map in chunks and picks a random chunk with above average
    /// attractiveness.
    fn find_coarse_attack_location(&self, decision: &SupportPowerDecision) -> Option<CPos> {
        let world = self.self_.world();
        let map = world.map();
        let player = self.self_.owner();
        let layer = player.frozen_actor_layer();
        let r = decision.coarse_scan_radius;
        let mut suitable: Vec<(CPos, i32)> = Vec::new();
        let mut total = 0;
        let (w, h) = map.map_size;
        for i in (0..w).step_by(r as usize) {
            for j in (0..h).step_by(r as usize) {
                let tl = CPos::new(i, j);
                let br = CPos::new(i + r, j + r);
                let targets = world.actor_map().actors_in_box(map.center_of_cell(tl), map.center_of_cell(br));
                let frozen = layer.as_ref().map_or_else(Vec::new, |l| l.frozen_actors_in_region(tl, br, true));
                let a = decision.actors_attractiveness(&targets, &player) + decision.frozen_actors_attractiveness(&frozen, &player);
                if a < decision.minimum_attractiveness {
                    continue;
                }
                suitable.push((tl, a));
                total += a;
            }
        }
        if suitable.is_empty() {
            return None;
        }
        let average = total / suitable.len() as i32;
        let found = shuffle(suitable, &world.bot_random).find(|x| x.1 >= average);
        Some(found.expect("location with at least average attractiveness").0)
    }

    /// `FindFineAttackLocationToSupportPower`: detail scans an area around `check_pos`.
    fn find_fine_attack_location(&self, decision: &SupportPowerDecision, check_pos: CPos, extended_range: i32) -> Option<CPos> {
        let world = self.self_.world();
        let map = world.map();
        let player = self.self_.owner();
        let (mut best_location, mut best) = (None, 0);
        let r = decision.coarse_scan_radius;
        let step = decision.fine_scan_radius as usize;
        for i in (-extended_range..=r + extended_range).step_by(step) {
            let x = check_pos.x() + i;
            for j in (-extended_range..=r + extended_range).step_by(step) {
                let y = check_pos.y() + j;
                let a = decision.position_attractiveness(map.center_of_cell(CPos::new(x, y)), &player);
                if a <= best || a < decision.minimum_attractiveness {
                    continue;
                }
                best = a;
                best_location = Some(CPos::new(x, y));
            }
        }
        best_location
    }
}
