// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Activities/Move/AttackMoveActivity.cs` and `Traits/AttackMove.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::activity::{LINE_ORANGE_RED, TargetLineNode, with_line_color};
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::order::Order;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::{activity_common, info_impl, load_info, register_traits};

use crate::attack::AttackSource;
use crate::auto_target::AutoTarget;
use crate::interfaces::IMove;

pub fn register(r: &mut TraitRegistry) {
    r.register("AttackMove", |n| load_info!(AttackMoveInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct AttackMoveInfo {
    pub voice: String,
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub attack_move_condition: Option<String>,
    pub assault_move_condition: Option<String>,
    pub move_into_shroud: bool,
    pub attack_move_cursor: String,
    pub attack_move_blocked_cursor: String,
    pub assault_move_cursor: String,
    pub assault_move_blocked_cursor: String,
}

impl Default for AttackMoveInfo {
    fn default() -> Self {
        AttackMoveInfo {
            voice: "Action".into(),
            instance_name: None,
            attack_move_condition: None,
            assault_move_condition: None,
            move_into_shroud: true,
            attack_move_cursor: "attackmove".into(),
            attack_move_blocked_cursor: "attackmove-blocked".into(),
            assault_move_cursor: "assaultmove".into(),
            assault_move_blocked_cursor: "assaultmove-blocked".into(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct AttackMove {
    pub info: Rc<AttackMoveInfo>,
    mv: Rc<dyn IMove>,
}

register_traits!(AttackMove: IResolveOrder, rustora_engine::sound::IOrderVoice);

impl rustora_engine::sound::IOrderVoice for AttackMove {
    fn voice_phrase_for_order(&self, self_: &ActorRef, order: &Order) -> Option<String> {
        if !crate::util::voice_allows_target(self_, order, self.info.move_into_shroud) {
            return None;
        }
        matches!(order.order_string.as_str(), "AttackMove" | "AssaultMove").then(|| self.info.voice.clone())
    }
}
info_impl!(AttackMoveInfo => |info, init| Some(Rc::new(AttackMove { info, mv: init.self_.trait_::<dyn IMove>() })));

impl IResolveOrder for AttackMove {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "AttackMove" && order.order_string != "AssaultMove" {
            return;
        }
        if !order.target.is_valid_for(self_) {
            return;
        }
        let w = self_.world();
        let cell = w.map().clamp(w.map().cell_containing(order.target.center_position()));
        if !self.info.move_into_shroud && !self_.owner().shroud().is_explored(cell) {
            return;
        }
        let target_location = self.mv.nearest_moveable_cell(cell);
        let assault = order.order_string == "AssaultMove";
        let mv = self.mv.clone();
        self_.queue_activity_queued(
            order.queued,
            AttackMoveActivity::new(
                self_,
                MoveOrder::To {
                    mv,
                    cell: target_location,
                    near_enough: 8,
                    ignore_actor: None,
                    evaluate_nearest_movable_cell: false,
                    line_color: Some(LINE_ORANGE_RED),
                },
                assault,
            ),
        );
        crate::target_lines::show_target_lines(self_);
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct AttackMoveActivity {
    base: ActivityBase,
    get_move: MoveOrder,
    is_assault_move: bool,
    auto_target: Option<Rc<AutoTarget>>,
    attack_move: Option<Rc<AttackMove>>,
    running_move_activity: Cell<bool>,
    token: Cell<i32>,
    target: RefCell<Target>,
}

/// The move of an [`AttackMoveActivity`], created anew whenever the activity moves on.
#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
pub enum MoveOrder {
    /// `IMove.MoveTo`, with an optional target line color.
    To {
        mv: Rc<dyn IMove>,
        cell: rustora_core::world_types::CPos,
        near_enough: i32,
        ignore_actor: Option<ActorRef>,
        evaluate_nearest_movable_cell: bool,
        line_color: Option<rustora_core::fieldloader::Color>,
    },
    /// `IMove.MoveWithinRange` of the cell where `cell_of` is when the move starts, as in `Hunt`.
    WithinRangeOfCell {
        mv: Rc<dyn IMove>,
        cell_of: ActorRef,
        min_range: rustora_core::world_types::WDist,
        max_range: rustora_core::world_types::WDist,
    },
    /// `IMove.MoveFollow`, with an optional target line color.
    Follow {
        mv: Rc<dyn IMove>,
        self_: ActorRef,
        target: Target,
        min_range: rustora_core::world_types::WDist,
        max_range: rustora_core::world_types::WDist,
        line_color: Option<rustora_core::fieldloader::Color>,
    },
    /// A closure, which save games cannot hold.
    Closure(rustora_engine::persist::Unsaved<dyn Fn() -> ActivityRef>),
}

impl MoveOrder {
    fn activity(&self) -> ActivityRef {
        match self {
            MoveOrder::To {
                mv,
                cell,
                near_enough,
                ignore_actor,
                evaluate_nearest_movable_cell,
                line_color,
            } => {
                let a = mv.move_to(*cell, *near_enough, ignore_actor.clone(), *evaluate_nearest_movable_cell);
                match line_color {
                    Some(c) => with_line_color(a, *c),
                    None => a,
                }
            }
            MoveOrder::WithinRangeOfCell {
                mv,
                cell_of,
                min_range,
                max_range,
            } => {
                let target = Target::from_cell(&cell_of.world(), cell_of.location(), rustora_engine::traits::subcell::FULL_CELL);
                mv.move_within_range(&target, *min_range, *max_range, None)
            }
            MoveOrder::Follow {
                mv,
                self_,
                target,
                min_range,
                max_range,
                line_color,
            } => {
                let a = mv.move_follow(self_, target, *min_range, *max_range, None);
                match line_color {
                    Some(c) => with_line_color(a, *c),
                    None => a,
                }
            }
            MoveOrder::Closure(f) => (f.0)(),
        }
    }
}

impl AttackMoveActivity {
    pub fn new(self_: &ActorRef, get_move: MoveOrder, assault_moving: bool) -> ActivityRef {
        let base = ActivityBase::default();
        base.child_has_priority.set(false);
        Rc::new(AttackMoveActivity {
            base,
            get_move,
            is_assault_move: assault_moving,
            auto_target: self_.trait_or_default::<AutoTarget>(),
            attack_move: self_.trait_or_default::<AttackMove>(),
            running_move_activity: Cell::new(false),
            token: Cell::new(INVALID_CONDITION_TOKEN),
            target: RefCell::new(Target::Invalid),
        })
    }
}

impl Activity for AttackMoveActivity {
    activity_common!("AttackMoveActivity");

    fn target_line_nodes(&self, self_: &ActorRef) -> Vec<TargetLineNode> {
        self.get_move.activity().target_line_nodes(self_)
    }

    fn on_first_run(&self, self_: &ActorRef) {
        let (Some(attack_move), true) = (&self.attack_move, self.auto_target.is_some()) else {
            self.base.queue_child(self.get_move.activity());
            return;
        };
        let condition = if self.is_assault_move {
            &attack_move.info.assault_move_condition
        } else {
            &attack_move.info.attack_move_condition
        };
        self.token.set(self_.grant_condition(condition.as_deref().unwrap_or("")));
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let Some(auto_target) = self
            .auto_target
            .as_ref()
            .filter(|_| !self.base.is_canceling() && self.attack_move.is_some())
        else {
            return self.base.tick_child(self_);
        };
        if self.base.child_activity().is_none() || self.running_move_activity.get() {
            let t = auto_target.scan_for_target(self_, auto_target.allow_move(), true, !self.running_move_activity.get());
            *self.target.borrow_mut() = t.clone();
            if t.target_type() != TargetType::Invalid {
                self.running_move_activity.set(false);
                if let Some(c) = self.base.child_activity() {
                    c.cancel(self_, false);
                }
                for ab in auto_target.active_attack_bases() {
                    self.base
                        .queue_child(ab.get_attack_activity(self_, AttackSource::AttackMove, &t, auto_target.allow_move(), false));
                }
            }
            if self.base.child_activity().is_none() {
                self.running_move_activity.set(true);
                self.base.queue_child(self.get_move.activity());
            }
        }
        self.base.tick_child(self_) && self.running_move_activity.get()
    }

    fn on_last_run(&self, self_: &ActorRef) {
        if self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
    }

    fn get_targets(&self, self_: &ActorRef) -> Vec<Target> {
        self.base.child_activity().map_or_else(Vec::new, |c| c.get_targets(self_))
    }
}

/// Registers the save game loaders of this module.
pub(crate) fn register_persist() {
    rustora_engine::persist_activities!(AttackMoveActivity);
}
