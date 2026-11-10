// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Tells how much a game slows down and why, from what the client sees alone, so it also works on OpenRA servers.
//! The client attempts a world tick once per timestep. An attempt at a net frame waits while the orders of a client
//! for that frame are missing; each waiting attempt costs one timestep and counts for its causes. A timestep that
//! the server stretched with its tick scale counts its extra time for the slower players, or for the latest client
//! if a rustora server reports it. The rest of the lost time counts for this computer, which did not attempt the
//! ticks in time.

use std::collections::VecDeque;

/// How far back the report looks.
pub const WINDOW_MS: i64 = 5000;
/// The report shows a game that runs slower than this share of its nominal speed.
const SHOW_BELOW_PERCENT: i32 = 90;
/// How long the report stays after the game recovered.
const HOLD_MS: i64 = 3000;
/// The report needs at least this much history.
const MIN_HISTORY_MS: i64 = 2000;
/// Causes below this share are left out.
const MIN_CAUSE_PERCENT: i32 = 2;

/// Why the game waited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LagCause {
    /// The orders of this client are late.
    Client(i32),
    /// The server acknowledges the own orders late, while the own connection is fine.
    Server,
    /// The server acknowledges the own orders late, and the own connection alone is poor.
    OwnConnection,
    /// The server stretches the timestep so that this client does not run ahead of slower ones.
    SlowerPlayers,
    /// This computer did not attempt the ticks in time.
    ThisComputer,
}

/// The causes of one waiting tick attempt, from the clients whose orders are missing. The server acknowledges the own
/// orders at once, so while their acknowledgement is missing the server or the own connection is slow, and the orders
/// of others that are missing too wait for the same reason. Other clients run ahead by up to the order latency, so
/// their orders may still be there. `own_connection_poor` tells whether the own connection alone is poor, which
/// points at the own connection instead of the server. While the own orders are acknowledged, the clients whose
/// orders are missing are late.
pub fn wait_causes(missing: &[i32], local_client: i32, own_connection_poor: bool) -> Vec<LagCause> {
    if missing.contains(&local_client) {
        return vec![if own_connection_poor {
            LagCause::OwnConnection
        } else {
            LagCause::Server
        }];
    }
    missing.iter().map(|&c| LagCause::Client(c)).collect()
}

enum Attempt {
    /// A tick with the timestep it was scheduled with, and the client whose late orders made the server stretch the
    /// timestep, if known.
    Tick { timestep: i32, slower: Option<i32> },
    /// A waiting attempt with its timestep and causes.
    Wait { timestep: i32, causes: Vec<LagCause> },
}

/// The slowdown of the last [`WINDOW_MS`].
#[derive(Clone, Debug, PartialEq)]
pub struct LagReport {
    /// The game speed in percent of the nominal speed.
    pub speed_percent: i32,
    /// The causes with their share of the time in percent, the largest first.
    pub causes: Vec<(LagCause, i32)>,
}

/// Collects the tick attempts of a running game.
pub struct LagMonitor {
    /// The nominal timestep of the game speed in milliseconds.
    nominal_timestep: i32,
    start: Option<i64>,
    attempts: VecDeque<(i64, Attempt)>,
    shown: Option<(i64, LagReport)>,
}

impl LagMonitor {
    pub fn new(nominal_timestep: i32) -> LagMonitor {
        LagMonitor {
            nominal_timestep: nominal_timestep.max(1),
            start: None,
            attempts: VecDeque::new(),
            shown: None,
        }
    }

    /// A tick at `now` that was scheduled with `timestep`. `slower` is the client whose late orders made the server
    /// stretch the timestep, if a rustora server reported it.
    pub fn tick(&mut self, now: i64, timestep: i32, slower: Option<i32>) {
        self.push(now, Attempt::Tick { timestep, slower });
    }

    /// A waiting attempt at `now` that was scheduled with `timestep`.
    pub fn wait(&mut self, now: i64, timestep: i32, causes: Vec<LagCause>) {
        self.push(now, Attempt::Wait { timestep, causes });
    }

    /// Forgets the history, after a pause in which no ticks were attempted.
    pub fn reset(&mut self) {
        self.start = None;
        self.attempts.clear();
        self.shown = None;
    }

    fn push(&mut self, now: i64, attempt: Attempt) {
        self.start.get_or_insert(now);
        self.attempts.push_back((now, attempt));
        while self.attempts.front().is_some_and(|(t, _)| *t < now - WINDOW_MS) {
            self.attempts.pop_front();
        }
    }

