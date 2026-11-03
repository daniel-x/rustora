// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Player/MissionObjectives.cs`, `Traits/Player/ConquestVictoryConditions.cs`,
//! `Traits/OwnerLostAction.cs` and `Traits/MustBeDestroyed.cs`. Notifications, the game over delay and the
//! time limit hook are left out.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use rustora_core::field_enum;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::*;
use rustora_engine::damage::DamageTypes;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship, WinState};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::base::MapOptions;

pub fn register(r: &mut TraitRegistry) {
    r.register("MissionObjectives", |n| load_info!(MissionObjectivesInfo, n));
    r.register("ConquestVictoryConditions", |n| load_info!(ConquestVictoryConditionsInfo, n));
    r.register("OwnerLostAction", |n| load_info!(OwnerLostActionInfo, n));
    r.register("MustBeDestroyed", |n| load_info!(MustBeDestroyedInfo, n));
}

pub trait INotifyWinStateChanged {
    fn on_player_won(&self, player: &PlayerRef);
    fn on_player_lost(&self, player: &PlayerRef);
}

pub trait INotifyOwnerLost {
    fn on_owner_lost(&self, self_: &ActorRef);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectiveState {
    Incomplete,
    Completed,
    Failed,
}

pub struct MissionObjective {
    pub required: bool,
    pub description: String,
    pub ty: String,
    pub state: ObjectiveState,
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MissionObjectivesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub cooperative: bool,
    pub early_game_over: bool,
    pub game_over_delay: i32,
}

impl Default for MissionObjectivesInfo {
    fn default() -> Self {
        MissionObjectivesInfo {
            instance_name: None,
            cooperative: false,
            early_game_over: false,
            game_over_delay: 1500,
        }
    }
}

pub struct MissionObjectives {
    pub info: Rc<MissionObjectivesInfo>,
    player: PlayerRef,
    objectives: RefCell<Vec<MissionObjective>>,
    allies: OnceCell<Vec<PlayerRef>>,
    enemies: OnceCell<Vec<PlayerRef>>,
    win_state_cooperative: Cell<WinState>,
}

register_traits!(MissionObjectives: INotifyWinStateChanged, ISync, IResolveOrder, IWorldLoaded);
info_impl!(MissionObjectivesInfo => |info, init| Some(Rc::new(MissionObjectives {
    info,
    player: init.self_.owner(),
    objectives: RefCell::new(Vec::new()),
    allies: OnceCell::new(),
    enemies: OnceCell::new(),
    win_state_cooperative: Cell::new(WinState::Undefined),
})));

fn objectives_of(p: &PlayerRef) -> Rc<MissionObjectives> {
    p.player_actor().trait_::<MissionObjectives>()
}

fn set_win_state(p: &PlayerRef, s: WinState) {
    p.win_state.set(s);
}

impl ISync for MissionObjectives {
    fn sync_hash(&self) -> i32 {
        self.objectives.borrow().iter().fold(0, |h, o| h ^ o.state as i32)
    }
}

impl IWorldLoaded for MissionObjectives {
    fn world_loaded(&self, world: &Rc<World>) {
        let players = world.players();
        let p = &self.player;
        let _ = self.allies.set(
            players
                .iter()
                .filter(|q| !q.non_combatant && p.is_allied_with(Some(q)))
                .cloned()
                .collect(),
        );
        let _ = self.enemies.set(
            players
                .iter()
                .filter(|q| !q.non_combatant && p.relationship_with(Some(q)) == PlayerRelationship::ENEMY)
                .cloned()
                .collect(),
        );
    }
}

impl MissionObjectives {
    pub fn add(&self, player: &PlayerRef, description: &str, ty: &str, required: bool) -> usize {
        let mut o = self.objectives.borrow_mut();
        o.push(MissionObjective {
            required,
            description: description.into(),
            ty: ty.into(),
            state: ObjectiveState::Incomplete,
        });
        player.has_objectives.set(true);
        o.len() - 1
    }

    fn win_state_changed_listeners(player: &PlayerRef) -> Vec<Rc<dyn INotifyWinStateChanged>> {
        player.player_actor().traits_implementing::<dyn INotifyWinStateChanged>()
    }

