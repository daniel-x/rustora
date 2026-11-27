// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The figures that rustora adds to the performance text: the tick and frame rates with their targets, the split of
//! the logic tick time, the slowest recent tick, the frames built and drawn, and the lag of the players.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The span over which the rates and averages are measured.
const RATE_WINDOW: Duration = Duration::from_secs(1);
/// The span in which the slowest tick is looked for.
pub const SLOWEST_WINDOW: Duration = Duration::from_secs(5);

/// A rate that the client aims at, and what sets it.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// What sets the rate, like the game speed or the vsync.
    pub label: String,
    /// The rate per second, `None` if there is no limit.
    pub rate: Option<f64>,
}

/// A logic tick that ticked the world.
#[derive(Clone, Copy, Debug)]
struct TickRecord {
    at: Instant,
    world_tick: i32,
    /// The time of the whole logic tick.
    total: Duration,
    /// The part of `total` that the world tick with its orders took.
    world: Duration,
}

#[derive(Default)]
struct Stats {
    ticks: VecDeque<TickRecord>,
    built: VecDeque<Instant>,
    drawn: VecDeque<Instant>,
    tick_target: Option<Target>,
    frame_target: Option<Target>,
    lag: Vec<String>,
}

thread_local! {
    static STATS: RefCell<Stats> = RefCell::default();
}

fn trim(times: &mut VecDeque<Instant>, now: Instant) {
    while times.front().is_some_and(|t| now.saturating_duration_since(*t) > RATE_WINDOW) {
        times.pop_front();
    }
}

/// Records a logic tick that ticked the world to `world_tick`, with its time `total` and the part `world` of it.
pub fn record_tick(world_tick: i32, total: Duration, world: Duration) {
    let at = Instant::now();
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        s.ticks.push_back(TickRecord {
            at,
            world_tick,
            total,
            world,
        });
        while s.ticks.front().is_some_and(|t| at.saturating_duration_since(t.at) > SLOWEST_WINDOW) {
            s.ticks.pop_front();
        }
    });
}

/// Forgets the recorded ticks, when a world starts.
pub fn reset_ticks() {
    STATS.with(|s| s.borrow_mut().ticks.clear());
}

/// Records a frame that the simulation thread built.
pub fn frame_built() {
    let now = Instant::now();
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        s.built.push_back(now);
        trim(&mut s.built, now);
    });
}

/// Records a frame that the window drew.
pub fn frame_drawn() {
    let now = Instant::now();
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        s.drawn.push_back(now);
        trim(&mut s.drawn, now);
    });
}

pub fn set_tick_target(target: Option<Target>) {
    STATS.with(|s| s.borrow_mut().tick_target = target);
}

pub fn set_frame_target(target: Option<Target>) {
    STATS.with(|s| s.borrow_mut().frame_target = target);
}

/// Sets the lines of the lag table, empty outside of network games.
pub fn set_lag(lines: Vec<String>) {
    STATS.with(|s| s.borrow_mut().lag = lines);
}

/// The tick figures of the performance text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickStats {
    /// The world ticks per second over the last [`RATE_WINDOW`], 0 while the world does not tick.
    pub rate: f64,
    /// The average logic tick time and its world part in milliseconds over the last [`RATE_WINDOW`].
    pub average_ms: f64,
    pub world_ms: f64,
    /// The world tick and time in milliseconds of the slowest logic tick of the last [`SLOWEST_WINDOW`].
    pub slowest: Option<(i32, f64)>,
    pub target: Option<Target>,
}

pub fn tick_stats() -> TickStats {
    let now = Instant::now();
    STATS.with(|s| {
        let s = s.borrow();
        let recent: Vec<&TickRecord> = s
            .ticks
            .iter()
            .filter(|t| now.saturating_duration_since(t.at) <= RATE_WINDOW)
            .collect();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let n = recent.len();
        let rate = match (recent.first(), recent.last()) {
            (Some(first), Some(last)) if n >= 2 && last.at > first.at => (n - 1) as f64 / (last.at - first.at).as_secs_f64(),
            _ => 0.0,
        };
        let average = |f: fn(&TickRecord) -> Duration| {
            if n == 0 {
                0.0
            } else {
                recent.iter().map(|t| ms(f(t))).sum::<f64>() / n as f64
            }
        };
        TickStats {
            rate,
            average_ms: average(|t| t.total),
            world_ms: average(|t| t.world),
            slowest: s.ticks.iter().max_by_key(|t| t.total).map(|t| (t.world_tick, ms(t.total))),
            target: s.tick_target.clone(),
        }
    })
}

/// The frame figures of the performance text over the last [`RATE_WINDOW`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameStats {
    pub built: usize,
    pub drawn: usize,
    /// The longest time between two drawn frames in milliseconds.
    pub longest_gap_ms: f64,
    pub target: Option<Target>,
}

pub fn frame_stats() -> FrameStats {
    let now = Instant::now();
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        trim(&mut s.built, now);
        trim(&mut s.drawn, now);
        let longest_gap = s.drawn.iter().zip(s.drawn.iter().skip(1)).map(|(a, b)| *b - *a).max();
        FrameStats {
            built: s.built.len(),
            drawn: s.drawn.len(),
            longest_gap_ms: longest_gap.map_or(0.0, |g| g.as_secs_f64() * 1000.0),
            target: s.frame_target.clone(),
        }
    })
}

pub fn lag_lines() -> Vec<String> {
    STATS.with(|s| s.borrow().lag.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slowest_tick_and_averages() {
        reset_ticks();
        record_tick(1, Duration::from_millis(10), Duration::from_millis(8));
        record_tick(2, Duration::from_millis(30), Duration::from_millis(20));
        let stats = tick_stats();
        assert_eq!(stats.slowest, Some((2, 30.0)));
        assert!((stats.average_ms - 20.0).abs() < 1e-9);
        assert!((stats.world_ms - 14.0).abs() < 1e-9);
    }
}
