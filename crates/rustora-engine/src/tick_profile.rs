// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The time that the world ticks spend per activity type, ticking trait type and on the effects, a rustora addition
//! for the performance text. It measures only while enabled, as every measured call costs a clock read.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use rustora_core::hash::HashMap;

/// What a measured time belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TickPart {
    /// The ticks of the actors whose current activity has this type name, with their child activities.
    Activity(&'static str),
    /// The `ITick` calls of the traits of this type name.
    Trait(&'static str),
    /// The ticks of the effects.
    Effects,
}

/// The measured times of the world ticks since the last [`TickProfile::take`].
#[derive(Default)]
pub struct TickProfile {
    enabled: Cell<bool>,
    ticks: Cell<u32>,
    totals: RefCell<HashMap<TickPart, Duration>>,
}

impl TickProfile {
    pub fn set_enabled(&self, enabled: bool) {
        if !enabled {
            self.ticks.set(0);
            self.totals.borrow_mut().clear();
        }
        self.enabled.set(enabled);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.get()
    }

    /// Counts a measured world tick.
    pub(crate) fn count_tick(&self) {
        self.ticks.set(self.ticks.get() + 1);
    }

    /// Runs `f` and adds its time to `part` while enabled.
    pub(crate) fn measure<R>(&self, part: impl FnOnce() -> TickPart, f: impl FnOnce() -> R) -> R {
        if !self.enabled.get() {
            return f();
        }
        let part = part();
        let start = Instant::now();
        let result = f();
        *self.totals.borrow_mut().entry(part).or_default() += start.elapsed();
        result
    }

    /// The measured world ticks and the times of their parts since the last call, the longest first, and starts anew.
    pub fn take(&self) -> (u32, Vec<(TickPart, Duration)>) {
        let mut parts: Vec<(TickPart, Duration)> = self.totals.borrow_mut().drain().collect();
        parts.sort_by_key(|p| std::cmp::Reverse(p.1));
        (self.ticks.replace(0), parts)
    }
}

/// The type name without its module path and generic arguments.
pub fn short_type_name(name: &str) -> &str {
    let name = name.split('<').next().unwrap_or(name);
    name.rsplit("::").next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_only_while_enabled() {
        let profile = TickProfile::default();
        profile.measure(|| TickPart::Effects, || ());
        assert!(profile.take().1.is_empty());
        profile.set_enabled(true);
        profile.count_tick();
        profile.measure(|| TickPart::Trait("A"), || std::thread::sleep(Duration::from_millis(2)));
        profile.measure(|| TickPart::Effects, || ());
        let (ticks, parts) = profile.take();
        assert_eq!(ticks, 1);
        assert_eq!(parts[0].0, TickPart::Trait("A"));
        assert_eq!(parts.len(), 2);
        assert_eq!(profile.take(), (0, Vec::new()));
    }

    #[test]
    fn shortens_type_names() {
        assert_eq!(short_type_name("rustora_mods_common::auto_target::AutoTarget"), "AutoTarget");
        assert_eq!(short_type_name("a::Foo<b::Bar>"), "Foo");
        assert_eq!(short_type_name("Plain"), "Plain");
    }
}