    pub fn mark_completed(&self, player: &PlayerRef, id: usize) {
        {
            let mut o = self.objectives.borrow_mut();
            if id >= o.len() || o[id].state != ObjectiveState::Incomplete {
                return;
            }
            o[id].state = ObjectiveState::Completed;
        }
        let all_done = {
            let o = self.objectives.borrow();
            o[id].required && o.iter().filter(|x| x.required).all(|x| x.state == ObjectiveState::Completed)
        };
        if all_done {
            for n in Self::win_state_changed_listeners(player) {
                n.on_player_won(player);
            }
            self.check_if_game_is_over(player);
        }
    }

    /// `CheckIfGameIsOver`: C# ends the game `GameOverDelay` milliseconds later from the game loop, outside the
    /// simulation, so this only files the request with the world for the host loop.
    fn check_if_game_is_over(&self, player: &PlayerRef) {
        let w = player.player_actor().world();
        if w.players()
            .iter()
            .all(|p| p.non_combatant || p.win_state.get() != WinState::Undefined || !p.has_objectives.get())
        {
            w.request_end_game(self.info.game_over_delay);
        }
    }

    pub fn mark_failed(&self, player: &PlayerRef, id: usize) {
        let required = {
            let mut o = self.objectives.borrow_mut();
            if id >= o.len() || o[id].state == ObjectiveState::Failed {
                return;
            }
            o[id].state = ObjectiveState::Failed;
            o[id].required
        };
        if required {
            for n in Self::win_state_changed_listeners(player) {
                n.on_player_lost(player);
            }
            self.check_if_game_is_over(player);
        }
    }

    pub fn force_defeat(&self, player: &PlayerRef) {
        let count = self.objectives.borrow().len();
        for id in 0..count {
            if self.objectives.borrow()[id].state == ObjectiveState::Incomplete {
                self.mark_failed(player, id);
            }
        }
    }

    fn allies(&self) -> &[PlayerRef] {
        self.allies.get().map_or(&[], |v| v.as_slice())
    }

    fn enemies(&self) -> &[PlayerRef] {
        self.enemies.get().map_or(&[], |v| v.as_slice())
    }
}

impl INotifyWinStateChanged for MissionObjectives {
    fn on_player_won(&self, player: &PlayerRef) {
        if self.info.cooperative {
            self.win_state_cooperative.set(WinState::Won);
            if self
                .allies()
                .iter()
                .all(|p| objectives_of(p).win_state_cooperative.get() == WinState::Won)
            {
                for p in self.allies() {
                    set_win_state(p, WinState::Won);
                }
                if self.info.early_game_over {
                    for p in self.enemies() {
                        objectives_of(p).force_defeat(p);
                    }
                }
            }
        } else {
            set_win_state(player, WinState::Won);
            if self.info.early_game_over {
                for p in self.enemies() {
                    objectives_of(p).force_defeat(p);
                }
            }
        }
    }

