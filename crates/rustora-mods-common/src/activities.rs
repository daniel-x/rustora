// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of the basic and movement activities of `Mods.Common/Activities`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use rustora_core::world_types::{CPos, CVec, Int2, WAngle, WDist, WPos, WRot, WVec};
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::activity::{LINE_GREEN, TargetLineNode, chain_target_line_nodes};
use rustora_engine::activity_common;
use rustora_engine::actor::ActorRef;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::traits::*;
use rustora_engine::world::World;

use crate::actor_map::WorldExt;
use crate::interfaces::*;
use crate::locomotor::BlockedByActor;
use crate::mobile::{Mobile, MoveResult, mobile_of};
use crate::util;

pub struct Wait {
    base: ActivityBase,
    remaining_ticks: Cell<i32>,
}

impl Wait {
    pub fn new(period: i32) -> ActivityRef {
        Rc::new(Wait {
            base: ActivityBase::default(),
            remaining_ticks: Cell::new(period),
        })
    }

    pub fn new_interruptible(period: i32, interruptible: bool) -> ActivityRef {
        let base = ActivityBase::default();
        base.is_interruptible.set(interruptible);
        Rc::new(Wait {
            base,
            remaining_ticks: Cell::new(period),
        })
    }
}

impl Activity for Wait {
    activity_common!("Wait");

    fn tick(&self, _self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let r = self.remaining_ticks.get();
        self.remaining_ticks.set(r.wrapping_sub(1));
        r == 0
    }
}

/// `Activities/CallFunc.cs`.
pub struct CallFunc {
    base: ActivityBase,
    f: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl CallFunc {
    pub fn new(f: Box<dyn FnOnce()>) -> ActivityRef {
        Rc::new(CallFunc {
            base: ActivityBase::default(),
            f: RefCell::new(Some(f)),
        })
    }
}

impl Activity for CallFunc {
    activity_common!("CallFunc");

    fn tick(&self, _self_: &ActorRef) -> bool {
        if let Some(f) = self.f.borrow_mut().take() {
            f();
        }
        true
    }
}

/// `Activities/SimpleTeleport.cs`.
pub struct SimpleTeleport {
    base: ActivityBase,
    destination: CPos,
}

impl SimpleTeleport {
    pub fn new(destination: CPos) -> ActivityRef {
        Rc::new(SimpleTeleport {
            base: ActivityBase::default(),
            destination,
        })
    }
}

impl Activity for SimpleTeleport {
    activity_common!("SimpleTeleport");

    fn tick(&self, self_: &ActorRef) -> bool {
        self_
            .trait_::<dyn IPositionable>()
            .set_position_cell(self_, self.destination, rustora_engine::traits::subcell::ANY);
        self_.generation.set(self_.generation.get().wrapping_add(1));
        true
    }
}

pub struct WaitFor {
    base: ActivityBase,
    f: Option<Box<dyn Fn() -> bool>>,
}

impl WaitFor {
    pub fn new(f: Box<dyn Fn() -> bool>, interruptible: bool) -> ActivityRef {
        let base = ActivityBase::default();
        base.is_interruptible.set(interruptible);
        Rc::new(WaitFor { base, f: Some(f) })
    }
}

impl Activity for WaitFor {
    activity_common!("WaitFor");

    fn tick(&self, _self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        self.f.as_ref().is_none_or(|f| f())
    }
}

pub struct Turn {
    base: ActivityBase,
    mobile: Option<Rc<Mobile>>,
    facing: Rc<dyn IFacing>,
    desired_facing: WAngle,
}

impl Turn {
    pub fn new(self_: &ActorRef, desired_facing: WAngle) -> ActivityRef {
        Rc::new(Turn {
            base: ActivityBase::default(),
            mobile: self_.trait_or_default::<Mobile>(),
            facing: self_.trait_::<dyn IFacing>(),
            desired_facing,
        })
    }
}

impl Activity for Turn {
    activity_common!("Turn");

    fn tick(&self, _self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        if self.mobile.as_ref().is_some_and(|m| m.is_trait_disabled() || m.is_trait_paused()) {
            return false;
        }
        if self.desired_facing == self.facing.facing() {
            return true;
        }
        self.facing.set_facing(util::tick_facing(
            self.facing.facing(),
            self.desired_facing,
            self.facing.turn_speed(),
        ));
        false
    }
}

pub struct Drag {
    base: ActivityBase,
    positionable: Rc<dyn IPositionable>,
    disableable: Option<Rc<dyn IDisabledTrait>>,
    start: WPos,
    end: WPos,
    length: i32,
    ticks: Cell<i32>,
    desired_facing: Option<WAngle>,
}

impl Drag {
    pub fn new(self_: &ActorRef, start: WPos, end: WPos, length: i32, facing: Option<WAngle>) -> ActivityRef {
        let base = ActivityBase::default();
        base.is_interruptible.set(false);
        Rc::new(Drag {
            base,
            positionable: self_.trait_::<dyn IPositionable>(),
            disableable: self_.trait_or_default::<Mobile>().map(|m| m as Rc<dyn IDisabledTrait>),
            start,
            end,
            length,
            ticks: Cell::new(0),
            desired_facing: facing,
        })
    }
}

impl Activity for Drag {
    activity_common!("Drag");

    fn target_line_nodes(&self, self_: &ActorRef) -> Vec<TargetLineNode> {
        let _ = self_;
        vec![TargetLineNode {
            target: Target::from_pos(self.end),
            color: LINE_GREEN,
        }]
    }

