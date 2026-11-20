// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The benchmark of `--bench-ticks`: once a game or replay runs, the client plays that many world ticks as fast as
//! possible with one frame built per tick and without vsync, prints one `BENCH` line and quits. The frame skip
//! settings decide which of the frames the main thread draws and whether the simulation waits for them.

use std::time::{Duration, Instant};

use crate::frames::Presented;

/// The measurement of a benchmark, started at the first world tick after the world began to run.
pub struct Bench {
    ticks: i32,
    run: Option<Run>,
}

struct Run {
    start_tick: i32,
    start: Instant,
    start_cpu: Option<f64>,
    /// The idle time of the simulation thread when the run started.
    start_idle: Duration,
    built: u64,
    drawn: u64,
    build: Duration,
    execute: Duration,
    present: Duration,
    main_busy: Duration,
}

impl Bench {
    pub fn new(ticks: i32) -> Bench {
        Bench { ticks, run: None }
    }

    /// Starts the measurement at the first `world_tick` and prints the result once `ticks` world ticks passed.
    /// `idle` is the time that the simulation thread waited so far. Returns true when the benchmark is over.
    pub fn after_logic_tick(&mut self, world_tick: Option<i32>, idle: Duration) -> bool {
        let Some(tick) = world_tick else { return false };
        let Some(run) = &self.run else {
            self.run = Some(Run {
                start_tick: tick,
                start: Instant::now(),
                start_cpu: process_cpu_seconds(),
                start_idle: idle,
                built: 0,
                drawn: 0,
                build: Duration::ZERO,
                execute: Duration::ZERO,
                present: Duration::ZERO,
                main_busy: Duration::ZERO,
            });
            return false;
        };
        let ticks = tick - run.start_tick;
        if ticks < self.ticks {
            return false;
        }
        let wall = run.start.elapsed();
        let cpu = match (process_cpu_seconds(), run.start_cpu) {
            (Some(now), Some(start)) => format!(" cpu {:.1} s", now - start),
            _ => String::new(),
        };
        let ms = |d: Duration, n: u64| d.as_secs_f64() * 1000.0 / n.max(1) as f64;
        let sim_busy = wall.saturating_sub(idle - run.start_idle);
        let ticks = ticks.max(1) as u64;
        println!(
            "BENCH ticks {ticks} from {} wall {:.1} s{cpu} frames {} drawn {} fps {:.1} build {:.2} ms/frame execute {:.2} \
             ms/drawn present {:.2} ms/drawn busy sim {:.2} ms/tick main {:.2} ms/tick",
            run.start_tick,
            wall.as_secs_f64(),
            run.built,
            run.drawn,
            run.drawn as f64 / wall.as_secs_f64(),
            ms(run.build, run.built),
            ms(run.execute, run.drawn),
            ms(run.present, run.drawn),
            ms(sim_busy, ticks),
            ms(run.main_busy, ticks),
        );
        true
    }

    /// Counts a frame that the simulation thread built in `build`.
    pub fn built(&mut self, build: Duration) {
        if let Some(run) = &mut self.run {
            run.built += 1;
            run.build += build;
        }
    }

    /// Counts the report of the main thread on a frame. The execute time includes the uploads of skipped frames.
    pub fn presented(&mut self, report: &Presented) {
        if let Some(run) = &mut self.run {
            run.drawn += u64::from(report.drawn);
            run.execute += report.execute;
            run.present += report.present;
            run.main_busy += report.main_busy;
        }
    }
}

/// The user plus system time of the process in seconds, from `/proc/self/stat` in clock ticks of 1/100 s.
fn process_cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let user: f64 = fields.get(11)?.parse().ok()?;
    let system: f64 = fields.get(12)?.parse().ok()?;
    Some((user + system) / 100.0)
}
