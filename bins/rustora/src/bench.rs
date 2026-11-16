// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The benchmark of `--bench-ticks`: once a game or replay runs, the client plays that many world ticks as fast as
//! possible with one render per tick and without vsync, prints one `BENCH` line and quits.

use std::time::{Duration, Instant};

/// The measurement of a benchmark, started at the first world tick after the world began to run.
pub struct Bench {
    ticks: i32,
    run: Option<Run>,
}

struct Run {
    start_tick: i32,
    start: Instant,
    start_cpu: Option<f64>,
    frames: u64,
    scene: Duration,
    total: Duration,
}

impl Bench {
    pub fn new(ticks: i32) -> Bench {
        Bench { ticks, run: None }
    }

    /// Starts the measurement at the first `world_tick` and prints the result once `ticks` world ticks passed.
    /// Returns true when the benchmark is over.
    pub fn after_logic_tick(&mut self, world_tick: Option<i32>) -> bool {
        let Some(tick) = world_tick else { return false };
        let Some(run) = &self.run else {
            self.run = Some(Run {
                start_tick: tick,
                start: Instant::now(),
                start_cpu: process_cpu_seconds(),
                frames: 0,
                scene: Duration::ZERO,
                total: Duration::ZERO,
            });
            return false;
        };
        if tick - run.start_tick < self.ticks {
            return false;
        }
        let wall = run.start.elapsed().as_secs_f64();
        let cpu = match (process_cpu_seconds(), run.start_cpu) {
            (Some(now), Some(start)) => format!(" cpu {:.1} s", now - start),
            _ => String::new(),
        };
        let frames = run.frames.max(1) as f64;
        println!(
            "BENCH ticks {} from {} wall {wall:.1} s{cpu} frames {} fps {:.1} scene {:.2} ms/frame render {:.2} ms/frame",
            tick - run.start_tick,
            run.start_tick,
            run.frames,
            frames / wall,
            run.scene.as_secs_f64() * 1000.0 / frames,
            run.total.as_secs_f64() * 1000.0 / frames
        );
        true
    }

    /// Counts a rendered frame: `scene` until the frame was submitted, `total` including the present.
    pub fn frame(&mut self, scene: Duration, total: Duration) {
        if let Some(run) = &mut self.run {
            run.frames += 1;
            run.scene += scene;
            run.total += total;
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