    fn on_first_run(&self, self_: &ActorRef) {
        if let Some(f) = self.desired_facing {
            self.base.queue_child(Turn::new(self_, f));
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.disableable.as_ref().is_some_and(|d| d.is_trait_disabled()) {
            return false;
        }
        let pos = if self.length > 1 {
            WPos::lerp(self.start, self.end, self.ticks.get(), self.length - 1)
        } else {
            self.end
        };
        self.positionable.set_center_position(self_, pos);
        self.ticks.set(self.ticks.get() + 1);
        self.ticks.get() >= self.length
    }

    fn get_targets(&self, _self_: &ActorRef) -> Vec<Target> {
        vec![Target::from_pos(self.end)]
    }
}

pub type GetPathFn = Rc<dyn Fn(BlockedByActor) -> (bool, Vec<CPos>)>;

enum PathSource {
    /// Scriptable move order, ignores lane bias.
    Script(CPos),
    Destination,
    Func(GetPathFn),
}

const PATH_SEARCH_ORDER: [BlockedByActor; 4] = [
    BlockedByActor::All,
    BlockedByActor::Stationary,
    BlockedByActor::Immovable,
    BlockedByActor::None,
];

pub struct Move {
    base: ActivityBase,
    me: Weak<Move>,
    pub actor_facing_modifier: Cell<WAngle>,
    mobile: Rc<Mobile>,
    self_: ActorRef,
    near_enough: WDist,
    source: PathSource,
    ignore_actor: Option<ActorRef>,
    carryover_progress: Cell<i32>,
    last_move_part_completed_tick: Cell<i32>,
    already_at_destination: Cell<bool>,
    path: RefCell<Option<Vec<CPos>>>,
    destination: Cell<Option<CPos>>,
    start_ticks: Cell<i32>,
    had_no_path: Cell<bool>,
    has_waited: Cell<bool>,
    wait_ticks_remaining: Cell<i32>,
    evaluate_nearest_movable_cell: bool,
}

impl Move {
    fn make(
        self_: &ActorRef,
        source: PathSource,
        destination: Option<CPos>,
        near_enough: WDist,
        ignore_actor: Option<ActorRef>,
        evaluate: bool,
    ) -> Rc<Move> {
        Rc::new_cyclic(|me| Move {
            base: ActivityBase::default(),
            me: me.clone(),
            actor_facing_modifier: Cell::new(WAngle::ZERO),
            mobile: mobile_of(self_).expect("Move needs Mobile").me(),
            self_: self_.clone(),
            near_enough,
            source,
            ignore_actor,
            carryover_progress: Cell::new(0),
            last_move_part_completed_tick: Cell::new(0),
            already_at_destination: Cell::new(false),
            path: RefCell::new(None),
            destination: Cell::new(destination),
            start_ticks: Cell::new(0),
            had_no_path: Cell::new(false),
            has_waited: Cell::new(false),
            wait_ticks_remaining: Cell::new(0),
            evaluate_nearest_movable_cell: evaluate,
        })
    }

    pub fn new_script(self_: &ActorRef, destination: CPos) -> ActivityRef {
        Self::make(self_, PathSource::Script(destination), Some(destination), WDist::ZERO, None, false)
    }

    pub fn new_near(
        self_: &ActorRef,
        destination: CPos,
        near_enough: WDist,
        ignore_actor: Option<ActorRef>,
        evaluate: bool,
    ) -> ActivityRef {
        Self::make(
            self_,
            PathSource::Destination,
            Some(destination),
            near_enough,
            ignore_actor,
            evaluate,
        )
    }

    pub fn new_func(self_: &ActorRef, get_path: GetPathFn) -> ActivityRef {
        Self::make(self_, PathSource::Func(get_path), None, WDist::ZERO, None, false)
    }

    fn get_path(&self, check: BlockedByActor) -> (bool, Vec<CPos>) {
        let m = &self.mobile;
        match &self.source {
            PathSource::Script(destination) => {
                if m.to_cell() == *destination {
                    return (true, Vec::new());
                }
                (
                    false,
                    m.path_finder()
                        .find_path_to_target_cell(&self.self_, &[m.to_cell()], *destination, check, None, None, false),
                )
            }
            PathSource::Destination => {
                let Some(d) = self.destination.get() else {
                    return (false, Vec::new());
                };
                if m.to_cell() == d {
                    return (true, Vec::new());
                }
                (
                    false,
                    m.path_finder()
                        .find_path_to_target_cell(&self.self_, &[m.to_cell()], d, check, None, self.ignore_actor.as_ref(), true),
                )
            }
            PathSource::Func(f) => f(check),
        }
    }

    fn eval_path(&self, check: BlockedByActor) -> (bool, Vec<CPos>) {
        let (at, path) = self.get_path(check);
        let to = self.mobile.to_cell();
        (at, path.into_iter().take_while(|a| *a != to).collect())
    }

    fn set_eval(&self, check: BlockedByActor) {
        let (a, p) = self.eval_path(check);
        self.already_at_destination.set(a);
        *self.path.borrow_mut() = Some(p);
    }

    fn path_len(&self) -> usize {
        self.path.borrow().as_ref().map_or(0, Vec::len)
    }

    fn path_clear(&self) {
        if let Some(p) = self.path.borrow_mut().as_mut() {
            p.clear();
        }
    }

    pub fn path_add(&self, c: CPos) {
        self.path.borrow_mut().as_mut().expect("move path").push(c);
    }

