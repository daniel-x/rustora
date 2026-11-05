// Copyright (c) The OpenRA Developers and Contributors. Rustora is a Rust port of OpenRA.
// SPDX-License-Identifier: GPL-3.0-or-later

//! Port of `Traits/Player/PlayerStatistics.cs`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::damage::AttackInfo;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, WinState};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::base::PlayerExperience;
use crate::building::BuildingInfo;
use crate::interfaces::{INotifyKilled, IPositionable};
use crate::player::PlayerResources;
use crate::production::ValuedInfo;

pub fn register(r: &mut TraitRegistry) {
    r.register("PlayerStatistics", |n| load_info!(PlayerStatisticsInfo, n));
    r.register("UpdatesPlayerStatistics", |n| load_info!(UpdatesPlayerStatisticsInfo, n));
}

const SAMPLE_INTERVAL_MS: i32 = 30000;
const INCOME_INTERVAL_MS: i32 = 1000;
const EARNED_SECONDS: usize = 60;

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PlayerStatisticsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

/// The counters shown by the observer panels and the game info page.
#[derive(Default)]
pub struct PlayerStatistics {
    pub order_count: Cell<i32>,
    pub income_samples: RefCell<Vec<i32>>,
    pub income: Cell<i32>,
    pub display_income: Cell<i32>,
    pub army_samples: RefCell<Vec<i32>>,
    pub kills_cost: Cell<i32>,
    pub deaths_cost: Cell<i32>,
    pub units_killed: Cell<i32>,
    pub units_dead: Cell<i32>,
    pub buildings_killed: Cell<i32>,
    pub buildings_dead: Cell<i32>,
    pub army_value: Cell<i32>,
    pub assets_value: Cell<i32>,
    /// The unit counts of the army display by actor name, in order of first appearance.
    pub units: RefCell<IndexMap<String, i32>>,
    earned_seconds: RefCell<VecDeque<i32>>,
    last_income: Cell<i32>,
    last_income_tick: Cell<i32>,
    ticks: Cell<i32>,
    army_graph_disabled: Cell<bool>,
    income_graph_disabled: Cell<bool>,
    resources: RefCell<Option<Rc<PlayerResources>>>,
    experience: RefCell<Option<Rc<PlayerExperience>>>,
}

register_traits!(PlayerStatistics: ITick, IResolveOrder, INotifyCreated, IWorldLoaded);
info_impl!(PlayerStatisticsInfo => |_info, _init| Some(Rc::new(PlayerStatistics::default())));

fn add(cell: &Cell<i32>, delta: i32) {
    cell.set(cell.get() + delta);
}

impl PlayerStatistics {
    pub fn experience(&self) -> i32 {
        self.experience.borrow().as_ref().map_or(0, |e| e.experience())
    }

    fn change_unit_count(&self, actor_name: &str, delta: i32) {
        *self.units.borrow_mut().entry(actor_name.to_string()).or_insert(0) += delta;
    }

    fn sample(disabled: &Cell<bool>, value: i32, owner: &PlayerRef, samples: &RefCell<Vec<i32>>) {
        if !disabled.get() && (value != 0 || owner.win_state.get() == WinState::Undefined) {
            samples.borrow_mut().push(value);
        } else {
            disabled.set(true);
        }
    }
}

impl INotifyCreated for PlayerStatistics {
    fn created(&self, self_: &ActorRef) {
        let resources = self_.trait_or_default::<PlayerResources>();
        self.income_graph_disabled.set(resources.is_none());
        *self.resources.borrow_mut() = resources;
        *self.experience.borrow_mut() = self_.trait_or_default::<PlayerExperience>();
    }
}

impl ITick for PlayerStatistics {
    fn tick(&self, self_: &ActorRef) {
        add(&self.ticks, 1);
        let world = self_.world();
        let timestep = world.timestep;
        if self.ticks.get() * timestep >= SAMPLE_INTERVAL_MS {
            self.ticks.set(0);
            let owner = self_.owner();
            Self::sample(&self.army_graph_disabled, self.army_value.get(), &owner, &self.army_samples);
            Self::sample(&self.income_graph_disabled, self.income.get(), &owner, &self.income_samples);
        }
        let Some(resources) = self.resources.borrow().clone() else {
            return;
        };
        let tick_delta = world.world_tick() - self.last_income_tick.get();
        if tick_delta * timestep >= INCOME_INTERVAL_MS {
            self.last_income_tick.set(world.world_tick());
            let mut earned_seconds = self.earned_seconds.borrow_mut();
            let last_earned = if earned_seconds.len() >= EARNED_SECONDS {
                earned_seconds.pop_front().unwrap_or(0)
            } else {
                0
            };
            self.last_income.set(self.income.get());
            self.display_income.set(self.income.get());
            self.income.set(resources.earned.get() - last_earned);
            earned_seconds.push_back(resources.earned.get());
        } else {
            let (a, b) = (self.last_income.get(), self.income.get());
            self.display_income.set(a + (b - a) * (tick_delta * timestep) / INCOME_INTERVAL_MS);
        }
    }
}

impl IResolveOrder for PlayerStatistics {
    fn resolve_order(&self, _self_: &ActorRef, order: &Order) {
        if !order.order_string.starts_with("Dev") {
            add(&self.order_count, 1);
        }
    }
}