    /// The slowdown of the last [`WINDOW_MS`] before `now`, or `None` with too little history.
    pub fn measure(&self, now: i64) -> Option<LagReport> {
        let span = (now - self.start?).min(WINDOW_MS);
        if span < MIN_HISTORY_MS {
            return None;
        }
        let nominal = self.nominal_timestep as i64;
        let mut ticked = 0i64;
        let mut waited = 0i64;
        let mut stretched = 0i64;
        let mut stretched_unknown = 0i64;
        let mut by_cause: Vec<(LagCause, i64)> = Vec::new();
        let mut add = |cause: LagCause, ms: i64| match by_cause.iter_mut().find(|(c, _)| *c == cause) {
            Some((_, t)) => *t += ms,
            None => by_cause.push((cause, ms)),
        };
        for (t, attempt) in &self.attempts {
            if *t < now - span || *t >= now {
                continue;
            }
            match attempt {
                Attempt::Tick { timestep, slower } => {
                    ticked += nominal;
                    let extra = (*timestep as i64 - nominal).max(0);
                    stretched += extra;
                    match slower {
                        Some(client) if extra > 0 => add(LagCause::Client(*client), extra),
                        _ => stretched_unknown += extra,
                    }
                }
                Attempt::Wait { timestep, causes } => {
                    waited += *timestep as i64;
                    for &cause in causes {
                        add(cause, *timestep as i64);
                    }
                }
            }
        }
        if stretched_unknown > 0 {
            add(LagCause::SlowerPlayers, stretched_unknown);
        }
        let computer = span - ticked.min(span) - waited - stretched;
        if computer > 0 {
            add(LagCause::ThisComputer, computer);
        }
        let percent = |ms: i64| ((ms * 100 + span / 2) / span) as i32;
        let mut causes: Vec<(LagCause, i32)> = by_cause
            .into_iter()
            .map(|(c, ms)| (c, percent(ms)))
            .filter(|&(_, p)| p >= MIN_CAUSE_PERCENT)
            .collect();
        causes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Some(LagReport {
            speed_percent: percent(ticked).min(100),
            causes,
        })
    }

    /// The report to show at `now`: while the game runs slower than [`SHOW_BELOW_PERCENT`], and for [`HOLD_MS`]
    /// after it recovered.
    pub fn report(&mut self, now: i64) -> Option<LagReport> {
        match self.measure(now).filter(|r| r.speed_percent < SHOW_BELOW_PERCENT) {
            Some(r) => {
                self.shown = Some((now, r.clone()));
                Some(r)
            }
            None => self.shown.as_ref().filter(|(t, _)| now - *t < HOLD_MS).map(|(_, r)| r.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(monitor: &mut LagMonitor, from: i64, to: i64, step: i64, mut f: impl FnMut(&mut LagMonitor, i64)) {
        let mut t = from;
        while t < to {
            f(monitor, t);
            t += step;
        }
    }

    #[test]
    fn a_game_at_full_speed_shows_nothing() {
        let mut m = LagMonitor::new(40);
        run(&mut m, 0, 6000, 40, |m, t| m.tick(t, 40, None));
        let r = m.measure(6000).unwrap();
        assert_eq!(r.speed_percent, 100);
        assert!(r.causes.is_empty());
        assert_eq!(m.report(6000), None);
    }

    #[test]
    fn waiting_for_a_client_counts_for_that_client() {
        let mut m = LagMonitor::new(40);
        let mut n = 0;
        run(&mut m, 0, 6000, 40, |m, t| {
            n += 1;
            if n % 4 == 0 {
                m.wait(t, 40, vec![LagCause::Client(2)]);
            } else {
                m.tick(t, 40, None);
            }
        });
        let r = m.report(6000).unwrap();
        assert_eq!(r.speed_percent, 75);
        assert_eq!(r.causes, vec![(LagCause::Client(2), 25)]);
    }

    #[test]
    fn late_attempts_count_for_this_computer_and_stretched_ones_for_slower_players() {
        let mut m = LagMonitor::new(40);
        run(&mut m, 0, 6000, 50, |m, t| m.tick(t, 40, None));
        let r = m.measure(6000).unwrap();
        assert_eq!(r.speed_percent, 80);
        assert_eq!(r.causes, vec![(LagCause::ThisComputer, 20)]);

        let mut m = LagMonitor::new(40);
        run(&mut m, 0, 6000, 44, |m, t| m.tick(t, 44, None));
        let r = m.measure(6000).unwrap();
        assert_eq!(r.speed_percent, 91);
        assert_eq!(r.causes, vec![(LagCause::SlowerPlayers, 9)]);

        let mut m = LagMonitor::new(40);
        run(&mut m, 0, 6000, 44, |m, t| m.tick(t, 44, Some(3)));
        assert_eq!(m.measure(6000).unwrap().causes, vec![(LagCause::Client(3), 9)]);
    }

    #[test]
    fn missing_orders_name_the_cause() {
        assert_eq!(wait_causes(&[2], 0, false), vec![LagCause::Client(2)]);
        assert_eq!(wait_causes(&[1, 2], 0, false), vec![LagCause::Client(1), LagCause::Client(2)]);
        assert_eq!(wait_causes(&[0, 1, 2], 0, false), vec![LagCause::Server]);
        assert_eq!(wait_causes(&[0], 0, false), vec![LagCause::Server]);
        assert_eq!(wait_causes(&[0, 2], 0, true), vec![LagCause::OwnConnection]);
    }

    #[test]
    fn the_report_stays_a_while_after_recovery() {
        let mut m = LagMonitor::new(40);
        run(&mut m, 0, 3000, 40, |m, t| m.wait(t, 40, vec![LagCause::Server]));
        assert!(m.report(3000).is_some());
        run(&mut m, 3000, 9000, 40, |m, t| m.tick(t, 40, None));
        assert!(m.report(9000).is_none());
        m.reset();
        run(&mut m, 0, 3000, 40, |m, t| m.wait(t, 40, vec![LagCause::Server]));
        assert!(m.report(3000).is_some());
        run(&mut m, 3000, 5000, 40, |m, t| m.tick(t, 40, None));
        assert!(m.report(5000).is_some(), "held");
    }
}