    /// Returns the next cell with subcell and `ShouldTryAgain`.
    fn pop_path(&self, self_: &ActorRef) -> (Option<(CPos, u8)>, bool) {
        let m = &self.mobile;
        if self.path_len() == 0 {
            return (None, false);
        }
        let next_cell = *self.path.borrow().as_ref().unwrap().last().unwrap();
        if !util::are_adjacent_cells(m.to_cell(), next_cell) {
            self.set_eval(BlockedByActor::Immovable);
            return (None, false);
        }
        let ignore = self.ignore_actor.as_ref();
        let contains_temporary_blocker = crate::gate::contains_temporary_blocker(&self_.world(), next_cell, Some(self_));
        if contains_temporary_blocker || !m.can_enter_cell(next_cell, ignore, BlockedByActor::All) {
            let cell_range = self.near_enough.length / 1024;
            let dest = self.destination.get().expect("destination");
            if !contains_temporary_blocker
                && (m.to_cell() - dest).length_squared() <= cell_range * cell_range
                && m.can_stay_in_cell(m.to_cell())
            {
                if self.path_len() < 2 {
                    self.path_clear();
                    return (None, false);
                }
                let blocker_dist_sq = (next_cell - dest).length_squared();
                let nudge_or_repath = CVec::DIRECTIONS.iter().map(|d| next_cell + *d).any(|c| {
                    c != self_.location()
                        && (c - dest).length_squared() <= blocker_dist_sq
                        && m.can_enter_cell(c, ignore, BlockedByActor::All)
                });
                if !nudge_or_repath {
                    self.path_clear();
                    return (None, false);
                }
            }
            if !m.can_enter_cell(next_cell, ignore, BlockedByActor::Immovable) {
                self.set_eval(BlockedByActor::Immovable);
                return (None, false);
            }
            let w = self_.world();
            notify_blocker(self_, next_cell);
            if !self.has_waited.get() {
                self.wait_ticks_remaining.set(m.locomotor_info().wait_average);
                self.has_waited.set(true);
                return (None, true);
            }
            self.wait_ticks_remaining.set(self.wait_ticks_remaining.get() - 1);
            if self.wait_ticks_remaining.get() >= 0 {
                return (None, true);
            }
            self.has_waited.set(false);
            if cell_is_evacuating(&w, next_cell) {
                return (None, true);
            }
            m.remove_influence();
            let (_, new_path) = self.eval_path(BlockedByActor::All);
            m.add_influence();
            if !new_path.is_empty() {
                let mut p = new_path;
                let new_cell = p.pop().unwrap();
                *self.path.borrow_mut() = Some(p);
                return (
                    Some((
                        new_cell,
                        m.get_available_subcell(next_cell, m.from_subcell.get(), ignore, BlockedByActor::All),
                    )),
                    true,
                );
            } else if m.is_blocking()
                && let Some(new_cell) = m.get_adjacent_cell(next_cell, None)
            {
                if (next_cell - new_cell).length_squared() > 2 {
                    self.path_add(m.to_cell());
                }
                return (
                    Some((
                        new_cell,
                        m.get_available_subcell(new_cell, m.from_subcell.get(), ignore, BlockedByActor::All),
                    )),
                    true,
                );
            }
            return (None, false);
        }
        self.has_waited.set(false);
        self.path.borrow_mut().as_mut().unwrap().pop();
        (
            Some((
                next_cell,
                m.get_available_subcell(next_cell, m.from_subcell.get(), ignore, BlockedByActor::All),
            )),
            true,
        )
    }

    /// `Cancel(self, keepQueue, forceClearPath)`.
    pub fn cancel_force(&self, self_: &ActorRef, keep_queue: bool, force_clear_path: bool) {
        if self.path.borrow().is_some() && (force_clear_path || self.mobile.can_stay_in_cell(self.mobile.to_cell())) {
            self.path_clear();
        }
        self.base.cancel(self_, keep_queue);
    }
}

impl Activity for Move {
    activity_common!("Move");

    fn target_line_target(&self, self_: &ActorRef) -> Option<Target> {
        let world = self_.world();
        self.destination.get().map(|d| Target::from_cell(&world, d, 0))
    }