    fn on_player_lost(&self, player: &PlayerRef) {
        if self.info.cooperative {
            self.win_state_cooperative.set(WinState::Lost);
            if self
                .allies()
                .iter()
                .any(|p| objectives_of(p).win_state_cooperative.get() == WinState::Lost)
            {
                for p in self.allies() {
                    set_win_state(p, WinState::Lost);
                }
                if self.info.early_game_over {
                    for p in self.enemies() {
                        set_win_state(p, WinState::Won);
                    }
                }
            }
        } else {
            set_win_state(player, WinState::Lost);
            if self.info.early_game_over {
                for p in self.enemies() {
                    set_win_state(p, WinState::Won);
                }
            }
        }
    }
}

impl IResolveOrder for MissionObjectives {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "Surrender" {
            self.force_defeat(&self_.owner());
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ConquestVictoryConditionsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub objective: String,
}

impl Default for ConquestVictoryConditionsInfo {
    fn default() -> Self {
        ConquestVictoryConditionsInfo {
            instance_name: None,
            objective: "Destroy all opposition!".into(),
        }
    }
}

pub struct ConquestVictoryConditions {
    info: Rc<ConquestVictoryConditionsInfo>,
    mo: Rc<MissionObjectives>,
    short_game: bool,
    other_players: OnceCell<Vec<PlayerRef>>,
    objective_id: Cell<Option<usize>>,
}

register_traits!(ConquestVictoryConditions: ITick, INotifyWinStateChanged);
info_impl!(ConquestVictoryConditionsInfo => |info, init| {
    let short_game = init.world().world_actor().trait_::<MapOptions>().short_game();
    Some(Rc::new(ConquestVictoryConditions {
        info,
        mo: init.self_.trait_::<MissionObjectives>(),
        short_game,
        other_players: OnceCell::new(),
        objective_id: Cell::new(None),
    }))
});

/// `PlayerExtensions.HasNoRequiredUnits`.
pub fn has_no_required_units(world: &World, player: &PlayerRef, short_game: bool) -> bool {
    let actors = world.actors_with_trait::<MustBeDestroyed>();
    if short_game {
        return !actors
            .iter()
            .any(|(a, t)| t.info.required_for_short_game && Rc::ptr_eq(&a.owner(), player));
    }
    !actors.iter().any(|(a, _)| Rc::ptr_eq(&a.owner(), player) && a.is_in_world())
}

impl ITick for ConquestVictoryConditions {
    fn tick(&self, self_: &ActorRef) {
        let owner = self_.owner();
        if owner.win_state.get() != WinState::Undefined || owner.non_combatant {
            return;
        }
        let id = match self.objective_id.get() {
            Some(id) => id,
            None => {
                let id = self.mo.add(&owner, &self.info.objective, "Primary", true);
                self.objective_id.set(Some(id));
                id
            }
        };
        let w = self_.world();
        if has_no_required_units(&w, &owner, self.short_game) {
            self.mo.mark_failed(&owner, id);
        }
        let others = self.other_players.get_or_init(|| {
            w.players()
                .into_iter()
                .filter(|p| !p.non_combatant && !p.is_allied_with(Some(&owner)))
                .collect()
        });
        if others.is_empty() || others.iter().any(|p| p.win_state.get() != WinState::Lost) {
            return;
        }
        self.mo.mark_completed(&owner, id);
    }
}

impl INotifyWinStateChanged for ConquestVictoryConditions {
    fn on_player_lost(&self, player: &PlayerRef) {
        let w = player.player_actor().world();
        for (a, t) in w.actors_with_trait::<dyn INotifyOwnerLost>() {
            if Rc::ptr_eq(&a.owner(), player) {
                t.on_owner_lost(&a);
            }
        }
    }

    fn on_player_won(&self, _player: &PlayerRef) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerLostActionType {
    ChangeOwner,
    Dispose,
    Kill,
}
field_enum!(OwnerLostActionType, [ChangeOwner, Dispose, Kill]);

#[derive(Clone, Debug, FieldLoad)]
pub struct OwnerLostActionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub action: OwnerLostActionType,
    pub owner: String,
    pub death_types: DamageTypes,
}

impl Default for OwnerLostActionInfo {
    fn default() -> Self {
        OwnerLostActionInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            action: OwnerLostActionType::Kill,
            owner: "Neutral".into(),
            death_types: DamageTypes::default(),
        }
    }
}

pub struct OwnerLostAction {
    info: Rc<OwnerLostActionInfo>,
    state: ConditionalState,
}

register_traits!(OwnerLostAction: INotifyOwnerLost, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(OwnerLostActionInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(OwnerLostAction { info, state }))
});

rustora_engine::conditional_simple!(OwnerLostAction);

impl INotifyOwnerLost for OwnerLostAction {
    fn on_owner_lost(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            return;
        }
        match self.info.action {
            OwnerLostActionType::Kill => {
                if !self_.disposed() {
                    if let Some(h) = self_.cache().health.clone() {
                        h.kill(self_, Some(self_), &self.info.death_types);
                    }
                }
            }
            OwnerLostActionType::Dispose => self_.dispose(),
            OwnerLostActionType::ChangeOwner => {
                let p = self_
                    .world()
                    .players()
                    .into_iter()
                    .find(|p| p.internal_name == self.info.owner)
                    .expect("owner");
                self_.change_owner(p);
            }
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct MustBeDestroyedInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub required_for_short_game: bool,
}

pub struct MustBeDestroyed {
    pub info: Rc<MustBeDestroyedInfo>,
}

register_traits!(MustBeDestroyed);
info_impl!(MustBeDestroyedInfo => |info, _| Some(Rc::new(MustBeDestroyed { info })));
