// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `PerfDebugLogic.cs` and `LoadIngamePerfLogic.cs`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::{NoLogic, services};
use crate::loader::cell;
use crate::perf;
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};

/// `LoadIngamePerfLogic`.
pub fn load_ingame_perf(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let root = ui.child(widget, "PERF_ROOT");
    ui.load_widget("PERF_WIDGETS", Some(root), &WidgetArgs::default());
    cell(NoLogic)
}

/// `PerfDebugLogic`: the frame rate is the rolling average over the frames of the last second.
pub fn perf_debug(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let settings = services(ui).settings.clone();
    let graph = ui.child(widget, "GRAPH_BG");
    let s = settings.clone();
    ui.set_visible_fn(graph, move || s.borrow().debug.perf_graph);
    let text = ui.child(widget, "PERF_TEXT");
    let s = settings.clone();
    ui.set_visible_fn(text, move || s.borrow().debug.perf_text);
    let start = Instant::now();
    let timings: RefCell<VecDeque<(u64, Duration)>> = RefCell::new(VecDeque::from([(perf::render_frame(), Duration::ZERO)]));
    let service = services(ui);
    ui.label(text).get_text = Some(Rc::new(move || {
        let mut timings = timings.borrow_mut();
        timings.push_back((perf::render_frame(), start.elapsed()));
        let cutoff = timings.back().expect("timing").1.saturating_sub(Duration::from_secs(1));
        let first_past_cutoff = timings.iter().position(|t| t.1 >= cutoff).unwrap_or(0);
        if timings.len() - first_past_cutoff >= 2 {
            timings.drain(..first_past_cutoff);
        }
        let (oldest, newest) = (timings[0], *timings.back().expect("timing"));
        let seconds = (newest.1 - oldest.1).as_secs_f64();
        let fps = if seconds > 0.0 {
            (newest.0 - oldest.0) as f64 / seconds
        } else {
            0.0
        };
        let samples = service.settings.borrow().debug.samples.max(1) as usize;
        let tick = service.ingame.borrow().as_ref().map_or(0, |i| i.world.world_tick());
        let viewport = service.ingame.borrow().as_ref().map_or((0, 0), |i| i.viewport.borrow().size());
        format!(
            "FPS: {fps:.0}\nTick {tick} @ {:.1} ms\nRender {} @ {:.1} ms\nBatches: {}\nViewport Size: {} x {} / 1\nWFB Size: {} x {}",
            perf::with_item("tick_time", |i| i.average(samples)),
            perf::render_frame(),
            perf::with_item("render", |i| i.average(samples)),
            perf::with_item("batches", |i| i.last_value()),
            viewport.0,
            viewport.1,
            viewport.0,
            viewport.1,
        )
    }));
    cell(NoLogic)
}