    fn on_first_run(&self, self_: &ActorRef) {
        self.start_ticks.set(self_.world().world_tick());
        self.mobile.move_result.set(MoveResult::InProgress);
        if self.evaluate_nearest_movable_cell
            && let Some(d) = self.destination.get()
        {
            let movable = self.mobile.nearest_moveable_cell_range(d, 1, 10);
            self.destination.set(
                self.mobile
                    .can_enter_cell(movable, None, BlockedByActor::Immovable)
                    .then_some(movable),
            );
        }
        for check in PATH_SEARCH_ORDER {
            self.set_eval(check);
            if self.already_at_destination.get() || self.path_len() > 0 {
                return;
            }
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let m = &self.mobile;
        m.turn_to_move.set(false);
        if self.base.is_canceling() && m.can_stay_in_cell(m.to_cell()) {
            self.path_clear();
            m.move_result.set(MoveResult::CompleteCanceled);
            return true;
        }
        if m.is_trait_disabled() || m.is_trait_paused() {
            return false;
        }
        if self.already_at_destination.get() {
            m.move_result.set(MoveResult::CompleteDestinationReached);
            return true;
        }
        if self.destination.get() == Some(m.to_cell()) {
            m.move_result.set(if self.had_no_path.get() {
                MoveResult::CompleteDestinationBlocked
            } else {
                MoveResult::CompleteDestinationReached
            });
            return true;
        }
        if self.path_len() == 0 {
            self.had_no_path.set(true);
            self.destination.set(Some(m.to_cell()));
            return false;
        }
        self.destination.set(Some(self.path.borrow().as_ref().unwrap()[0]));
        let (next, should_try_again) = self.pop_path(self_);
        let Some((next_cell, next_sub)) = next else {
            if !should_try_again {
                m.move_result.set(MoveResult::CompleteDestinationBlocked);
                return true;
            }
            return false;
        };
        let w = self_.world();
        let map = w.map();
        let mut first_facing = map.facing_between(m.from_cell(), next_cell, m.facing());
        if m.info.can_move_backward
            && (m.info.max_backward_cells < 0 || (self.path_len() as i32) < m.info.max_backward_cells)
            && (m.info.backward_duration < 0 || w.world_tick() - self.start_ticks.get() < m.info.backward_duration)
            && (first_facing.angle - m.facing().angle).abs() > 256
        {
            self.actor_facing_modifier.set(WAngle::new(512));
            first_facing = first_facing + self.actor_facing_modifier.get();
        } else {
            self.actor_facing_modifier.set(WAngle::ZERO);
        }
        if !m.info.turns_while_moving && first_facing != m.facing() {
            self.path_add(next_cell);
            self.base.queue_child(Turn::new(self_, first_facing));
            m.turn_to_move.set(true);
            return false;
        }
        m.set_location(m.from_cell(), m.from_subcell.get(), next_cell, next_sub);
        let from = map.center_of_cell(m.from_cell()) + map.offset_of_subcell(m.from_subcell.get());
        let to = util::between_cells(&w, m.from_cell(), m.to_cell())
            + (map.offset_of_subcell(m.from_subcell.get()) + map.offset_of_subcell(m.to_subcell.get())) / 2;
        self.base.queue_child(MovePart::new(
            self.me.upgrade().expect("move alive"),
            true,
            from,
            to,
            m.facing(),
            first_facing,
            self.carryover_progress.get(),
            false,
        ));
        self.carryover_progress.set(0);
        false
    }

    fn on_last_run(&self, _self_: &ActorRef) {
        *self.path.borrow_mut() = None;
    }

    fn cancel(&self, self_: &ActorRef, keep_queue: bool) {
        self.cancel_force(self_, keep_queue, false);
    }

    fn get_targets(&self, self_: &ActorRef) -> Vec<Target> {
        let w = self_.world();
        if let Some(p) = self.path.borrow().as_ref() {
            return p.iter().rev().map(|c| Target::from_cell(&w, *c, FULL_CELL)).collect();
        }
        if let Some(d) = self.destination.get() {
            return vec![Target::from_cell(&w, d, FULL_CELL)];
        }
        Vec::new()
    }
}

pub fn notify_blocker(self_: &ActorRef, position: CPos) {
    for blocker in self_.world().actor_map().get_actors_at(position) {
        for n in blocker.traits_implementing::<dyn INotifyBlockingMove>() {
            n.on_notify_blocking_move(&blocker, self_);
        }
    }
}

/// `NotifyBlocker(IEnumerable<CPos>)`.
pub fn notify_blocker_cells(self_: &ActorRef, cells: &[CPos]) {
    for c in cells {
        notify_blocker(self_, *c);
    }
}

fn cell_is_evacuating(w: &World, cell: CPos) -> bool {
    for actor in w.actor_map().get_actors_at(cell) {
        match mobile_of(&actor) {
            Some(m) if !m.is_trait_disabled() && m.is_leaving() => {}
            _ => return false,
        }
    }
    true
}

/// `Move.MovePart` with its `MoveFirstHalf` and `MoveSecondHalf` variants.
struct MovePart {
    base: ActivityBase,
    mv: Rc<Move>,
    first_half: bool,
    from: WPos,
    to: WPos,
    from_facing: WAngle,
    to_facing: WAngle,
    enable_arc: bool,
    arc_center: WPos,
    arc_from_length: i32,
    arc_from_angle: WAngle,
    arc_to_length: i32,
    arc_to_angle: WAngle,
    distance: i32,
    turns_while_moving: bool,
    progress: Cell<i32>,
}

impl MovePart {
    #[allow(clippy::too_many_arguments)]
    fn new(
        mv: Rc<Move>,
        first_half: bool,
        from: WPos,
        to: WPos,
        from_facing: WAngle,
        to_facing: WAngle,
        carryover: i32,
        should_arc: bool,
    ) -> ActivityRef {
        let base = ActivityBase::default();
        base.is_interruptible.set(false);
        let turns_while_moving = mv.mobile.info.turns_while_moving;
        let mut part = MovePart {
            base,
            first_half,
            from,
            to,
            from_facing,
            to_facing,
            enable_arc: false,
            arc_center: WPos::ZERO,
            arc_from_length: 0,
            arc_from_angle: WAngle::ZERO,
            arc_to_length: 0,
            arc_to_angle: WAngle::ZERO,
            distance: (to - from).length(),
            turns_while_moving,
            progress: Cell::new(carryover),
            mv,
        };
        if should_arc {
            let u = WVec::new(1024, 0, 0).rotate(&WRot::from_yaw(from_facing));
            let v = WVec::new(1024, 0, 0).rotate(&WRot::from_yaw(to_facing));
            if v.x.wrapping_mul(u.y) != v.y.wrapping_mul(u.x) {
                let w = from - to;
                let s = (v.y.wrapping_mul(w.x).wrapping_sub(v.x.wrapping_mul(w.y))).wrapping_mul(1024)
                    / (v.x.wrapping_mul(u.y).wrapping_sub(v.y.wrapping_mul(u.x)));
                let x = from.x + s.wrapping_mul(u.x) / 1024;
                let y = from.y + s.wrapping_mul(u.y) / 1024;
                part.arc_center = WPos::new(x, y, 0);
                part.arc_from_length = (part.arc_center - from).horizontal_length();
                part.arc_from_angle = (part.arc_center - from).yaw();
                part.arc_to_length = (part.arc_center - to).horizontal_length();
                part.arc_to_angle = (part.arc_center - to).yaw();
                part.enable_arc = true;
            }
        }
        Rc::new(part)
    }

    fn is_turn(&self, m: &Mobile, w: &World, next_cell: CPos) -> bool {
        if m.info.always_turn_in_place || self.turns_while_moving {
            return false;
        }
        if self.mv.actor_facing_modifier.get() != WAngle::ZERO && w.world_tick() - self.mv.start_ticks.get() >= m.info.backward_duration {
            return false;
        }
        let map = w.map();
        let next_facing = map.facing_between(next_cell, m.to_cell(), m.facing());
        let current_facing = map.facing_between(m.to_cell(), m.from_cell(), m.facing());
        let delta = (next_facing - current_facing).angle;
        delta != 0 && !(384..=640).contains(&delta)
    }

