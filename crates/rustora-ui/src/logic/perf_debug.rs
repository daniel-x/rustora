// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `PerfDebugLogic.cs` and `LoadIngamePerfLogic.cs`, with the figures of [`crate::perf_stats`] and the tick
//! profile of the world added to the text.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rustora_engine::tick_profile::{TickPart, short_type_name};

use super::{NoLogic, Services, services};
use crate::draw::Rect;
use crate::input::{KeyEvent, KeyInput};
use crate::loader::cell;
use crate::perf;
use crate::perf_stats::{self, Target};
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::{TextAlign, TextVAlign};

/// How often the parts of the tick profile are summed up.
const PROFILE_INTERVAL: Duration = Duration::from_secs(1);
/// The parts of the tick profile that the text shows.
const PROFILE_TOP: usize = 5;
/// Where the text goes: at the top left, the place of the lag display of rustora, which it replaces.
const TEXT_BOUNDS: Rect = Rect {
    x: 10,
    y: 70,
    w: 640,
    h: 0,
};

/// `LoadIngamePerfLogic`.
pub fn load_ingame_perf(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let root = ui.child(widget, "PERF_ROOT");
    ui.load_widget("PERF_WIDGETS", Some(root), &WidgetArgs::default());
    cell(NoLogic)
}

/// `PerfDebugLogic`: the frame rate is the rolling average over the frames of the last second. Unlike in OpenRA, the
/// text is left aligned at the top left, so that it does not move when its lines change their length.
struct PerfDebug {
    services: Rc<Services>,
    text: Rc<RefCell<String>>,
    start: Instant,
    timings: VecDeque<(u64, Duration)>,
    /// The parts of the tick profile in milliseconds per tick, the longest first, and when they were summed up.
    profile: Vec<(TickPart, f64)>,
    profile_at: Instant,
}

impl PerfDebug {
    fn fps(&mut self) -> f64 {
        self.timings.push_back((perf::render_frame(), self.start.elapsed()));
        let cutoff = self.timings.back().expect("timing").1.saturating_sub(Duration::from_secs(1));
        let first_past_cutoff = self.timings.iter().position(|t| t.1 >= cutoff).unwrap_or(0);
        if self.timings.len() - first_past_cutoff >= 2 {
            self.timings.drain(..first_past_cutoff);
        }
        let (oldest, newest) = (self.timings[0], *self.timings.back().expect("timing"));
        let seconds = (newest.1 - oldest.1).as_secs_f64();
        if seconds > 0.0 {
            (newest.0 - oldest.0) as f64 / seconds
        } else {
            0.0
        }
    }

    /// Sums up the tick profile of the world once per [`PROFILE_INTERVAL`].
    fn update_profile(&mut self) {
        if self.profile_at.elapsed() < PROFILE_INTERVAL {
            return;
        }
        self.profile_at = Instant::now();
        let ingame = self.services.ingame.borrow();
        let Some((ticks, parts)) = ingame.as_ref().map(|i| i.world.tick_profile.take()) else {
            self.profile.clear();
            return;
        };
        self.profile = parts
            .into_iter()
            .take(PROFILE_TOP)
            .map(|(part, total)| (part, total.as_secs_f64() * 1000.0 / ticks.max(1) as f64))
            .collect();
        if ticks == 0 {
            self.profile.clear();
        }
    }

