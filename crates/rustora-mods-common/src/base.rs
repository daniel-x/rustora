// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `DeveloperMode`, `DebugPauseState`, `MapOptions`, `MapBuildRadius`, power, base and line-build traits, and `BuildingUtils`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_core::field_flags;
use rustora_core::world_types::{CPos, CVec, WAngle, WDist};
use rustora_derive::FieldLoad;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::actor_init::{ActorInit, ActorInits, FacingInit, LocationInit, OwnerInit};
use rustora_engine::conditional::*;
use rustora_engine::damage::{AttackInfo, DamageTypes};
use rustora_engine::net_collections::NetMap;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{conditional_plumbing, conditional_simple, info_impl, load_info, register_traits, value_init};

use crate::actor_map::{ById, WorldExt};
use crate::building::{BuildingInfluence, BuildingInfo};
use crate::decorations::{AnnotationCircle, IRenderCircles};
use crate::interfaces::*;
use crate::player::PlayerResources;
use crate::resources::ResourceLayer;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("DeveloperMode", |n| load_info!(DeveloperModeInfo, n));
    r.register("DebugPauseState", |n| load_info!(DebugPauseStateInfo, n));
    r.register("MapOptions", |n| load_info!(MapOptionsInfo, n));
    r.register("MapBuildRadius", |n| load_info!(MapBuildRadiusInfo, n));
    r.register("MapCreeps", |n| load_info!(MapCreepsInfo, n));
    r.register("PowerManager", |n| load_info!(PowerManagerInfo, n));
    r.register("Power", |n| load_info!(PowerInfo, n));
    r.register("ScalePowerWithHealth", |n| load_info!(ScalePowerWithHealthInfo, n));
    r.register("AffectedByPowerOutage", |n| load_info!(AffectedByPowerOutageInfo, n));
    r.register("GrantConditionOnPowerState", |n| load_info!(GrantConditionOnPowerStateInfo, n));
    r.register("BaseProvider", |n| load_info!(BaseProviderInfo, n));
    r.register("BaseBuilding", |n| load_info!(BaseBuildingInfo, n));
    r.register("GivesBuildableArea", |n| load_info!(GivesBuildableAreaInfo, n));
    r.register("RequiresBuildableArea", |n| load_info!(RequiresBuildableAreaInfo, n));
    r.register("LineBuild", |n| load_info!(LineBuildInfo, n));
    r.register("LineBuildNode", |n| load_info!(LineBuildNodeInfo, n));
    r.register("FreeActor", |n| load_info!(FreeActorInfo, n));
    r.register("PlayerExperience", |n| load_info!(PlayerExperienceInfo, n));
    r.register("RepairableBuilding", |n| load_info!(RepairableBuildingInfo, n));
    r.register("AllyRepair", |n| load_info!(AllyRepairInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct DeveloperModeInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub checkbox_enabled: bool,
    pub cash: i32,
    pub resource_growth: i32,
    pub fast_build: bool,
    pub fast_charge: bool,
    pub disable_shroud: bool,
    pub unlimited_power: bool,
    pub build_anywhere: bool,
    pub path_debug: bool,
}

impl Default for DeveloperModeInfo {
    fn default() -> Self {
        DeveloperModeInfo {
            instance_name: None,
            checkbox_enabled: false,
            cash: 20000,
            resource_growth: 100,
            fast_build: false,
            fast_charge: false,
            disable_shroud: false,
            unlimited_power: false,
            build_anywhere: false,
            path_debug: false,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct DeveloperMode {
    info: Rc<DeveloperModeInfo>,
    enabled: Cell<bool>,
    fast_charge: Cell<bool>,
    all_tech: Cell<bool>,
    fast_build: Cell<bool>,
    disable_shroud: Cell<bool>,
    path_debug: Cell<bool>,
    unlimited_power: Cell<bool>,
    build_anywhere: Cell<bool>,
    enable_all: Cell<bool>,
}

register_traits!(DeveloperMode: IResolveOrder, ISync, INotifyCreated, IUnlocksRenderPlayer);
info_impl!(DeveloperModeInfo => |info, _| Some(Rc::new(DeveloperMode {
    enabled: Cell::new(false),
    fast_charge: Cell::new(info.fast_charge),
    all_tech: Cell::new(false),
    fast_build: Cell::new(info.fast_build),
    disable_shroud: Cell::new(info.disable_shroud),
    path_debug: Cell::new(info.path_debug),
    unlimited_power: Cell::new(info.unlimited_power),
    build_anywhere: Cell::new(info.build_anywhere),
    enable_all: Cell::new(false),
    info,
})));

impl ISync for DeveloperMode {
    fn sync_hash(&self) -> i32 {
        sync_bool(self.fast_charge.get())
            ^ sync_bool(self.all_tech.get())
            ^ sync_bool(self.fast_build.get())
            ^ sync_bool(self.disable_shroud.get())
            ^ sync_bool(self.path_debug.get())
            ^ sync_bool(self.unlimited_power.get())
            ^ sync_bool(self.build_anywhere.get())
    }
}

impl INotifyCreated for DeveloperMode {
    fn created(&self, self_: &ActorRef) {
        let w = self_.world();
        let non_bot_players = w.lobby_info.clients.iter().filter(|c| !c.is_bot() && !c.is_observer()).count();
        self.enabled
            .set(non_bot_players == 1 || w.lobby_info.global_settings.option_bool("cheats", self.info.checkbox_enabled));
    }
}

impl IUnlocksRenderPlayer for DeveloperMode {
    fn render_player_unlocked(&self) -> bool {
        self.enabled.get()
    }
}

impl DeveloperMode {
    pub fn enabled(&self) -> bool {
        self.enabled.get()
    }
    pub fn fast_charge(&self) -> bool {
        self.enabled.get() && self.fast_charge.get()
    }
    pub fn all_tech(&self) -> bool {
        self.enabled.get() && self.all_tech.get()
    }
    pub fn fast_build(&self) -> bool {
        self.enabled.get() && self.fast_build.get()
    }
    pub fn disable_shroud(&self) -> bool {
        self.enabled.get() && self.disable_shroud.get()
    }
    pub fn path_debug(&self) -> bool {
        self.enabled.get() && self.path_debug.get()
    }
    pub fn unlimited_power(&self) -> bool {
        self.enabled.get() && self.unlimited_power.get()
    }
    pub fn build_anywhere(&self) -> bool {
        self.enabled.get() && self.build_anywhere.get()
    }
}

fn toggle(c: &Cell<bool>) {
    c.set(!c.get());
}

impl IResolveOrder for DeveloperMode {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if !self.enabled.get() {
            return;
        }
        let amount = if order.extra_data != 0 {
            order.extra_data as i32
        } else {
            self.info.cash
        };
        match order.order_string.as_str() {
            "DevAll" => {
                toggle(&self.enable_all);
                let e = self.enable_all.get();
                for c in [
                    &self.all_tech,
                    &self.fast_charge,
                    &self.fast_build,
                    &self.disable_shroud,
                    &self.unlimited_power,
                    &self.build_anywhere,
                ] {
                    c.set(e);
                }
                if e {
                    self_.trait_::<PlayerResources>().change_cash(amount);
                }
                self_.owner().shroud().set_disabled(self.disable_shroud());
            }
            "DevEnableTech" => toggle(&self.all_tech),
            "DevFastCharge" => toggle(&self.fast_charge),
            "DevFastBuild" => toggle(&self.fast_build),
            "DevGiveCash" => {
                self_.trait_::<PlayerResources>().change_cash(amount);
            }
            "DevGiveCashAll" => {
                for p in self_.world().players() {
                    if p.playable {
                        p.player_actor().trait_::<PlayerResources>().change_cash(amount);
                    }
                }
            }
            "DevGrowResources" => {
                for (a, t) in self_.world().actors_with_trait::<dyn ISeedableResource>() {
                    for _ in 0..self.info.resource_growth {
                        t.seed(&a);
                    }
                }
            }
            "DevVisibility" => {
                toggle(&self.disable_shroud);
                self_.owner().shroud().set_disabled(self.disable_shroud());
            }
            "DevPathDebug" => toggle(&self.path_debug),
            "DevGiveExploration" => self_.owner().shroud().explore_all(),
            "DevResetExploration" => self_.owner().shroud().reset_exploration(),
            "DevUnlimitedPower" => toggle(&self.unlimited_power),
            "DevBuildAnywhere" => toggle(&self.build_anywhere),
            "DevPlayerExperience" => {
                if let Some(e) = self_.owner().player_actor().trait_or_default::<PlayerExperience>() {
                    e.give_experience(order.extra_data as i32);
                }
            }
            "DevKill" => {
                if order.target.target_type() == TargetType::Actor {
                    let a = order.target.actor().unwrap().clone();
                    let names: Vec<&str> = order.target_string.as_deref().unwrap_or("").split(' ').collect();
                    crate::health::kill_actor(&a, Some(&a), &DamageTypes::new(&names));
                }
            }
            "DevDispose" if order.target.target_type() == TargetType::Actor => {
                order.target.actor().unwrap().dispose();
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PlayerExperienceInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(Default, rustora_engine::persist::Persist)]
pub struct PlayerExperience {
    experience: Cell<i32>,
}

register_traits!(PlayerExperience: ISync);
info_impl!(PlayerExperienceInfo => |_, _| Some(Rc::new(PlayerExperience::default())));

impl ISync for PlayerExperience {
    fn sync_hash(&self) -> i32 {
        self.experience.get()
    }
}

impl PlayerExperience {
    pub fn experience(&self) -> i32 {
        self.experience.get()
    }

    pub fn give_experience(&self, amount: i32) {
        self.experience.set(self.experience.get().wrapping_add(amount));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MapOptionsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub short_game_checkbox_enabled: bool,
    pub tech_level: String,
    pub game_speed: Option<String>,
}

impl Default for MapOptionsInfo {
    fn default() -> Self {
        MapOptionsInfo {
            instance_name: None,
            short_game_checkbox_enabled: true,
            tech_level: "unrestricted".into(),
            game_speed: None,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct MapOptions {
    info: Rc<MapOptionsInfo>,
    short_game: Cell<bool>,
    tech_level: RefCell<String>,
}

register_traits!(MapOptions: INotifyCreated);
info_impl!(MapOptionsInfo => |info, _| Some(Rc::new(MapOptions { info, short_game: Cell::new(false), tech_level: RefCell::new(String::new()) })));

impl INotifyCreated for MapOptions {
    fn created(&self, self_: &ActorRef) {
        let w = self_.world();
        let gs = &w.lobby_info.global_settings;
        self.short_game
            .set(gs.option_bool("shortgame", self.info.short_game_checkbox_enabled));
        *self.tech_level.borrow_mut() = gs.option_str("techlevel", &self.info.tech_level).to_string();
    }
}

impl MapOptions {
    pub fn short_game(&self) -> bool {
        self.short_game.get()
    }

    pub fn tech_level(&self) -> String {
        self.tech_level.borrow().clone()
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MapBuildRadiusInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub ally_build_radius_checkbox_enabled: bool,
    pub build_radius_checkbox_enabled: bool,
}

impl Default for MapBuildRadiusInfo {
    fn default() -> Self {
        MapBuildRadiusInfo {
            instance_name: None,
            ally_build_radius_checkbox_enabled: true,
            build_radius_checkbox_enabled: true,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct MapBuildRadius {
    info: Rc<MapBuildRadiusInfo>,
    pub ally_build_radius_enabled: Cell<bool>,
    pub build_radius_enabled: Cell<bool>,
}

register_traits!(MapBuildRadius: INotifyCreated);
info_impl!(MapBuildRadiusInfo => |info, _| Some(Rc::new(MapBuildRadius { info, ally_build_radius_enabled: Cell::new(false), build_radius_enabled: Cell::new(false) })));

impl INotifyCreated for MapBuildRadius {
    fn created(&self, self_: &ActorRef) {
        let w = self_.world();
        let gs = &w.lobby_info.global_settings;
        self.ally_build_radius_enabled
            .set(gs.option_bool("allybuild", self.info.ally_build_radius_checkbox_enabled));
        self.build_radius_enabled
            .set(gs.option_bool("buildradius", self.info.build_radius_checkbox_enabled));
    }
}

/// `MapCreepsInfo`: the lobby option `creeps`.
#[derive(Clone, Debug, FieldLoad)]
pub struct MapCreepsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub checkbox_enabled: bool,
}

impl Default for MapCreepsInfo {
    fn default() -> Self {
        MapCreepsInfo {
            instance_name: None,
            checkbox_enabled: true,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct MapCreeps {
    info: Rc<MapCreepsInfo>,
    pub enabled: Cell<bool>,
}

register_traits!(MapCreeps: INotifyCreated);
info_impl!(MapCreepsInfo => |info, _| Some(Rc::new(MapCreeps { info, enabled: Cell::new(false) })));

impl INotifyCreated for MapCreeps {
    fn created(&self, self_: &ActorRef) {
        let gs = &self_.world().lobby_info.global_settings;
        self.enabled.set(gs.option_bool("creeps", self.info.checkbox_enabled));
    }
}

/// Bit flags of `PowerState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerState(pub i32);

impl PowerState {
    #[allow(non_upper_case_globals)]
    pub const Normal: PowerState = PowerState(1);
    #[allow(non_upper_case_globals)]
    pub const Low: PowerState = PowerState(2);
    #[allow(non_upper_case_globals)]
    pub const Critical: PowerState = PowerState(4);
}

field_flags!(PowerState, [Normal, Low, Critical]);

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PowerManagerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub speech_notification: Option<String>,
    pub text_notification: Option<String>,
    pub advice_interval: i32,
}

#[derive(rustora_engine::persist::Persist)]
pub struct PowerManager {
    self_: ActorRef,
    #[persist(skip)]
    last_power_advice: Cell<Option<std::time::Instant>>,
    dev_mode: Rc<DeveloperMode>,
    power_drain: RefCell<NetMap<u32, i32>>,
    power_provided: Cell<i32>,
    power_drained: Cell<i32>,
    power_outage_remaining_ticks: Cell<i32>,
    power_outage_total_ticks: Cell<i32>,
    is_low_power: Cell<bool>,
    was_low_power: Cell<bool>,
    /// `PlayLowPowerNotification`, settable by scripts.
    pub play_low_power_notification: Cell<bool>,
    was_hack_enabled: Cell<bool>,
}

register_traits!(PowerManager: INotifyCreated, ITick, ISync, IResolveOrder);
info_impl!(PowerManagerInfo => |info, init| {
    let dev_mode = init.self_.trait_::<DeveloperMode>();
    Some(Rc::new(PowerManager {
        last_power_advice: Cell::new(None),
        self_: init.self_.clone(),
        was_hack_enabled: Cell::new(dev_mode.unlimited_power()),
        dev_mode,
        power_drain: RefCell::new(NetMap::new()),
        power_provided: Cell::new(0),
        power_drained: Cell::new(0),
        power_outage_remaining_ticks: Cell::new(0),
        power_outage_total_ticks: Cell::new(0),
        is_low_power: Cell::new(false),
        was_low_power: Cell::new(false),
        play_low_power_notification: Cell::new(info.advice_interval > 0),
    }))
});

impl ISync for PowerManager {
    fn sync_hash(&self) -> i32 {
        self.power_provided.get() ^ self.power_drained.get()
    }
}

impl INotifyCreated for PowerManager {
    fn created(&self, self_: &ActorRef) {
        let me = self_.clone();
        self_
            .world()
            .add_frame_end_task(Box::new(move |_| me.trait_::<PowerManager>().update_power_requiring_actors()));
    }
}

impl ITick for PowerManager {
    fn tick(&self, _self_: &ActorRef) {
        if self.was_hack_enabled.get() != self.dev_mode.unlimited_power() {
            self.power_provided.set(0);
            self.power_drained.set(0);
            if !self.dev_mode.unlimited_power() {
                for v in self.power_drain.borrow().values() {
                    if *v > 0 {
                        self.power_provided.set(self.power_provided.get() + *v);
                    } else if *v < 0 {
                        self.power_drained.set(self.power_drained.get() - *v);
                    }
                }
            }
            self.was_hack_enabled.set(self.dev_mode.unlimited_power());
            self.update_power_state();
        }
        self.play_low_power_advice();
        let r = self.power_outage_remaining_ticks.get();
        if r > 0 {
            self.power_outage_remaining_ticks.set(r - 1);
            if r - 1 == 0 {
                self.update_power_outage_actors();
            }
        }
    }
}

impl IResolveOrder for PowerManager {
    fn resolve_order(&self, _self_: &ActorRef, order: &Order) {
        if self.dev_mode.enabled() && order.order_string == "PowerOutage" {
            self.trigger_power_outage(order.extra_data as i32);
        }
    }
}

impl PowerManager {
    pub fn power_provided(&self) -> i32 {
        self.power_provided.get()
    }

    pub fn power_drained(&self) -> i32 {
        self.power_drained.get()
    }

    pub fn excess_power(&self) -> i32 {
        self.power_provided.get() - self.power_drained.get()
    }

    pub fn power_outage_remaining_ticks(&self) -> i32 {
        self.power_outage_remaining_ticks.get()
    }

    pub fn power_outage_total_ticks(&self) -> i32 {
        self.power_outage_total_ticks.get()
    }

    pub fn update_actor(&self, a: &ActorRef) {
        if !a.is_in_world() {
            return;
        }
        let old = self.power_drain.borrow().get(&a.id).copied().unwrap_or(0);
        let amount: i32 = a
            .traits_implementing::<Power>()
            .iter()
            .filter(|t| !t.state.is_trait_disabled())
            .map(|p| p.get_enabled_power())
            .sum();
        self.power_drain.borrow_mut().insert(a.id, amount);
        if amount == old || self.dev_mode.unlimited_power() {
            return;
        }
        if old > 0 {
            self.power_provided.set(self.power_provided.get() - old);
        } else if old < 0 {
            self.power_drained.set(self.power_drained.get() + old);
        }
        if amount > 0 {
            self.power_provided.set(self.power_provided.get() + amount);
        } else if amount < 0 {
            self.power_drained.set(self.power_drained.get() - amount);
        }
        self.update_power_state();
    }

    pub fn remove_actor(&self, a: &ActorRef) {
        if a.is_in_world() {
            return;
        }
        let Some(amount) = self.power_drain.borrow_mut().remove(&a.id) else {
            return;
        };
        if self.dev_mode.unlimited_power() {
            return;
        }
        if amount > 0 {
            self.power_provided.set(self.power_provided.get() - amount);
        } else if amount < 0 {
            self.power_drained.set(self.power_drained.get() + amount);
        }
        self.update_power_state();
    }

    /// The notification part of `ITick.Tick`. It speaks once per `AdviceInterval` of real time while power is low.
    fn play_low_power_advice(&self) {
        let info = self.self_.info.trait_info::<PowerManagerInfo>().expect("PowerManager");
        let due = self
            .last_power_advice
            .get()
            .is_none_or(|t| t.elapsed().as_millis() > info.advice_interval as u128);
        if self.play_low_power_notification.get() && self.is_low_power.get() && due {
            rustora_engine::sound::notify_owner(
                &self.self_,
                "Speech",
                info.speech_notification.as_deref(),
                info.text_notification.as_deref(),
            );
            self.last_power_advice.set(Some(std::time::Instant::now()));
        }
    }

    fn update_power_state(&self) {
        self.is_low_power.set(self.excess_power() < 0);
        if self.is_low_power.get() != self.was_low_power.get() {
            self.update_power_requiring_actors();
        }
        if self.is_low_power.get() && !self.was_low_power.get() {
            self.last_power_advice.set(None);
        }
        self.was_low_power.set(self.is_low_power.get());
    }

    pub fn power_state(&self) -> PowerState {
        let (p, d) = (self.power_provided.get(), self.power_drained.get());
        if p >= d {
            PowerState::Normal
        } else if p > d / 2 {
            PowerState::Low
        } else {
            PowerState::Critical
        }
    }

    pub fn trigger_power_outage(&self, total_ticks: i32) {
        self.power_outage_total_ticks.set(total_ticks);
        self.power_outage_remaining_ticks.set(total_ticks);
        self.update_power_outage_actors();
    }

    fn valid_owned(&self, a: &ActorRef) -> bool {
        !a.is_dead() && a.is_in_world() && a.owner().player_actor().id == self.self_.owner().player_actor().id
    }

    fn update_power_outage_actors(&self) {
        for (a, t) in self.self_.world().actors_with_trait::<AffectedByPowerOutage>() {
            if self.valid_owned(&a) {
                t.update_status(&a);
            }
        }
    }

    fn update_power_requiring_actors(&self) {
        for (a, t) in self.self_.world().actors_with_trait::<dyn INotifyPowerLevelChanged>() {
            if self.valid_owned(&a) {
                t.power_level_changed(&a);
            }
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PowerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub amount: i32,
}

#[derive(rustora_engine::persist::Persist)]
pub struct Power {
    info: Rc<PowerInfo>,
    state: ConditionalState,
    self_: ActorRef,
    player_power: RefCell<Rc<PowerManager>>,
}

register_traits!(Power: INotifyAddedToWorld, INotifyRemovedFromWorld, INotifyOwnerChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(PowerInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(Power {
        info,
        state,
        self_: init.self_.clone(),
        player_power: RefCell::new(init.self_.owner().player_actor().trait_::<PowerManager>()),
    }))
});

impl ConditionalHooks for Power {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, self_: &ActorRef) {
        self.player_power.borrow().clone().update_actor(self_);
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        self.player_power.borrow().clone().update_actor(self_);
    }
}

conditional_plumbing!(Power);

impl INotifyCreated for Power {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl Power {
    pub fn get_enabled_power(&self) -> i32 {
        let mods = self.self_.traits_implementing::<dyn IPowerModifier>();
        util::apply_percentage_modifiers(self.info.amount, mods.iter().map(|m| m.get_power_modifier()))
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl INotifyAddedToWorld for Power {
    fn added_to_world(&self, self_: &ActorRef) {
        self.player_power.borrow().clone().update_actor(self_);
    }
}

impl INotifyRemovedFromWorld for Power {
    fn removed_from_world(&self, self_: &ActorRef) {
        self.player_power.borrow().clone().remove_actor(self_);
    }
}

impl INotifyOwnerChanged for Power {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        self.player_power.borrow().clone().remove_actor(self_);
        *self.player_power.borrow_mut() = new_owner.player_actor().trait_::<PowerManager>();
        self.player_power.borrow().clone().update_actor(self_);
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ScalePowerWithHealthInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct ScalePowerWithHealth {
    health: Rc<dyn IHealth>,
    power: RefCell<Rc<PowerManager>>,
}

register_traits!(ScalePowerWithHealth: IPowerModifier, INotifyDamage, INotifyOwnerChanged);
info_impl!(ScalePowerWithHealthInfo => |_, init| Some(Rc::new(ScalePowerWithHealth {
    health: init.self_.trait_::<dyn IHealth>(),
    power: RefCell::new(init.self_.owner().player_actor().trait_::<PowerManager>()),
})));

impl IPowerModifier for ScalePowerWithHealth {
    fn get_power_modifier(&self) -> i32 {
        (100i64 * self.health.hp() as i64 / self.health.max_hp() as i64) as i32
    }
}

impl INotifyDamage for ScalePowerWithHealth {
    fn damaged(&self, self_: &ActorRef, _e: &AttackInfo) {
        self.power.borrow().clone().update_actor(self_);
    }
}

impl INotifyOwnerChanged for ScalePowerWithHealth {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        *self.power.borrow_mut() = new_owner.player_actor().trait_::<PowerManager>();
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct AffectedByPowerOutageInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub condition: Option<String>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct AffectedByPowerOutage {
    info: Rc<AffectedByPowerOutageInfo>,
    state: ConditionalState,
    player_power: RefCell<Rc<PowerManager>>,
    token: Cell<i32>,
}

register_traits!(AffectedByPowerOutage: INotifyOwnerChanged, INotifyCreated, INotifyAddedToWorld, IObservesVariables, IDisabledTrait, ISync, ISelectionBar);
info_impl!(AffectedByPowerOutageInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(AffectedByPowerOutage {
        info,
        state,
        player_power: RefCell::new(init.self_.owner().player_actor().trait_::<PowerManager>()),
        token: Cell::new(INVALID_CONDITION_TOKEN),
    }))
});

impl ConditionalHooks for AffectedByPowerOutage {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, self_: &ActorRef) {
        self.update_status(self_);
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        self.revoke(self_);
    }
}

conditional_plumbing!(AffectedByPowerOutage);

/// The remaining time of the power outage, in yellow.
impl ISelectionBar for AffectedByPowerOutage {
    fn value(&self, _self_: &ActorRef, _viewer: Option<&PlayerRef>) -> f32 {
        let power = self.player_power.borrow();
        let remaining = power.power_outage_remaining_ticks();
        if self.state.is_trait_disabled() || remaining <= 0 {
            return 0.0;
        }
        remaining as f32 / power.power_outage_total_ticks() as f32
    }

    fn color(&self) -> rustora_core::fieldloader::Color {
        rustora_core::fieldloader::Color::from_argb(255, 255, 255, 0)
    }
}

impl INotifyCreated for AffectedByPowerOutage {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl INotifyAddedToWorld for AffectedByPowerOutage {
    fn added_to_world(&self, self_: &ActorRef) {
        self.update_status(self_);
    }
}

impl AffectedByPowerOutage {
    pub fn update_status(&self, self_: &ActorRef) {
        if !self.state.is_trait_disabled() && self.player_power.borrow().power_outage_remaining_ticks() > 0 {
            if self.token.get() == INVALID_CONDITION_TOKEN {
                self.token.set(self_.grant_condition(self.info.condition.as_deref().unwrap_or("")));
            }
        } else {
            self.revoke(self_);
        }
    }

    fn revoke(&self, self_: &ActorRef) {
        if self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
    }
}

impl INotifyOwnerChanged for AffectedByPowerOutage {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        *self.player_power.borrow_mut() = new_owner.player_actor().trait_::<PowerManager>();
        self.update_status(self_);
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GrantConditionOnPowerStateInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub condition: String,
    #[field(require)]
    pub valid_power_states: PowerState,
}

#[derive(rustora_engine::persist::Persist)]
pub struct GrantConditionOnPowerState {
    info: Rc<GrantConditionOnPowerStateInfo>,
    state: ConditionalState,
    player_power: RefCell<Option<Rc<PowerManager>>>,
    token: Cell<i32>,
}

register_traits!(GrantConditionOnPowerState: INotifyOwnerChanged, INotifyPowerLevelChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GrantConditionOnPowerStateInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(GrantConditionOnPowerState { info, state, player_power: RefCell::new(None), token: Cell::new(INVALID_CONDITION_TOKEN) }))
});

impl ConditionalHooks for GrantConditionOnPowerState {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, self_: &ActorRef) {
        self.update(self_);
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        self.update(self_);
    }
}

conditional_plumbing!(GrantConditionOnPowerState);

impl GrantConditionOnPowerState {
    fn update(&self, self_: &ActorRef) {
        let ps = self.player_power.borrow().as_ref().expect("created").power_state();
        let valid = !self.state.is_trait_disabled() && self.info.valid_power_states.0 & ps.0 == ps.0;
        if valid && self.token.get() == INVALID_CONDITION_TOKEN {
            self.token.set(self_.grant_condition(&self.info.condition));
        } else if !valid && self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
    }
}

impl INotifyCreated for GrantConditionOnPowerState {
    fn created(&self, self_: &ActorRef) {
        *self.player_power.borrow_mut() = Some(self_.owner().player_actor().trait_::<PowerManager>());
        conditional_created(self, self_);
        self.update(self_);
    }
}

impl INotifyPowerLevelChanged for GrantConditionOnPowerState {
    fn power_level_changed(&self, self_: &ActorRef) {
        self.update(self_);
    }
}

impl INotifyOwnerChanged for GrantConditionOnPowerState {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        *self.player_power.borrow_mut() = Some(new_owner.player_actor().trait_::<PowerManager>());
        self.update(self_);
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct BaseProviderInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub range: WDist,
    pub cooldown: i32,
    pub initial_delay: i32,
    pub circle_ready_color: rustora_core::fieldloader::Color,
    pub circle_blocked_color: rustora_core::fieldloader::Color,
    pub circle_width: f32,
    pub circle_border_color: rustora_core::fieldloader::Color,
    pub circle_border_width: f32,
}

impl Default for BaseProviderInfo {
    fn default() -> Self {
        BaseProviderInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            range: WDist::from_cells(10),
            cooldown: 0,
            initial_delay: 0,
            circle_ready_color: rustora_core::fieldloader::Color::from_argb(128, 255, 255, 255),
            circle_blocked_color: rustora_core::fieldloader::Color::from_argb(128, 255, 0, 0),
            circle_width: 1.0,
            circle_border_color: rustora_core::fieldloader::Color::from_argb(96, 0, 0, 0),
            circle_border_width: 3.0,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct BaseProvider {
    pub info: Rc<BaseProviderInfo>,
    state: ConditionalState,
    dev_mode: Rc<DeveloperMode>,
    progress: Cell<i32>,
}

register_traits!(BaseProvider: ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync, IRenderCircles);
info_impl!(BaseProviderInfo => |info, init| {
    let state = ConditionalState::new_pausable(&info.base);
    Some(Rc::new(BaseProvider {
        progress: Cell::new(info.initial_delay),
        info,
        state,
        dev_mode: init.self_.owner().player_actor().trait_::<DeveloperMode>(),
    }))
});

conditional_simple!(BaseProvider);

impl ITick for BaseProvider {
    fn tick(&self, _self_: &ActorRef) {
        if self.progress.get() > 0 {
            self.progress.set(self.progress.get() - 1);
        }
    }
}

impl BaseProvider {
    pub fn begin_cooldown(&self) {
        self.progress.set(self.info.cooldown);
    }

    pub fn ready(&self) -> bool {
        if self.state.is_trait_disabled() || self.state.is_trait_paused() {
            return false;
        }
        self.dev_mode.fast_build() || self.progress.get() == 0
    }

    /// `ValidRenderPlayer`: the build radius is on, and the viewer owns the actor, or is an ally while allies may
    /// build in it.
    fn valid_render_player(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> bool {
        let Some(radius) = self_.world().world_actor().trait_or_default::<MapBuildRadius>() else {
            return false;
        };
        let Some(viewer) = viewer else {
            return false;
        };
        radius.build_radius_enabled.get()
            && (Rc::ptr_eq(&self_.owner(), viewer)
                || (radius.ally_build_radius_enabled.get() && self_.owner().is_allied_with(Some(&**viewer))))
    }

    /// `RangeCircleRenderables`.
    pub fn range_circle(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> Option<AnnotationCircle> {
        if self.state.is_trait_disabled() || !self.valid_render_player(self_, viewer) {
            return None;
        }
        Some(AnnotationCircle {
            center: self_.center_position(),
            radius: self.info.range,
            color: if self.ready() {
                self.info.circle_ready_color
            } else {
                self.info.circle_blocked_color
            },
            width: self.info.circle_width,
            border_color: self.info.circle_border_color,
            border_width: self.info.circle_border_width,
            trail: None,
        })
    }
}

impl IRenderCircles for BaseProvider {
    fn circles(&self, self_: &ActorRef, viewer: Option<&PlayerRef>, selected: bool, out: &mut Vec<AnnotationCircle>) {
        if selected {
            out.extend(self.range_circle(self_, viewer));
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct BaseBuildingInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct BaseBuilding;
register_traits!(BaseBuilding);
info_impl!(BaseBuildingInfo => |_, _| Some(Rc::new(BaseBuilding)));

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GivesBuildableAreaInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub area_types: IndexSet<String>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct GivesBuildableArea {
    info: Rc<GivesBuildableAreaInfo>,
    state: ConditionalState,
}

register_traits!(GivesBuildableArea: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GivesBuildableAreaInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(GivesBuildableArea { info, state }))
});

conditional_simple!(GivesBuildableArea);

#[derive(Clone, Debug, FieldLoad)]
pub struct RequiresBuildableAreaInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub area_types: IndexSet<String>,
    pub adjacent: i32,
}

impl Default for RequiresBuildableAreaInfo {
    fn default() -> Self {
        RequiresBuildableAreaInfo {
            instance_name: None,
            area_types: IndexSet::new(),
            adjacent: 2,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct RequiresBuildableArea;
register_traits!(RequiresBuildableArea);
info_impl!(RequiresBuildableAreaInfo => |_, _| Some(Rc::new(RequiresBuildableArea)));

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, rustora_engine::persist::Persist)]
#[persist(value)]
pub enum LineBuildDirection {
    #[default]
    Unset,
    X,
    Y,
}

value_init!(LineBuildDirectionInit, LineBuildDirection);

/// `LineBuildParentInit`: parents by actor, or by map actor name.
#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
pub struct LineBuildParentInit {
    pub actors: Option<Vec<Option<ActorRef>>>,
    pub names: Vec<String>,
}

impl ActorInit for LineBuildParentInit {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct LineBuildInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub range: i32,
    pub node_types: IndexSet<String>,
    pub segment_type: Option<String>,
    pub segments_require_node: bool,
}

impl Default for LineBuildInfo {
    fn default() -> Self {
        LineBuildInfo {
            instance_name: None,
            range: 5,
            node_types: IndexSet::from(["wall".to_string()]),
            segment_type: None,
            segments_require_node: false,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct LineBuild {
    info: Rc<LineBuildInfo>,
    parent_nodes: Vec<ActorRef>,
    segments: RefCell<rustora_engine::net_collections::NetSet<ById>>,
}

register_traits!(LineBuild: INotifyKilled, INotifyAddedToWorld, INotifyRemovedFromWorld, INotifyLineBuildSegmentsChanged);
info_impl!(LineBuildInfo => |info, init| {
    let parent_nodes = match init.inits.get::<LineBuildParentInit>() {
        Some(LineBuildParentInit { actors: Some(v), .. }) => v.iter().flatten().cloned().collect(),
        Some(p) => {
            let w = init.world();
            let sma = w.world_actor().trait_::<crate::world::SpawnMapActors>();
            let actors = sma.actors.borrow();
            p.names.iter().filter_map(|n| actors.get(n).cloned()).collect()
        }
        None => Vec::new(),
    };
    Some(Rc::new(LineBuild { info, parent_nodes, segments: RefCell::new(Default::default()) }))
});

impl INotifyLineBuildSegmentsChanged for LineBuild {
    fn segment_added(&self, _self_: &ActorRef, segment: &ActorRef) {
        self.segments.borrow_mut().insert(ById(segment.clone()));
    }

    fn segment_removed(&self, _self_: &ActorRef, segment: &ActorRef) {
        self.segments.borrow_mut().remove(&ById(segment.clone()));
    }
}

impl INotifyAddedToWorld for LineBuild {
    fn added_to_world(&self, self_: &ActorRef) {
        for parent in &self.parent_nodes {
            if !parent.disposed() {
                for n in parent.traits_implementing::<dyn INotifyLineBuildSegmentsChanged>() {
                    n.segment_added(parent, self_);
                }
            }
        }
    }
}

impl INotifyRemovedFromWorld for LineBuild {
    fn removed_from_world(&self, self_: &ActorRef) {
        for parent in &self.parent_nodes {
            if !parent.disposed() {
                for n in parent.traits_implementing::<dyn INotifyLineBuildSegmentsChanged>() {
                    n.segment_removed(parent, self_);
                }
            }
        }
        if self.info.segments_require_node {
            let s = self.segments.borrow().to_vec();
            for a in s {
                a.0.dispose();
            }
        }
    }
}

impl INotifyKilled for LineBuild {
    fn killed(&self, _self_: &ActorRef, e: &AttackInfo) {
        if self.info.segments_require_node {
            let s = self.segments.borrow().to_vec();
            for a in s {
                crate::health::kill_actor(&a.0, e.attacker.as_ref(), &DamageTypes::default());
            }
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct LineBuildNodeInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub types: IndexSet<String>,
    pub connections: Vec<CVec>,
}

impl Default for LineBuildNodeInfo {
    fn default() -> Self {
        LineBuildNodeInfo {
            instance_name: None,
            types: IndexSet::from(["wall".to_string()]),
            connections: vec![CVec::new(1, 0), CVec::new(0, 1), CVec::new(-1, 0), CVec::new(0, -1)],
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct LineBuildNode;
register_traits!(LineBuildNode);
info_impl!(LineBuildNodeInfo => |_, _| Some(Rc::new(LineBuildNode)));

value_init!(FreeActorInit, bool);

/// `ParentActorInit`.
#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
pub struct ParentActorInit(pub ActorRef);

impl ActorInit for ParentActorInit {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct FreeActorInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub actor: String,
    pub spawn_offset: CVec,
    pub facing: WAngle,
    pub allow_respawn: bool,
}

#[derive(rustora_engine::persist::Persist)]
pub struct FreeActor {
    info: Rc<FreeActorInfo>,
    state: ConditionalState,
    allow_spawn: Cell<bool>,
}

register_traits!(FreeActor: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(FreeActorInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    let allow = init.inits.get_for::<FreeActorInit>(info.instance_name.as_deref()).is_none_or(|i| i.value);
    Some(Rc::new(FreeActor { info, state, allow_spawn: Cell::new(allow) }))
});

impl ConditionalHooks for FreeActor {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        if !self.allow_spawn.get() {
            return;
        }
        self.allow_spawn.set(self.info.allow_respawn);
        let (me, info) = (self_.clone(), self.info.clone());
        self_.world().add_frame_end_task(Box::new(move |w: &Rc<World>| {
            w.create_actor(
                &info.actor,
                ActorInits::new()
                    .with(ParentActorInit(me.clone()))
                    .with(LocationInit::new(me.location() + info.spawn_offset))
                    .with(OwnerInit::from_player(&me.owner()))
                    .with(FacingInit::new(info.facing)),
            );
        }));
    }
}

conditional_plumbing!(FreeActor);

impl INotifyCreated for FreeActor {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

/// `BuildingUtils.IsCellBuildable`. Replaceable actors are not used by RA.
pub fn is_cell_buildable(world: &World, cell: CPos, bi: &BuildingInfo, to_ignore: Option<&ActorRef>) -> bool {
    if !world.map().contains(cell) {
        return false;
    }
    if !bi.allow_invalid_placement {
        for a in world.actor_map().get_actors_at(cell) {
            if to_ignore.is_some_and(|i| i.id == a.id) {
                continue;
            }
            return false;
        }
        if world.world_actor().trait_::<BuildingInfluence>().any_building_at(cell) {
            return false;
        }
    } else if world.world_actor().trait_::<BuildingInfluence>().any_building_at(cell) {
        return false;
    }
    bi.terrain_types.contains(&world.map().terrain_type_info(cell).ty)
}

/// `BuildingUtils.CanPlaceBuilding`.
pub fn can_place_building(world: &World, cell: CPos, bi: &BuildingInfo, to_ignore: Option<&ActorRef>) -> bool {
    if bi.allow_invalid_placement {
        return true;
    }
    let resource_layer = world.world_actor().trait_or_default::<ResourceLayer>();
    bi.tiles(cell).iter().all(|t| {
        world.map().contains(*t)
            && (bi.allow_placement_on_resources || resource_layer.as_ref().is_none_or(|r| r.get_resource(*t).ty.is_none()))
            && is_cell_buildable(world, *t, bi, to_ignore)
    })
}

/// `AIUtils.ClearBlockersOrders`: move orders for idle units of `owner` standing on `tiles`.
pub fn clear_blockers_orders(world: &Rc<World>, tiles: &[CPos], owner: &PlayerRef, ignore_actor: Option<&ActorRef>) -> Vec<Order> {
    let map = world.map();
    let adjacent: Vec<CPos> = crate::util::expand_footprint(tiles, true)
        .into_iter()
        .filter(|c| !tiles.contains(c) && map.contains(*c))
        .collect();
    let mut out = Vec::new();
    for &tile in tiles {
        for a in world.actor_map().get_actors_at(tile) {
            if !a.is_owned_by(owner) || !a.is_idle() || ignore_actor.is_some_and(|i| i.id == a.id) {
                continue;
            }
            let move_cell = if let Some(mobile) = a.trait_or_default::<crate::mobile::Mobile>() {
                let location = a.location();
                let closest = adjacent
                    .iter()
                    .copied()
                    .filter(|t| mobile.can_enter_cell(*t, None, crate::locomotor::BlockedByActor::All))
                    .min_by_key(|t| (location - *t).length_squared());
                let Some(closest) = closest else {
                    continue;
                };
                closest
            } else if a.trait_or_default::<crate::aircraft::Aircraft>().is_some() {
                a.location()
            } else {
                continue;
            };
            let mut order = Order::new(
                "Move",
                Some(a.clone()),
                Target::from_cell(world, move_cell, rustora_engine::traits::subcell::FULL_CELL),
                false,
            );
            order.suppress_visual_feedback = true;
            out.push(order);
        }
    }
    out
}

/// `BuildingUtils.GetLineBuildCells`.
pub fn get_line_build_cells(
    world: &World,
    cell: CPos,
    ai: &ActorInfo,
    bi: &BuildingInfo,
    owner: &PlayerRef,
) -> Vec<(CPos, Option<ActorRef>)> {
    let lbi = ai.trait_info::<LineBuildInfo>().expect("LineBuild");
    let mut out = Vec::new();
    if is_cell_buildable(world, cell, bi, None) {
        out.push((cell, None));
    }
    let vecs = [CVec::new(1, 0), CVec::new(0, 1), CVec::new(-1, 0), CVec::new(0, -1)];
    let mut dirs = [0; 4];
    let mut connectors: [Option<ActorRef>; 4] = [None, None, None, None];
    for d in 0..4 {
        for i in 1..lbi.range {
            if dirs[d] != 0 {
                continue;
            }
            let (si, sbi) = match lbi.segment_type.as_deref().filter(|s| !s.is_empty()) {
                Some(s) => {
                    let si = world.rules().actor(s).expect("segment type");
                    let sbi = si.trait_info::<BuildingInfo>().expect("segment Building");
                    (si, sbi)
                }
                None => (
                    world.rules().actor(&ai.name).expect("actor"),
                    ai.trait_info::<BuildingInfo>().expect("Building"),
                ),
            };
            let _ = si;
            let c = cell + vecs[d] * i;
            if is_cell_buildable(world, c, &sbi, None) || !owner.shroud().is_explored(c) {
                continue;
            }
            connectors[d] = world.actor_map().get_actors_at(c).into_iter().find(|a| {
                a.info
                    .trait_infos::<LineBuildNodeInfo>()
                    .iter()
                    .any(|info| info.types.iter().any(|t| lbi.node_types.contains(t)) && info.connections.contains(&vecs[d]))
            });
            dirs[d] = if connectors[d].is_some() { i } else { -1 };
        }
        if dirs[d] > 0 {
            for i in 1..dirs[d] {
                out.push((cell + vecs[d] * i, connectors[d].clone()));
            }
        }
    }
    out
}

/// `BuildingInfo.FindBaseProvider`.
pub fn find_base_provider(bi: &BuildingInfo, world: &World, p: &PlayerRef, top_left: CPos) -> Option<Rc<BaseProvider>> {
    let center = world.map().center_of_cell(top_left) + bi.center_offset(world);
    let radius = world.world_actor().trait_or_default::<MapBuildRadius>()?;
    if !radius.build_radius_enabled.get() {
        return None;
    }
    let ally_build = radius.ally_build_radius_enabled.get();
    for (a, bp) in world.actors_with_trait::<BaseProvider>() {
        let valid_owner = a.is_owned_by(p) || (ally_build && a.owner().relationship_with(Some(p)) == PlayerRelationship::ALLY);
        if !valid_owner || !bp.ready() {
            continue;
        }
        if Target::from_pos(a.center_position()).is_in_range(center, bp.info.range) {
            return Some(bp);
        }
    }
    None
}

fn any_gives_buildable_area(actors: &[ActorRef], p: &PlayerRef, ally_build: bool, rba: &RequiresBuildableAreaInfo) -> bool {
    for a in actors {
        if !a.is_in_world() {
            continue;
        }
        if !a.is_owned_by(p) && (!ally_build || a.owner().relationship_with(Some(p)) != PlayerRelationship::ALLY) {
            continue;
        }
        let overlaps = a
            .traits_implementing::<GivesBuildableArea>()
            .iter()
            .filter(|g| !g.state.is_trait_disabled())
            .any(|g| g.info.area_types.iter().any(|t| rba.area_types.contains(t)));
        if overlaps {
            return true;
        }
    }
    false
}

/// `BuildingInfo.IsCloseEnoughToBase`.
pub fn is_close_enough_to_base(bi: &BuildingInfo, world: &World, p: &PlayerRef, ai: &ActorInfo, top_left: CPos) -> bool {
    let Some(rba) = ai.trait_info::<RequiresBuildableAreaInfo>() else {
        return true;
    };
    if p.player_actor().trait_::<DeveloperMode>().build_anywhere() {
        return true;
    }
    let radius = world.world_actor().trait_or_default::<MapBuildRadius>();
    if radius.as_ref().is_some_and(|r| r.build_radius_enabled.get())
        && bi.requires_base_provider
        && find_base_provider(bi, world, p, top_left).is_none()
    {
        return false;
    }
    let adjacent = rba.adjacent;
    let scan_start = world.map().clamp(top_left - CVec::new(adjacent, adjacent));
    let scan_end = world.map().clamp(top_left + bi.dimensions + CVec::new(adjacent, adjacent));
    let influence = world.world_actor().trait_::<BuildingInfluence>();
    let ally_build = radius.as_ref().is_some_and(|r| r.ally_build_radius_enabled.get());
    let am = world.actor_map();
    let mut candidates = Vec::new();
    for y in scan_start.y()..scan_end.y() {
        for x in scan_start.x()..scan_end.x() {
            let c = CPos::new(x, y);
            if any_gives_buildable_area(&am.get_actors_at(c), p, ally_build, &rba)
                || any_gives_buildable_area(&influence.get_buildings_at(c), p, ally_build, &rba)
            {
                candidates.push(c);
            }
        }
    }
    let tiles = bi.tiles(top_left);
    candidates.iter().any(|a| {
        tiles
            .iter()
            .any(|b| (a.x() - b.x()).abs() <= adjacent && (a.y() - b.y()).abs() <= adjacent)
    })
}

/// `Traits/Buildings/RepairableBuilding.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct RepairableBuildingInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub repairing_notification: Option<String>,
    pub repairing_text_notification: Option<String>,
    pub repairing_stopped_notification: Option<String>,
    pub repairing_stopped_text_notification: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub repair_percent: i32,
    pub repair_interval: i32,
    pub repair_step: i32,
    pub repair_damage_types: DamageTypes,
    pub repair_bonuses: Vec<i32>,
    pub cancel_when_disabled: bool,
    pub player_experience: i32,
    pub repair_condition: Option<String>,
}

impl Default for RepairableBuildingInfo {
    fn default() -> Self {
        RepairableBuildingInfo {
            repairing_notification: None,
            repairing_text_notification: None,
            repairing_stopped_notification: None,
            repairing_stopped_text_notification: None,
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            repair_percent: 20,
            repair_interval: 24,
            repair_step: 7,
            repair_damage_types: DamageTypes::default(),
            repair_bonuses: vec![100, 150, 175, 200, 220, 240, 260, 280, 300],
            cancel_when_disabled: false,
            player_experience: 0,
            repair_condition: None,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct RepairableBuilding {
    info: Rc<RepairableBuildingInfo>,
    state: ConditionalState,
    health: Rc<dyn IHealth>,
    repair_tokens: RefCell<Vec<i32>>,
    remaining_ticks: Cell<i32>,
    pub repairers: RefCell<Vec<PlayerRef>>,
    repair_active: Cell<bool>,
}

register_traits!(RepairableBuilding: ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(RepairableBuildingInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(RepairableBuilding {
        health: init.self_.trait_::<dyn IHealth>(),
        info,
        state,
        repair_tokens: RefCell::new(Vec::new()),
        remaining_ticks: Cell::new(0),
        repairers: RefCell::new(Vec::new()),
        repair_active: Cell::new(false),
    }))
});

impl ConditionalHooks for RepairableBuilding {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}
rustora_engine::conditional_observers_impl!(RepairableBuilding);

impl INotifyCreated for RepairableBuilding {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl ISync for RepairableBuilding {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash()
            ^ self
                .repairers
                .borrow()
                .iter()
                .fold(0, |h, p| h ^ rustora_engine::player::hash_player(Some(p)))
    }
}

impl RepairableBuilding {
    pub fn repair_active(&self) -> bool {
        self.repair_active.get()
    }

    fn update_condition(&self, self_: &ActorRef) {
        let Some(c) = self.info.repair_condition.as_deref().filter(|c| !c.is_empty()) else {
            return;
        };
        let count = self.repairers.borrow().len();
        while count > self.repair_tokens.borrow().len() {
            let t = self_.grant_condition(c);
            self.repair_tokens.borrow_mut().push(t);
        }
        while count < self.repair_tokens.borrow().len() {
            let t = self.repair_tokens.borrow_mut().pop().unwrap();
            self_.revoke_condition(t);
        }
    }

    pub fn repair_building(&self, self_: &ActorRef, player: &PlayerRef) {
        if self.state.is_trait_disabled() || !crate::util::appears_friendly_to(self_, player.player_actor()) {
            return;
        }
        let pos = self.repairers.borrow().iter().position(|p| Rc::ptr_eq(p, player));
        if let Some(i) = pos {
            self.repairers.borrow_mut().remove(i);
            self.update_condition(self_);
            if self.repairers.borrow().is_empty() {
                self.repair_notification(
                    self_,
                    player,
                    self.info.repairing_stopped_notification.as_deref(),
                    self.info.repairing_stopped_text_notification.as_deref(),
                );
            }
            return;
        }
        if self.repairers.borrow().len() >= self.info.repair_bonuses.len() - 1 {
            return;
        }
        self.repairers.borrow_mut().push(player.clone());
        self.repair_notification(
            self_,
            player,
            self.info.repairing_notification.as_deref(),
            self.info.repairing_text_notification.as_deref(),
        );
        self.update_condition(self_);
    }

    fn repair_notification(&self, self_: &ActorRef, player: &PlayerRef, notification: Option<&str>, text: Option<&str>) {
        let world = self_.world();
        if let Some(n) = notification {
            rustora_engine::sound::play_notification(&world, Some(player), "Speech", n, Some(&player.faction));
        }
        world.sound.add_transient_line(Some(&self_.owner()), text);
    }
}

impl ITick for RepairableBuilding {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            if self.repair_active.get() && self.info.cancel_when_disabled {
                self.repairers.borrow_mut().clear();
                self.update_condition(self_);
            }
            return;
        }
        if self.remaining_ticks.get() != 0 {
            self.remaining_ticks.set(self.remaining_ticks.get() - 1);
            return;
        }
        let owner = self_.owner();
        self.repairers.borrow_mut().retain(|p| {
            p.win_state.get() == rustora_engine::player::WinState::Undefined && owner.relationship_with(Some(p)) == PlayerRelationship::ALLY
        });
        self.update_condition(self_);
        if self.repairers.borrow().is_empty() {
            self.repair_active.set(false);
            return;
        }
        let building_value = crate::production::get_sell_value(self_);
        let (max_hp, hp) = (self.health.max_hp(), self.health.hp());
        let hp_to_repair = self.info.repair_step.min(max_hp - hp);
        let cost = 1.max((hp_to_repair as i64 * self.info.repair_percent as i64 * building_value as i64 / (max_hp as i64 * 100)) as i32);
        let repairers = self.repairers.borrow().clone();
        let active_players = repairers
            .iter()
            .filter(|p| p.player_actor().trait_::<PlayerResources>().take_cash(cost, true))
            .count();
        self.repair_active.set(active_players > 0);
        if active_players == 0 {
            self.remaining_ticks.set(1);
            return;
        }
        let amount = -(hp_to_repair * self.info.repair_bonuses[active_players - 1] / 100);
        self.health.inflict_damage(
            self_,
            Some(self_),
            &rustora_engine::damage::Damage::new(amount, self.info.repair_damage_types),
            false,
        );
        if self.health.damage_state() == rustora_engine::damage::DamageState::UNDAMAGED {
            for r in &repairers {
                if !Rc::ptr_eq(r, &owner)
                    && let Some(pe) = r.player_actor().trait_or_default::<PlayerExperience>()
                {
                    pe.give_experience(self.info.player_experience);
                }
            }
            self.repairers.borrow_mut().clear();
            self.repair_active.set(false);
            self.update_condition(self_);
            return;
        }
        self.remaining_ticks.set(self.info.repair_interval);
    }
}

/// `Traits/Player/AllyRepair.cs`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct AllyRepairInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct AllyRepair;

register_traits!(AllyRepair: IResolveOrder);
info_impl!(AllyRepairInfo => |_, _| Some(Rc::new(AllyRepair)));

impl IResolveOrder for AllyRepair {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "RepairBuilding" || order.target.target_type() != TargetType::Actor {
            return;
        }
        let building = order.target.actor().unwrap().clone();
        if !crate::util::appears_friendly_to(&building, self_) {
            return;
        }
        if let Some(rb) = building.trait_or_default::<RepairableBuilding>() {
            rb.repair_building(&building, &self_.owner());
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct DebugPauseStateInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

/// Adds the pause state of the world to the sync hash.
#[derive(rustora_engine::persist::Persist)]
pub struct DebugPauseState {
    world: std::rc::Weak<rustora_engine::world::World>,
}

register_traits!(DebugPauseState: ISync);
info_impl!(DebugPauseStateInfo => |_info, init| Some(Rc::new(DebugPauseState {
    world: Rc::downgrade(&init.world()),
})));

impl ISync for DebugPauseState {
    fn sync_hash(&self) -> i32 {
        self.world.upgrade().is_some_and(|w| w.paused()) as i32
    }
}

/// Registers the save game loaders of this module.
pub(crate) fn register_persist() {
    rustora_engine::persist_values!(dyn rustora_engine::actor_init::ActorInit: LineBuildParentInit, ParentActorInit, LineBuildDirectionInit, FreeActorInit);
}