    fn on_complete(&self, self_: &ActorRef, m: &Mobile) -> Option<ActivityRef> {
        if !self.first_half {
            m.set_position_cell(self_, m.to_cell(), rustora_engine::traits::subcell::ANY);
            self.mv.carryover_progress.set(self.progress.get() - self.distance);
            return None;
        }
        let w = self_.world();
        let map = w.map();
        let from_off = map.offset_of_subcell(m.from_subcell.get());
        let to_off = map.offset_of_subcell(m.to_subcell.get());
        let (next, _) = self.mv.pop_path(self_);
        if let Some((next_cell, next_sub)) = next {
            if !m.is_trait_paused() && !m.is_trait_disabled() && self.is_turn(m, &w, next_cell) {
                let next_off = map.offset_of_subcell(next_sub);
                let ret = MovePart::new(
                    self.mv.clone(),
                    true,
                    util::between_cells(&w, m.from_cell(), m.to_cell()) + (from_off + to_off) / 2,
                    util::between_cells(&w, m.to_cell(), next_cell) + (to_off + next_off) / 2,
                    m.facing(),
                    map.facing_between(m.to_cell(), next_cell, m.facing()) + self.mv.actor_facing_modifier.get(),
                    self.progress.get() - self.distance,
                    true,
                );
                m.finished_moving(self_);
                m.set_location(m.to_cell(), m.to_subcell.get(), next_cell, next_sub);
                return Some(ret);
            }
            self.mv.path_add(next_cell);
        }
        let to_pos = map.center_of_cell(m.to_cell());
        let ret2 = MovePart::new(
            self.mv.clone(),
            false,
            util::between_cells(&w, m.from_cell(), m.to_cell()) + (from_off + to_off) / 2,
            to_pos + to_off,
            m.facing(),
            if self.turns_while_moving {
                map.facing_between(m.from_cell(), m.to_cell(), m.facing()) + self.mv.actor_facing_modifier.get()
            } else {
                m.facing()
            },
            self.progress.get() - self.distance,
            false,
        );
        m.entering_cell(self_);
        m.set_location(m.to_cell(), m.to_subcell.get(), m.to_cell(), m.to_subcell.get());
        Some(ret2)
    }
}

impl Activity for MovePart {
    activity_common!("MovePart");

    fn tick(&self, self_: &ActorRef) -> bool {
        let m = self.mv.mobile.clone();
        let w = self_.world();
        if self.mv.last_move_part_completed_tick.get() < w.world_tick() {
            self.progress.set(self.progress.get() + m.movement_speed_for_cell(m.to_cell()));
        }
        let progress = self.progress.get();
        if progress >= self.distance {
            let to = self.to - WVec::new(0, 0, w.map().distance_above_terrain(self.to).length);
            m.set_center_position(self_, to);
            m.set_facing(if self.turns_while_moving {
                util::tick_facing(m.facing(), self.to_facing, m.turn_speed())
            } else {
                self.to_facing
            });
            self.mv.last_move_part_completed_tick.set(w.world_tick());
            self.base.queue(self.on_complete(self_, &m));
            return true;
        }
        let pos = if self.enable_arc {
            let angle = WAngle::lerp(self.arc_from_angle, self.arc_to_angle, progress, self.distance);
            let length = Int2::lerp_i32(self.arc_from_length, self.arc_to_length, progress, self.distance);
            let height = Int2::lerp_i32(self.from.z, self.to.z, progress, self.distance);
            self.arc_center + WVec::new(0, length, height).rotate(&WRot::from_yaw(angle))
        } else {
            WPos::lerp(self.from, self.to, progress, self.distance)
        };
        let pos = pos - WVec::new(0, 0, w.map().distance_above_terrain(pos).length);
        m.set_center_position(self_, pos);
        m.set_facing(if self.turns_while_moving {
            util::tick_facing(m.facing(), self.to_facing, m.turn_speed())
        } else {
            WAngle::lerp(self.from_facing, self.to_facing, progress, self.distance)
        });
        false
    }

    fn get_targets(&self, self_: &ActorRef) -> Vec<Target> {
        self.mv.get_targets(self_)
    }
}

/// `MoveCooldownHelper`.
pub struct MoveCooldownHelper {
    pub retry_if_destination_blocked: Cell<bool>,
    pub cooldown: Cell<(i32, i32)>,
    mobile: Option<Rc<Mobile>>,
    was_moving: Cell<bool>,
    has_run_cooldown: Cell<bool>,
    cooldown_ticks: Cell<i32>,
}

impl MoveCooldownHelper {
    pub fn new(mobile: Option<Rc<Mobile>>) -> Self {
        MoveCooldownHelper {
            retry_if_destination_blocked: Cell::new(false),
            cooldown: Cell::new((20, 31)),
            mobile,
            was_moving: Cell::new(false),
            has_run_cooldown: Cell::new(false),
            cooldown_ticks: Cell::new(0),
        }
    }

    pub fn notify_move_queued(&self) {
        self.was_moving.set(true);
    }