impl IWorldLoaded for PlayerStatistics {
    fn world_loaded(&self, _world: &Rc<World>) {
        if !self.army_graph_disabled.get() {
            self.army_samples.borrow_mut().push(self.army_value.get());
        }
        if !self.income_graph_disabled.get() {
            self.income_samples.borrow_mut().push(self.income.get());
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct UpdatesPlayerStatisticsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub add_to_army_value: bool,
    pub add_to_assets_value: bool,
    pub override_actor: Option<String>,
}

impl Default for UpdatesPlayerStatisticsInfo {
    fn default() -> Self {
        UpdatesPlayerStatisticsInfo {
            instance_name: None,
            add_to_army_value: false,
            add_to_assets_value: true,
            override_actor: None,
        }
    }
}

pub struct UpdatesPlayerStatistics {
    info: Rc<UpdatesPlayerStatisticsInfo>,
    actor_name: String,
    cost: i32,
    player_stats: RefCell<Option<Rc<PlayerStatistics>>>,
    included_in_army_value: Cell<bool>,
    included_in_assets_value: Cell<bool>,
}

register_traits!(UpdatesPlayerStatistics: INotifyKilled, INotifyCreated, INotifyOwnerChanged, INotifyActorDisposing);
info_impl!(UpdatesPlayerStatisticsInfo => |info, init| {
    let self_ = &init.self_;
    Some(Rc::new(UpdatesPlayerStatistics {
        actor_name: info.override_actor.clone().unwrap_or_else(|| self_.info.name.clone()),
        cost: self_.info.trait_info::<ValuedInfo>().map_or(0, |v| v.cost),
        player_stats: RefCell::new(None),
        included_in_army_value: Cell::new(false),
        included_in_assets_value: Cell::new(false),
        info,
    }))
});

fn stats_of(player: &PlayerRef) -> Option<Rc<PlayerStatistics>> {
    player.player_actor().trait_or_default::<PlayerStatistics>()
}

impl UpdatesPlayerStatistics {
    fn remove_values(&self, stats: &PlayerStatistics) {
        if self.included_in_army_value.replace(false) {
            add(&stats.army_value, -self.cost);
            stats.change_unit_count(&self.actor_name, -1);
        }
        if self.included_in_assets_value.replace(false) {
            add(&stats.assets_value, -self.cost);
        }
    }

    fn stats(&self) -> Option<Rc<PlayerStatistics>> {
        self.player_stats.borrow().clone()
    }
}

impl INotifyKilled for UpdatesPlayerStatistics {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        let owner = self_.owner();
        let Some(stats) = self.stats().filter(|_| owner.win_state.get() == WinState::Undefined) else {
            return;
        };
        self.remove_values(&stats);
        add(&stats.deaths_cost, self.cost);
        let Some(attacker) = e.attacker.as_ref().filter(|a| a.id != self_.id) else {
            return;
        };
        let Some(attacker_stats) = stats_of(&attacker.owner()) else {
            return;
        };
        if self_.info.has_trait_info::<BuildingInfo>() {
            if !owner.non_combatant {
                add(&attacker_stats.buildings_killed, 1);
            }
            add(&stats.buildings_dead, 1);
        } else if !self_.traits_implementing::<dyn IPositionable>().is_empty() {
            if !owner.non_combatant {
                add(&attacker_stats.units_killed, 1);
            }
            add(&stats.units_dead, 1);
        }
        if !owner.non_combatant {
            add(&attacker_stats.kills_cost, self.cost);
        }
    }
}

impl INotifyCreated for UpdatesPlayerStatistics {
    fn created(&self, self_: &ActorRef) {
        let Some(stats) = stats_of(&self_.owner()) else {
            return;
        };
        self.included_in_army_value.set(self.info.add_to_army_value);
        if self.info.add_to_army_value {
            add(&stats.army_value, self.cost);
            stats.change_unit_count(&self.actor_name, 1);
        }
        self.included_in_assets_value.set(self.info.add_to_assets_value);
        if self.info.add_to_assets_value {
            add(&stats.assets_value, self.cost);
        }
        *self.player_stats.borrow_mut() = Some(stats);
    }
}

impl INotifyOwnerChanged for UpdatesPlayerStatistics {
    fn on_owner_changed(&self, _self_: &ActorRef, _old_owner: &PlayerRef, new_owner: &PlayerRef) {
        let (Some(stats), Some(new_stats)) = (self.stats(), stats_of(new_owner)) else {
            return;
        };
        if self.included_in_army_value.get() {
            add(&stats.army_value, -self.cost);
            add(&new_stats.army_value, self.cost);
            stats.change_unit_count(&self.actor_name, -1);
            new_stats.change_unit_count(&self.actor_name, 1);
        }
        if self.included_in_assets_value.get() {
            add(&stats.assets_value, -self.cost);
            add(&new_stats.assets_value, self.cost);
        }
        *self.player_stats.borrow_mut() = Some(new_stats);
    }
}

impl INotifyActorDisposing for UpdatesPlayerStatistics {
    fn disposing(&self, _self_: &ActorRef) {
        if let Some(stats) = self.stats() {
            self.remove_values(&stats);
        }
    }
}
