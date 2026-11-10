// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Measures how far clients lag behind each other and computes their tick scales (`Server/OrderBuffer.cs`).

use std::collections::VecDeque;
use std::time::Instant;

use indexmap::IndexMap;

const NUMBER_OF_FRAMES: usize = 20;
const INTERVAL: i64 = 1000;
const MAX_TICK_SCALE: f32 = 1.1;
const EMPTY_VALUE: i64 = -1;

pub struct OrderBuffer {
    game_timer: Instant,
    next_update: i64,
    timestamps: IndexMap<i32, i64>,
    deltas: IndexMap<i32, VecDeque<i64>>,
    timestep: i32,
    ticks_per_interval: i32,
    baseline_player: i32,
    players: Vec<i32>,
}

impl OrderBuffer {
    pub fn start(timestep: i32, players: Vec<i32>) -> OrderBuffer {
        OrderBuffer {
            game_timer: Instant::now(),
            next_update: INTERVAL,
            timestamps: players.iter().map(|&p| (p, EMPTY_VALUE)).collect(),
            deltas: players.iter().map(|&p| (p, VecDeque::new())).collect(),
            timestep,
            ticks_per_interval: INTERVAL as i32 / timestep,
            baseline_player: players.first().copied().unwrap_or(0),
            players,
        }
    }

    pub fn add_order_timestamp(&mut self, player_index: i32) {
        let now = self.game_timer.elapsed().as_millis() as i64;
        self.timestamps.insert(player_index, now);
        if self.timestamps.values().any(|&t| t == EMPTY_VALUE) {
            return;
        }
        let baseline = self.timestamps.get(&self.baseline_player).copied().unwrap_or(0);
        for (p, q) in &mut self.timestamps {
            if let Some(d) = self.deltas.get_mut(p) {
                d.push_back(baseline - *q);
                if d.len() > NUMBER_OF_FRAMES {
                    d.pop_front();
                }
            }
            *q = EMPTY_VALUE;
        }
    }

    /// The tick scales to send, once per interval when every player has a full delta history.
    pub fn get_tick_scales(&mut self) -> Vec<(i32, f32)> {
        let now = self.game_timer.elapsed().as_millis() as i64;
        if now < self.next_update {
            return Vec::new();
        }
        self.next_update = now + INTERVAL;
        if self.deltas.is_empty() || self.deltas.values().any(|q| q.len() != NUMBER_OF_FRAMES) {
            return Vec::new();
        }
        let medians: Vec<(i32, i64)> = self.deltas.iter().map(|(&p, q)| (p, median(q))).collect();
        let min_value = medians.iter().map(|m| m.1).min().unwrap_or(0);
        let offset = if min_value < 0 { min_value.abs() } else { 0 };
        let timestep = self.timestep as f32;
        medians
            .into_iter()
            .map(|(p, delta)| {
                let delta_per_tick = (delta + offset) as f32 / self.ticks_per_interval as f32;
                (p, ((timestep + delta_per_tick) / timestep).clamp(1.0, MAX_TICK_SCALE))
            })
            .collect()
    }

    /// How much later the orders of each player arrive than those of the earliest one, the median of the recent
    /// frames in milliseconds. Players without measured frames are left out.
    pub fn lateness(&self) -> Vec<(i32, i64)> {
        let medians: Vec<(i32, i64)> = self
            .deltas
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .map(|(&p, q)| (p, median(q)))
            .collect();
        let earliest = medians.iter().map(|m| m.1).max().unwrap_or(0);
        medians.into_iter().map(|(p, m)| (p, earliest - m)).collect()
    }

    pub fn remove_player(&mut self, player: i32) {
        self.players.retain(|&p| p != player);
        if player == self.baseline_player
            && let Some(&b) = self.players.first()
        {
            self.baseline_player = b;
        }
        self.timestamps.shift_remove(&player);
        self.deltas.shift_remove(&player);
    }
}

fn median(q: &VecDeque<i64>) -> i64 {
    let mut a: Vec<i64> = q.iter().copied().collect();
    a.sort_unstable();
    let n = a.len();
    if n % 2 != 0 { a[n / 2] } else { (a[(n - 1) / 2] + a[n / 2]) / 2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_even_and_odd() {
        assert_eq!(median(&VecDeque::from([3, 1, 2])), 2);
        assert_eq!(median(&VecDeque::from([4, 1, 2, 3])), 2);
    }

    #[test]
    fn no_scales_before_the_history_is_full() {
        let mut b = OrderBuffer::start(40, vec![0, 1]);
        b.next_update = 0;
        b.add_order_timestamp(0);
        b.add_order_timestamp(1);
        assert!(b.get_tick_scales().is_empty());
    }

    #[test]
    fn lateness_is_measured_from_the_earliest_player() {
        let mut b = OrderBuffer::start(40, vec![0, 1]);
        b.deltas.insert(0, VecDeque::from([0, 0, 0]));
        b.deltas.insert(1, VecDeque::from([-300, -400, -350]));
        assert_eq!(b.lateness(), vec![(0, 0), (1, 350)]);
    }

    #[test]
    fn removed_baseline_moves_to_the_next_player() {
        let mut b = OrderBuffer::start(40, vec![0, 1]);
        b.remove_player(0);
        assert_eq!(b.baseline_player, 1);
    }
}