    pub fn tick(&self, world: &World, target_is_hidden_actor: bool) -> Option<bool> {
        if !self.was_moving.get() {
            return None;
        }
        if !self.has_run_cooldown.get() {
            if target_is_hidden_actor {
                return Some(true);
            }
            let result = self.mobile.as_ref().map(|m| m.move_result.get());
            if matches!(
                result,
                None | Some(MoveResult::CompleteCanceled) | Some(MoveResult::CompleteDestinationReached)
            ) {
                self.was_moving.set(false);
                return None;
            }
            if !self.retry_if_destination_blocked.get() && result == Some(MoveResult::CompleteDestinationBlocked) {
                return Some(true);
            }
            self.has_run_cooldown.set(true);
            let (lo, hi) = self.cooldown.get();
            self.cooldown_ticks.set(world.shared_random.borrow_mut().next_range(lo, hi));
            Some(false)
        } else {
            if self.cooldown_ticks.get() > 0 {
                self.cooldown_ticks.set(self.cooldown_ticks.get() - 1);
            }
            if self.cooldown_ticks.get() <= 0 {
                self.has_run_cooldown.set(false);
                self.was_moving.set(false);
            }
            Some(false)
        }
    }
}

pub struct Follow {
    base: ActivityBase,
    min_range: WDist,
    max_range: WDist,
    mv: Rc<dyn IMove>,
    helper: MoveCooldownHelper,
    target: RefCell<Target>,
    last_visible_target: RefCell<Target>,
    use_last_visible_target: Cell<bool>,
}

pub fn initial_last_visible(self_: &ActorRef, target: &Target, initial: Option<WPos>) -> Target {
    let tt = target.target_type();
    if (tt == TargetType::Actor && target.actor().unwrap().can_be_viewed_by_player(Some(&self_.owner())))
        || tt == TargetType::FrozenActor
        || tt == TargetType::Terrain
    {
        Target::from_pos(target.center_position())
    } else if let Some(p) = initial {
        Target::from_pos(p)
    } else {
        Target::Invalid
    }
}

impl Follow {
    pub fn new(self_: &ActorRef, target: &Target, min_range: WDist, max_range: WDist, initial: Option<WPos>) -> ActivityRef {
        let helper = MoveCooldownHelper::new(self_.trait_or_default::<Mobile>());
        helper.retry_if_destination_blocked.set(true);
        Rc::new(Follow {
            base: ActivityBase::default(),
            min_range,
            max_range,
            mv: self_.trait_::<dyn IMove>(),
            helper,
            target: RefCell::new(target.clone()),
            last_visible_target: RefCell::new(initial_last_visible(self_, target, initial)),
            use_last_visible_target: Cell::new(false),
        })
    }
}

impl Activity for Follow {
    activity_common!("Follow");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        let t = if self.use_last_visible_target.get() {
            &self.last_visible_target
        } else {
            &self.target
        };
        Some(t.borrow().clone())
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let (t, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = t.clone();
        if !hidden && t.target_type() == TargetType::Actor {
            *self.last_visible_target.borrow_mut() = Target::from_target_positions(&t);
        }
        self.use_last_visible_target.set(hidden || !t.is_valid_for(self_));
        if let Some(r) = self.helper.tick(&self_.world(), hidden) {
            return r;
        }
        let lvt = self.last_visible_target.borrow().clone();
        if self.use_last_visible_target.get() && !lvt.is_valid_for(self_) {
            return true;
        }
        let pos = self_.center_position();
        let check = if self.use_last_visible_target.get() { lvt } else { t.clone() };
        if check.is_in_range(pos, self.max_range) && !check.is_in_range(pos, self.min_range) {
            return self.use_last_visible_target.get();
        }
        self.helper.notify_move_queued();
        self.base.queue_child(
            self.mv
                .move_within_range(&t, self.min_range, self.max_range, Some(check.center_position())),
        );
        false
    }
}

pub struct LocalMoveIntoTarget {
    base: ActivityBase,
    mobile: Rc<Mobile>,
    target: Target,
    threshold: WDist,
    target_start_pos: Cell<WPos>,
}

impl LocalMoveIntoTarget {
    pub fn new(self_: &ActorRef, target: &Target, threshold: WDist) -> ActivityRef {
        Rc::new(LocalMoveIntoTarget {
            base: ActivityBase::default(),
            mobile: self_.trait_::<Mobile>(),
            target: target.clone(),
            threshold,
            target_start_pos: Cell::new(WPos::ZERO),
        })
    }
}

impl Activity for LocalMoveIntoTarget {
    activity_common!("LocalMoveIntoTarget");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        Some(self.target.clone())
    }

    fn on_first_run(&self, self_: &ActorRef) {
        self.target_start_pos
            .set(util::closest_position(&self.target.positions(), self_.center_position()).unwrap_or(WPos::ZERO));
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() || self.target.target_type() == TargetType::Invalid {
            return true;
        }
        let m = &self.mobile;
        if m.is_trait_disabled() || m.is_trait_paused() {
            return false;
        }
        let current = self_.center_position();
        let target_pos = util::closest_position(&self.target.positions(), current).unwrap_or(WPos::ZERO);
        if self.threshold.length > 0 && (target_pos - self.target_start_pos.get()).length_squared() > self.threshold.length_squared() {
            return true;
        }
        let delta = target_pos - current;
        let facing = if delta.horizontal_length_squared() != 0 {
            delta.yaw()
        } else {
            m.facing()
        };
        if facing != m.facing() {
            m.set_facing(util::tick_facing(m.facing(), facing, m.turn_speed()));
            return false;
        }
        let speed = m.movement_speed_for_cell(self_.location());
        if delta.length_squared() <= (speed as i64) * (speed as i64) {
            m.set_center_position(self_, target_pos);
            return true;
        }
        m.set_center_position(self_, current + delta * speed / delta.length());
        false
    }

    fn get_targets(&self, _self_: &ActorRef) -> Vec<Target> {
        vec![self.target.clone()]
    }
}

enum AdjacentMode {
    Adjacent,
    Onto {
        offset: WVec,
        desired_facing: Option<WAngle>,
        turn: bool,
    },
    WithinRange {
        min_range: WDist,
        max_range: WDist,
        min_cells: i32,
        max_cells: i32,
    },
}

/// `MoveAdjacentTo` and its subclasses `MoveOnto`, `MoveOntoAndTurn` and `MoveWithinRange`.
pub struct MoveAdjacentTo {
    base: ActivityBase,
    me: Weak<MoveAdjacentTo>,
    pub mobile: Rc<Mobile>,
    self_: ActorRef,
    target: RefCell<Target>,
    last_visible_target: RefCell<Target>,
    last_visible_target_location: Cell<CPos>,
    use_last_visible_target: Cell<bool>,
    search_cells: RefCell<Vec<CPos>>,
    search_cells_tick: Cell<i32>,
    mode: AdjacentMode,
}

impl MoveAdjacentTo {
    fn make(self_: &ActorRef, target: &Target, initial: Option<WPos>, mode: AdjacentMode) -> Rc<MoveAdjacentTo> {
        let base = ActivityBase::default();
        base.child_has_priority.set(false);
        let w = self_.world();
        let m = Rc::new_cyclic(|me| MoveAdjacentTo {
            base,
            me: me.clone(),
            mobile: self_.trait_::<Mobile>(),
            self_: self_.clone(),
            target: RefCell::new(target.clone()),
            last_visible_target: RefCell::new(Target::Invalid),
            last_visible_target_location: Cell::new(CPos::ZERO),
            use_last_visible_target: Cell::new(false),
            search_cells: RefCell::new(Vec::new()),
            search_cells_tick: Cell::new(-1),
            mode,
        });
        let tt = target.target_type();
        if (tt == TargetType::Actor && target.actor().unwrap().can_be_viewed_by_player(Some(&self_.owner())))
            || tt == TargetType::FrozenActor
            || tt == TargetType::Terrain
        {
            *m.last_visible_target.borrow_mut() = Target::from_pos(target.center_position());
            m.set_visible_target_location(&w, target);
        } else if let Some(p) = initial {
            *m.last_visible_target.borrow_mut() = Target::from_pos(p);
            m.last_visible_target_location.set(w.map().cell_containing(p));
        }
        m
    }

