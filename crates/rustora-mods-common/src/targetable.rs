// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `Traits/Targetable.cs` and `Traits/Crushable.cs`.

use std::cell::OnceCell;
use std::rc::Rc;

use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_ext::ITargetable;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::terrain::TargetableType;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::actor_map::WorldExt;
use crate::health::kill_actor;
use crate::interfaces::*;
use crate::mobile::Mobile;

pub fn register(r: &mut TraitRegistry) {
    r.register("Targetable", |n| load_info!(TargetableInfo, n));
    r.register("Crushable", |n| load_info!(CrushableInfo, n));
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct TargetableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub target_types: BitSet<TargetableType>,
    pub requires_force_fire: bool,
}

pub struct Targetable {
    pub info: Rc<TargetableInfo>,
    state: ConditionalState,
    cloaks: OnceCell<Vec<Rc<crate::cloak::Cloak>>>,
}

register_traits!(Targetable: ITargetable, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(TargetableInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(Targetable { info, state, cloaks: OnceCell::new() }))
});

impl ConditionalHooks for Targetable {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl ISync for Targetable {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash()
    }
}

impl IObservesVariables for Targetable {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        conditional_observers(&self)
    }
}

impl IDisabledTrait for Targetable {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl INotifyCreated for Targetable {
    fn created(&self, self_: &ActorRef) {
        let _ = self.cloaks.set(self_.traits_implementing::<crate::cloak::Cloak>());
        conditional_created(self, self_);
    }
}

impl ITargetable for Targetable {
    fn target_types(&self) -> BitSet<TargetableType> {
        self.info.target_types
    }

    fn targetable_by(&self, self_: &ActorRef, viewer: &ActorRef) -> bool {
        if self.state.is_trait_disabled() {
            return false;
        }
        let cloaks = self.cloaks.get().expect("created");
        if cloaks.is_empty() || (!viewer.is_dead() && viewer.info.has_trait_info_named("IgnoresCloak")) {
            return true;
        }
        cloaks.iter().all(|c| c.is_trait_disabled() || c.is_visible_to(self_, &viewer.owner()))
    }

    fn requires_force_fire(&self) -> bool {
        self.info.requires_force_fire
    }

    fn is_trait_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CrushableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub crush_classes: BitSet<CrushClass>,
    pub warn_probability: i32,
    pub crushed_by_friendlies: bool,
}

impl Default for CrushableInfo {
    fn default() -> Self {
        CrushableInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            crush_classes: BitSet::new(&["infantry"]),
            warn_probability: 75,
            crushed_by_friendlies: false,
        }
    }
}

pub struct Crushable {
    info: Rc<CrushableInfo>,
    state: ConditionalState,
    self_: ActorRef,
}

register_traits!(Crushable: ICrushable, INotifyCrushed, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(CrushableInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(Crushable { info, state, self_: init.self_.clone() }))
});

impl ConditionalHooks for Crushable {
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

impl Crushable {
    fn update(&self, self_: &ActorRef) {
        if let Some(o) = self_.cache_opt().and_then(|c| c.occupies_space.clone()) {
            self_.world().actor_map().update_occupied_cells(o.as_ref());
        }
    }

    fn crushable_inner(&self, classes: BitSet<CrushClass>, crush_owner: &rustora_engine::player::PlayerRef) -> bool {
        if self.state.is_trait_disabled() {
            return false;
        }
        if !self.info.crushed_by_friendlies && crush_owner.is_allied_with(Some(&self.self_.owner())) {
            return false;
        }
        self.info.crush_classes.overlaps(&classes)
    }
}

impl ISync for Crushable {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash()
    }
}

impl IObservesVariables for Crushable {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        conditional_observers(&self)
    }
}

impl IDisabledTrait for Crushable {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl INotifyCreated for Crushable {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl ICrushable for Crushable {
    fn crushable_by_actor(&self, _self_: &ActorRef, crusher: &ActorRef, classes: BitSet<CrushClass>) -> bool {
        self.crushable_inner(classes, &crusher.owner())
    }

    fn crushable_by(&self, self_: &ActorRef, classes: BitSet<CrushClass>) -> u64 {
        if self.state.is_trait_disabled() || !self.info.crush_classes.overlaps(&classes) {
            return 0;
        }
        let w = self_.world();
        let all = w.all_players_mask.get();
        if self.info.crushed_by_friendlies { all } else { all & !self_.owner().allied_players_mask.get() }
    }
}

impl INotifyCrushed for Crushable {
    fn warn_crush(&self, self_: &ActorRef, crusher: &ActorRef, classes: BitSet<CrushClass>) {
        if !self.crushable_inner(classes, &crusher.owner()) {
            return;
        }
        if self_.trait_or_default::<Mobile>().is_some() && self_.world().shared_random.borrow_mut().next_max(100) <= self.info.warn_probability {
            self_.queue_activity_queued(false, crate::activities::Nudge::new(crusher));
        }
    }

    fn on_crush(&self, self_: &ActorRef, crusher: &ActorRef, classes: BitSet<CrushClass>) {
        if !self.crushable_inner(classes, &crusher.owner()) {
            return;
        }
        let w = self_.world();
        let types = crusher
            .trait_or_default::<Mobile>()
            .map(|m| m.info.locomotor_info(&w).crush_damage_types)
            .unwrap_or_default();
        kill_actor(self_, Some(crusher), &types);
    }
}