    fn compose(&mut self) -> String {
        let fps = self.fps();
        self.update_profile();
        let samples = self.services.settings.borrow().debug.samples.max(1) as usize;
        let tick = self.services.ingame.borrow().as_ref().map_or(0, |i| i.world.world_tick());
        let viewport = self
            .services
            .ingame
            .borrow()
            .as_ref()
            .map_or((0, 0), |i| i.viewport.borrow().size());
        let wfb = perf::world_buffer().map_or("none".to_string(), |(w, h)| format!("{w} x {h}"));
        let frames = perf_stats::frame_stats();
        let ticks = perf_stats::tick_stats();
        let mut lines = vec![
            format!("{fps:.1} fps ({})", target_text(frames.target.as_ref(), "fps")),
            format!(
                "fps: {} built, {} drawn, {} skipped, longest gap {:.0} ms",
                frames.built,
                frames.drawn,
                frames.built.saturating_sub(frames.drawn),
                frames.longest_gap_ms
            ),
            format!("{:.1} tps ({})", ticks.rate, target_text(ticks.target.as_ref(), "tps")),
            format!(
                "Tick {tick} @ {:.1} ms (world {:.1}, other {:.1})",
                perf::with_item("tick_time", |i| i.average(samples)),
                ticks.world_ms,
                (ticks.average_ms - ticks.world_ms).max(0.0)
            ),
        ];
        if !self.profile.is_empty() {
            let parts: Vec<String> = self
                .profile
                .iter()
                .map(|(part, ms)| format!("{} {ms:.1}", part_name(*part)))
                .collect();
            lines.push(format!("World: {} ms", parts.join(", ")));
        }
        if let Some((tick, ms)) = ticks.slowest {
            lines.push(format!(
                "Slowest tick in {} s: {tick} @ {ms:.1} ms",
                perf_stats::SLOWEST_WINDOW.as_secs()
            ));
        }
        lines.extend([
            format!(
                "Render {} @ {:.1} ms",
                perf::render_frame(),
                perf::with_item("render", |i| i.average(samples))
            ),
            format!("Batches: {}", perf::with_item("batches", |i| i.last_value())),
            format!("Viewport Size: {} x {} / 1", viewport.0, viewport.1),
            format!("WFB Size: {wfb}"),
        ]);
        lines.extend(perf_stats::lag_lines());
        lines.join("\n")
    }
}

impl ChromeLogic for PerfDebug {
    fn tick(&mut self, _ui: &mut Ui) {
        let enabled = self.services.settings.borrow().debug.perf_text;
        if let Some(ingame) = self.services.ingame.borrow().as_ref() {
            ingame.world.tick_profile.set_enabled(enabled);
        }
        if enabled {
            *self.text.borrow_mut() = self.compose();
        }
    }
}

/// `PerfDebugLogic`.
pub fn perf_debug(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let settings = services(ui).settings.clone();
    let graph = ui.child(widget, "GRAPH_BG");
    let s = settings.clone();
    ui.set_visible_fn(graph, move || s.borrow().debug.perf_graph);
    let text_widget = ui.child(widget, "PERF_TEXT");
    let s = settings.clone();
    ui.set_visible_fn(text_widget, move || s.borrow().debug.perf_text);
    let text = Rc::new(RefCell::new(String::new()));
    let shown = text.clone();
    {
        let mut label = ui.label(text_widget);
        label.get_text = Some(Rc::new(move || shown.borrow().clone()));
        label.align = TextAlign::Left;
        label.valign = TextVAlign::Top;
    }
    ui.node_mut(text_widget).bounds = TEXT_BOUNDS;
    let now = Instant::now();
    cell(PerfDebug {
        services: services(ui),
        text,
        start: now,
        timings: VecDeque::from([(perf::render_frame(), Duration::ZERO)]),
        profile: Vec::new(),
        profile_at: now,
    })
}

/// Toggles the perf text or graph if `e` presses the `TogglePerfText` or `TogglePerfGraph` hotkey, a rustora
/// addition. Returns true if it did.
pub fn toggle_by_hotkey(ui: &mut Ui, e: &KeyInput) -> bool {
    if e.event != KeyEvent::Down {
        return false;
    }
    let (text, graph) = {
        let hotkeys = &ui.assets.borrow().hotkeys;
        (
            hotkeys.is_activated("TogglePerfText", e),
            hotkeys.is_activated("TogglePerfGraph", e),
        )
    };
    if !text && !graph {
        return false;
    }
    let settings = services(ui).settings.clone();
    let debug = &mut settings.borrow_mut().debug;
    if text {
        debug.perf_text ^= true;
    }
    if graph {
        debug.perf_graph ^= true;
    }
    true
}

/// "<label>: <rate> <unit>", the label alone without a limit, or "unknown".
fn target_text(target: Option<&Target>, unit: &str) -> String {
    match target {
        Some(Target { label, rate: Some(rate) }) => format!("{label}: {rate:.1} {unit}"),
        Some(Target { label, rate: None }) => label.clone(),
        None => "unknown".to_string(),
    }
}

fn part_name(part: TickPart) -> String {
    match part {
        TickPart::Activity(name) => format!("{} (activity)", short_type_name(name)),
        TickPart::Trait(name) => short_type_name(name).to_string(),
        TickPart::Effects => "effects".to_string(),
    }
}