    pub fn new_adjacent(self_: &ActorRef, target: &Target, initial: Option<WPos>) -> ActivityRef {
        Self::make(self_, target, initial, AdjacentMode::Adjacent)
    }

    pub fn new_onto(self_: &ActorRef, target: &Target, offset: WVec, initial: Option<WPos>) -> ActivityRef {
        Self::make(
            self_,
            target,
            initial,
            AdjacentMode::Onto {
                offset,
                desired_facing: None,
                turn: false,
            },
        )
    }

    pub fn new_onto_and_turn(self_: &ActorRef, target: &Target, offset: WVec, desired_facing: Option<WAngle>) -> ActivityRef {
        Self::make(
            self_,
            target,
            None,
            AdjacentMode::Onto {
                offset,
                desired_facing,
                turn: true,
            },
        )
    }

    pub fn new_within_range(self_: &ActorRef, target: &Target, min_range: WDist, max_range: WDist, initial: Option<WPos>) -> ActivityRef {
        Self::make(
            self_,
            target,
            initial,
            AdjacentMode::WithinRange {
                min_range,
                max_range,
                min_cells: min_range.length / 1024,
                max_cells: (max_range.length + 1023) / 1024,
            },
        )
    }

    fn current_target(&self) -> Target {
        if self.use_last_visible_target.get() {
            self.last_visible_target.borrow().clone()
        } else {
            self.target.borrow().clone()
        }
    }

    fn set_visible_target_location(&self, w: &World, target: &Target) {
        let c = match &self.mode {
            AdjacentMode::Onto { offset, .. } => w.map().cell_containing(self.current_target().center_position() + *offset),
            _ => w.map().cell_containing(target.center_position()),
        };
        self.last_visible_target_location.set(c);
    }

    fn at_correct_range(&self, origin: WPos) -> bool {
        let AdjacentMode::WithinRange { min_range, max_range, .. } = &self.mode else {
            return false;
        };
        let t = self.current_target();
        t.is_in_range(origin, *max_range) && !t.is_in_range(origin, *min_range)
    }

    fn should_stop(&self, self_: &ActorRef) -> bool {
        match &self.mode {
            AdjacentMode::Adjacent => false,
            AdjacentMode::Onto { .. } => self.current_target().target_type() == TargetType::Terrain,
            AdjacentMode::WithinRange { .. } => {
                self.at_correct_range(self_.center_position())
                    && self.mobile.can_interact_with_ground_layer(self_)
                    && self.mobile.can_stay_in_cell(self_.location())
            }
        }
    }

    fn should_repath(&self, self_: &ActorRef, target_location: CPos) -> bool {
        let moved = self.last_visible_target_location.get() != target_location;
        match &self.mode {
            AdjacentMode::WithinRange { .. } => {
                moved
                    && (!self.at_correct_range(self_.center_position())
                        || !self.mobile.can_interact_with_ground_layer(self_)
                        || !self.mobile.can_stay_in_cell(self_.location()))
            }
            _ => moved,
        }
    }

    fn calculate_path_to_target(&self, self_: &ActorRef, check: BlockedByActor) -> (bool, Vec<CPos>) {
        let w = self_.world();
        let m = &self.mobile;
        let lvl = self.last_visible_target_location.get();
        match &self.mode {
            AdjacentMode::Adjacent => {
                if self.search_cells_tick.get() != w.world_tick() {
                    let mut sc = self.search_cells.borrow_mut();
                    sc.clear();
                    self.search_cells_tick.set(w.world_tick());
                    for cell in util::adjacent_cells(&w, &self.current_target()) {
                        if m.can_stay_in_cell(cell) && m.can_enter_cell(cell, None, BlockedByActor::All) {
                            if cell == self_.location() {
                                return (true, Vec::new());
                            }
                            sc.push(cell);
                        }
                    }
                }
            }
            AdjacentMode::Onto { .. } => {
                if lvl == self_.location() {
                    return (true, Vec::new());
                }
                if !m.can_enter_cell(lvl, None, BlockedByActor::All) && util::are_adjacent_cells(lvl, self_.location()) {
                    return (false, Vec::new());
                }
                let mut sc = self.search_cells.borrow_mut();
                if sc.is_empty() {
                    sc.push(lvl);
                } else {
                    sc[0] = lvl;
                }
            }
            AdjacentMode::WithinRange { min_cells, max_cells, .. } => {
                if lvl == self_.location() {
                    return (true, Vec::new());
                }
                if self.search_cells_tick.get() != w.world_tick() {
                    let mut sc = self.search_cells.borrow_mut();
                    sc.clear();
                    self.search_cells_tick.set(w.world_tick());
                    for cell in w.map().find_tiles_in_annulus(lvl, *min_cells, *max_cells, false) {
                        if m.can_stay_in_cell(cell)
                            && m.can_enter_cell(cell, None, BlockedByActor::All)
                            && self.at_correct_range(w.map().center_of_subcell(cell, m.from_subcell.get()))
                        {
                            sc.push(cell);
                        }
                    }
                }
            }
        }
        let sc = self.search_cells.borrow().clone();
        if sc.is_empty() {
            return (false, Vec::new());
        }
        (
            false,
            m.path_finder()
                .find_path_to_target_cells(self_, self_.location(), &sc, check, None, None, true),
        )
    }

