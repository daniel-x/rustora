// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `PerfHistory`, `PerfItem` and `PerfSample`: timings of the logic ticks and the frames for the performance
//! graph and text.

use std::cell::RefCell;
use std::time::Instant;

use rustora_core::fieldloader::Color;

const SAMPLE_COUNT: usize = 100;

/// `PerfHistory.Colors`.
const COLORS: [u32; 14] = [
    0xFF0000, 0x008000, 0xFFA500, 0xFFFF00, 0xFF00FF, 0x00FF00, 0x00FFFF, 0x0000FF, 0xFFFFFF, 0x008080, 0xFFC0CB, 0x9370DB, 0x808000,
    0x6495ED,
];

/// `PerfItem`: a ring of the last 100 sums.
pub struct PerfItem {
    pub name: String,
    pub color: Color,
    samples: [f64; SAMPLE_COUNT],
    value: f64,
    head: usize,
    tail: usize,
    has_normal_tick: bool,
}

impl PerfItem {
    fn new(name: &str, color: Color) -> PerfItem {
        PerfItem {
            name: name.to_string(),
            color,
            samples: [0.0; SAMPLE_COUNT],
            value: 0.0,
            head: 1,
            tail: 0,
            has_normal_tick: true,
        }
    }

    fn tick(&mut self) {
        self.samples[self.head] = self.value;
        self.head = (self.head + 1) % SAMPLE_COUNT;
        if self.head == self.tail {
            self.tail = (self.tail + 1) % SAMPLE_COUNT;
        }
        self.value = 0.0;
    }

    /// `Samples`: the newest first.
    pub fn samples(&self) -> Vec<f64> {
        let mut out = Vec::new();
        let mut n = self.head;
        while n != self.tail {
            n = (n + SAMPLE_COUNT - 1) % SAMPLE_COUNT;
            out.push(self.samples[n]);
        }
        out
    }

    /// `Average` of the newest `count` samples.
    pub fn average(&self, count: usize) -> f64 {
        let newest: Vec<f64> = self.samples().into_iter().take(count).collect();
        if newest.is_empty() {
            0.0
        } else {
            newest.iter().sum::<f64>() / newest.len() as f64
        }
    }

    /// `LastValue`.
    pub fn last_value(&self) -> f64 {
        self.samples[(self.head + SAMPLE_COUNT - 1) % SAMPLE_COUNT]
    }

    fn reset(&mut self) {
        self.head = 1;
        self.tail = 0;
        self.value = 0.0;
    }
}

#[derive(Default)]
struct History {
    items: Vec<PerfItem>,
    next_color: usize,
    render_frame: u64,
}

impl History {
    fn item(&mut self, name: &str) -> &mut PerfItem {
        let index = match self.items.iter().position(|i| i.name == name) {
            Some(i) => i,
            None => {
                let color = Color::from_argb(
                    255,
                    (COLORS[self.next_color] >> 16) as u8,
                    (COLORS[self.next_color] >> 8) as u8,
                    COLORS[self.next_color] as u8,
                );
                self.next_color = (self.next_color + 1) % COLORS.len();
                self.items.push(PerfItem::new(name, color));
                self.items.len() - 1
            }
        };
        &mut self.items[index]
    }
}

thread_local! {
    static HISTORY: RefCell<History> = RefCell::default();
}

/// `PerfHistory.Increment`.
pub fn increment(name: &str, x: f64) {
    HISTORY.with(|h| h.borrow_mut().item(name).value += x);
}

/// `PerfSample`: adds the milliseconds until the guard drops to the item.
pub struct PerfSample {
    name: &'static str,
    start: Instant,
}

impl PerfSample {
    pub fn new(name: &'static str) -> PerfSample {
        PerfSample {
            name,
            start: Instant::now(),
        }
    }
}

impl Drop for PerfSample {
    fn drop(&mut self) {
        increment(self.name, 1000.0 * self.start.elapsed().as_secs_f64());
    }
}

/// `PerfHistory.Tick`: a sample of each item that ticks with the world.
pub fn tick() {
    HISTORY.with(|h| {
        h.borrow_mut()
            .items
            .iter_mut()
            .filter(|i| i.has_normal_tick)
            .for_each(PerfItem::tick)
    });
}

/// The frame items of `Game.RenderTick`, which tick once per frame instead of with the world.
const FRAME_ITEMS: [&str; 5] = ["render", "batches", "render_world", "render_widgets", "render_flip"];

/// The end of `Game.RenderTick`: a sample of each frame item, and `Game.RenderFrame` counts up.
pub fn frame_tick() {
    HISTORY.with(|h| {
        let mut h = h.borrow_mut();
        for name in FRAME_ITEMS {
            let item = h.item(name);
            item.has_normal_tick = false;
            item.tick();
        }
        h.render_frame += 1;
    });
}

/// `Game.RenderFrame`.
pub fn render_frame() -> u64 {
    HISTORY.with(|h| h.borrow().render_frame)
}

/// `PerfHistory.Reset`.
pub fn reset() {
    HISTORY.with(|h| h.borrow_mut().items.iter_mut().for_each(PerfItem::reset));
}

/// Runs `f` with the items in the order of their first use.
pub fn with_items<R>(f: impl FnOnce(&[PerfItem]) -> R) -> R {
    HISTORY.with(|h| f(&h.borrow().items))
}

/// `PerfHistory.Items[name]`, through `f`.
pub fn with_item<R>(name: &str, f: impl FnOnce(&PerfItem) -> R) -> R {
    HISTORY.with(|h| f(h.borrow_mut().item(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_newest_first_and_limited() {
        let mut item = PerfItem::new("x", Color::default());
        for v in 0..150 {
            item.value = v as f64;
            item.tick();
        }
        let samples = item.samples();
        assert_eq!(samples.len(), SAMPLE_COUNT - 1);
        assert_eq!(samples[0], 149.0);
        assert_eq!(item.last_value(), 149.0);
        assert_eq!(item.average(2), 148.5);
    }
}
