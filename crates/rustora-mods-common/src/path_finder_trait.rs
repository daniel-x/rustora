// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/World/PathFinder.cs`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::world_types::CPos;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::hpf::HierarchicalPathFinder;
use crate::locomotor::{BlockedByActor, Locomotor, MOVEMENT_COST_FOR_UNREACHABLE_CELL};
use crate::mobile::mobile_of;
use crate::pathfinder::*;

pub fn register(r: &mut TraitRegistry) {
    r.register("PathFinder", |n| load_info!(PathFinderInfo, n));
}

const DEFAULT_HEURISTIC_WEIGHT_PERCENTAGE: i32 = 125;

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PathFinderInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(Default)]
pub struct PathFinder {
    by_none: RefCell<HashMap<String, Rc<HierarchicalPathFinder>>>,
    by_immovable: RefCell<HashMap<String, Rc<HierarchicalPathFinder>>>,
}

register_traits!(PathFinder: IWorldLoaded);
info_impl!(PathFinderInfo => |_, _| Some(Rc::new(PathFinder::default())));

impl IWorldLoaded for PathFinder {
    fn world_loaded(&self, w: &Rc<World>) {
        for l in w.world_actor().traits_implementing::<Locomotor>() {
            self.by_none
                .borrow_mut()
                .insert(l.info.name.clone(), HierarchicalPathFinder::new(w, l.clone(), BlockedByActor::None));
        }
        for l in w.world_actor().traits_implementing::<Locomotor>() {
            self.by_immovable.borrow_mut().insert(
                l.info.name.clone(),
                HierarchicalPathFinder::new(w, l.clone(), BlockedByActor::Immovable),
            );
        }
    }
}

fn actor_locomotor(a: &ActorRef) -> Rc<Locomotor> {
    mobile_of(a).expect("path searches need Mobile").locomotor()
}

impl PathFinder {
    pub fn find_path_to_target_cell(
        &self,
        self_: &ActorRef,
        sources: &[CPos],
        target: CPos,
        check: BlockedByActor,
        custom_cost: Option<CustomCost>,
        ignore_actor: Option<&ActorRef>,
        lane_bias: bool,
    ) -> Vec<CPos> {
        self.find_path_to_target(self_, sources, target, check, custom_cost, ignore_actor, false, lane_bias)
    }

    pub fn find_path_to_target_cells(
        &self,
        self_: &ActorRef,
        source: CPos,
        targets: &[CPos],
        check: BlockedByActor,
        custom_cost: Option<CustomCost>,
        ignore_actor: Option<&ActorRef>,
        lane_bias: bool,
    ) -> Vec<CPos> {
        if targets.is_empty() {
            return Vec::new();
        }
        let world = self_.world();
        let locomotor = actor_locomotor(self_);
        let accessible: Vec<CPos> = targets
            .iter()
            .copied()
            .filter(|t| {
                cell_allows_movement(&world, *t, custom_cost.as_ref())
                    && locomotor.movement_cost_to_enter_cell(Some(self_), *t, check, ignore_actor, true, FULL_CELL)
                        != MOVEMENT_COST_FOR_UNREACHABLE_CELL
            })
            .collect();
        if accessible.is_empty() {
            return Vec::new();
        }
        let source_accessible = cell_allows_movement(&world, source, custom_cost.as_ref())
            && locomotor.movement_cost_to_enter_cell(Some(self_), source, check, ignore_actor, true, FULL_CELL)
                != MOVEMENT_COST_FOR_UNREACHABLE_CELL;
        let mut path = if source_accessible {
            self.find_path_to_target(self_, targets, source, check, custom_cost, ignore_actor, true, lane_bias)
        } else {
            let mut search = PathSearch::to_target_cell(
                &world,
                &locomotor,
                Some(self_),
                &accessible,
                source,
                check,
                DEFAULT_HEURISTIC_WEIGHT_PERCENTAGE,
                custom_cost,
                ignore_actor,
                lane_bias,
                true,
                None,
                None,
            );
            search.find_path()
        };
        path.reverse();
        path
    }

    #[allow(clippy::too_many_arguments)]
    fn find_path_to_target(
        &self,
        self_: &ActorRef,
        sources: &[CPos],
        target: CPos,
        check: BlockedByActor,
        custom_cost: Option<CustomCost>,
        ignore_actor: Option<&ActorRef>,
        in_reverse: bool,
        lane_bias: bool,
    ) -> Vec<CPos> {
        if sources.is_empty() {
            return Vec::new();
        }
        let world = self_.world();
        let locomotor = actor_locomotor(self_);
        if !cell_allows_movement(&world, target, custom_cost.as_ref())
            || locomotor.movement_cost_to_enter_cell(Some(self_), target, check, ignore_actor, in_reverse, FULL_CELL)
                == MOVEMENT_COST_FOR_UNREACHABLE_CELL
        {
            return Vec::new();
        }
        let hpf = self.hierarchical(&locomotor, check, ignore_actor);
        if sources.len() == 1 {
            let source = sources[0];
            if (source - target).length_squared() < 3 {
                if !cell_allows_movement(&world, source, custom_cost.as_ref()) {
                    return Vec::new();
                }
                return vec![target, source];
            }
            return hpf.find_path(
                self_,
                source,
                target,
                check,
                DEFAULT_HEURISTIC_WEIGHT_PERCENTAGE,
                custom_cost,
                ignore_actor,
                in_reverse,
                lane_bias,
            );
        }
        hpf.find_path_multi(
            self_,
            sources,
            target,
            check,
            DEFAULT_HEURISTIC_WEIGHT_PERCENTAGE,
            custom_cost,
            ignore_actor,
            in_reverse,
            lane_bias,
        )
    }

    fn hierarchical(&self, locomotor: &Locomotor, check: BlockedByActor, ignore_actor: Option<&ActorRef>) -> Rc<HierarchicalPathFinder> {
        let m = if check == BlockedByActor::None || ignore_actor.is_some() {
            &self.by_none
        } else {
            &self.by_immovable
        };
        m.borrow()[&locomotor.info.name].clone()
    }

    pub fn find_path_to_target_cell_by_predicate(
        &self,
        self_: &ActorRef,
        sources: &[CPos],
        target_predicate: Box<dyn Fn(CPos) -> bool>,
        check: BlockedByActor,
        custom_cost: Option<CustomCost>,
        ignore_actor: Option<&ActorRef>,
        lane_bias: bool,
    ) -> Vec<CPos> {
        let world = self_.world();
        let mut search = PathSearch::to_target_cell_by_predicate(
            &world,
            &actor_locomotor(self_),
            Some(self_),
            sources,
            target_predicate,
            check,
            custom_cost,
            ignore_actor,
            lane_bias,
        );
        search.find_path()
    }

    pub fn path_exists_for_locomotor(&self, locomotor: &Locomotor, source: CPos, target: CPos) -> bool {
        self.by_none.borrow()[&locomotor.info.name].path_exists(source, target)
    }

    pub fn path_might_exist_for_locomotor_blocked_by_immovable(&self, locomotor: &Locomotor, source: CPos, target: CPos) -> bool {
        self.by_immovable.borrow()[&locomotor.info.name].path_exists(source, target)
    }
}