    fn queue_move(&self) {
        let me = self.me.clone();
        let self_ = self.self_.clone();
        self.base.queue_child(self.mobile.move_to_path(Rc::new(move |check| {
            me.upgrade().expect("activity alive").calculate_path_to_target(&self_, check)
        })));
    }

    fn base_tick(&self, self_: &ActorRef) -> bool {
        let w = self_.world();
        let old_location = self.last_visible_target_location.get();
        let (t, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = t.clone();
        if !hidden && t.target_type() == TargetType::Actor {
            *self.last_visible_target.borrow_mut() = Target::from_target_positions(&t);
            self.set_visible_target_location(&w, &t);
        }
        let target_is_valid = self.current_target().is_valid_for(self_);
        self.use_last_visible_target.set(hidden || !target_is_valid);
        let no_target = self.use_last_visible_target.get() && !self.last_visible_target.borrow().is_valid_for(self_);
        if self.should_stop(self_) || no_target {
            self.cancel(self_, true);
        } else if !self.base.is_canceling() && target_is_valid && self.should_repath(self_, old_location) {
            if let Some(c) = self.base.child_activity() {
                c.cancel(self_, false);
            }
            self.queue_move();
        }
        if !self.base.tick_child(self_) {
            return false;
        }
        if self.mobile.move_result.get() == MoveResult::CompleteDestinationReached {
            return true;
        }
        self.cancel(self_, true);
        true
    }
}

impl Activity for MoveAdjacentTo {
    activity_common!("MoveAdjacentTo");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        let t = if self.use_last_visible_target.get() {
            &self.last_visible_target
        } else {
            &self.target
        };
        Some(t.borrow().clone())
    }

    fn on_first_run(&self, _self_: &ActorRef) {
        self.queue_move();
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if !self.base_tick(self_) {
            return false;
        }
        if let AdjacentMode::Onto {
            desired_facing,
            turn: true,
            ..
        } = &self.mode
            && !self.base.is_canceling()
            && let Some(f) = desired_facing
            && *f != self.mobile.facing()
        {
            self.base.queue_child(Turn::new(self_, *f));
            return false;
        }
        true
    }

    fn get_targets(&self, self_: &ActorRef) -> Vec<Target> {
        self.base.child_activity().map_or_else(Vec::new, |c| c.get_targets(self_))
    }
}

pub struct Nudge {
    base: ActivityBase,
    nudger: ActorRef,
}

impl Nudge {
    pub fn new(nudger: &ActorRef) -> ActivityRef {
        Rc::new(Nudge {
            base: ActivityBase::default(),
            nudger: nudger.clone(),
        })
    }
}

impl Activity for Nudge {
    activity_common!("Nudge");

    fn target_line_nodes(&self, self_: &ActorRef) -> Vec<TargetLineNode> {
        chain_target_line_nodes(self.base.child_activity(), self_)
    }

    fn on_first_run(&self, self_: &ActorRef) {
        if let Some(m) = self_.trait_or_default::<Mobile>() {
            if m.is_trait_disabled() || m.is_trait_paused() || m.is_immovable() {
                return;
            }
            if let Some(cell) = m.get_adjacent_cell(self.nudger.location(), None) {
                self.base.queue_child(m.move_to(cell, 0, None, false));
            }
        } else if let Some(a) = self_.trait_or_default::<crate::aircraft::Aircraft>() {
            if a.is_trait_disabled() || a.is_trait_paused() || a.require_force_move() {
                return;
            }
            let w = self_.world();
            if !w.map().contains(self_.location()) {
                return;
            }
            let (d, f) = {
                let mut r = w.shared_random.borrow_mut();
                let d = r.next_range(512, 2048);
                let f = r.next_max(256);
                (d, f)
            };
            let offset = WVec::new(0, -d, 0).rotate(&WRot::from_facing(f));
            self.base
                .queue_child(crate::aircraft::Fly::new(self_, Target::from_pos(self_.center_position() + offset)));
            a.un_reserve();
        }
    }
}

pub struct Parachute {
    base: ActivityBase,
    pos: Rc<dyn IPositionable>,
    fall_vector: WVec,
    ground_level: Cell<i32>,
}

impl Parachute {
    pub fn new(self_: &ActorRef) -> ActivityRef {
        let base = ActivityBase::default();
        base.is_interruptible.set(false);
        Rc::new(Parachute {
            base,
            pos: self_.trait_::<dyn IPositionable>(),
            fall_vector: WVec::new(0, 0, crate::parachutable::fall_rate(self_)),
            ground_level: Cell::new(0),
        })
    }
}

impl Activity for Parachute {
    activity_common!("Parachute");

    fn on_first_run(&self, self_: &ActorRef) {
        self.ground_level.set(self_.world().map().center_of_cell(self_.location()).z);
        for n in self_.traits_implementing::<dyn INotifyParachute>() {
            n.on_parachute(self_);
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let next = self_.center_position() - self.fall_vector;
        if next.z < self.ground_level.get() {
            return true;
        }
        self.pos.set_center_position(self_, next);
        false
    }

    fn on_last_run(&self, self_: &ActorRef) {
        let c = self_.center_position();
        self.pos.set_position(self_, c + WVec::new(0, 0, self.ground_level.get() - c.z));
        for n in self_.traits_implementing::<dyn INotifyParachute>() {
            n.on_landed(self_);
        }
    }
}
