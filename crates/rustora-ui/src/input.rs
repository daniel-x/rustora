// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `IInputHandler.cs` and `MultiTapDetection.cs`: mouse and keyboard events.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEvent {
    Down,
    Up,
    Move,
    Scroll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    None,
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    /// `Modifiers.HasModifier` with the bit values of the `Modifiers` enum: shift 1, alt 2, ctrl 4, meta 8.
    pub fn has_flags(self, flags: u32) -> bool {
        let bits = u32::from(self.shift) | u32::from(self.alt) << 1 | u32::from(self.ctrl) << 2 | u32::from(self.meta) << 3;
        bits & flags == flags
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MouseInput {
    pub event: MouseEvent,
    pub button: MouseButton,
    pub x: i32,
    pub y: i32,
    pub scroll_delta: i32,
    pub modifiers: Modifiers,
    pub multi_tap: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    Down,
    Up,
}

/// A key press with the key named as in the `Keycode` enum of C#, for example `RETURN` or `F1`.
#[derive(Clone, Debug)]
pub struct KeyInput {
    pub event: KeyEvent,
    pub key: String,
    pub modifiers: Modifiers,
    pub multi_tap: i32,
    pub repeat: bool,
}

type Tap = Option<(Instant, (i32, i32))>;

/// Port of `TapHistory`: counts repeats of a key or button press within 250 ms and 4 pixels.
#[derive(Clone, Copy, Default)]
pub struct TapHistory {
    first: Tap,
    second: Tap,
    third: Tap,
}

fn close_enough(a: Tap, b: Tap) -> bool {
    let (Some((at, al)), Some((bt, bl))) = (a, b) else {
        return false;
    };
    let (dx, dy) = (al.0 - bl.0, al.1 - bl.1);
    at.duration_since(bt) < Duration::from_millis(250) && dx * dx + dy * dy < 16
}

impl TapHistory {
    /// `GetTapCount`: records a press.
    pub fn press(&mut self, now: Instant, location: (i32, i32)) -> i32 {
        self.first = self.second;
        self.second = self.third;
        self.third = Some((now, location));
        self.count()
    }

    /// `LastTapCount`.
    pub fn count(&self) -> i32 {
        if !close_enough(self.third, self.second) {
            1
        } else if !close_enough(self.second, self.first) {
            2
        } else {
            3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_taps_by_time_and_distance() {
        let start = Instant::now();
        let ms = |n| start + Duration::from_millis(n);
        let mut h = TapHistory::default();
        assert_eq!(h.press(ms(0), (0, 0)), 1);
        assert_eq!(h.press(ms(100), (3, 0)), 2);
        assert_eq!(h.press(ms(200), (3, 0)), 3);
        assert_eq!(h.count(), 3);
        assert_eq!(h.press(ms(300), (3, 4)), 1);
        assert_eq!(h.press(ms(1000), (3, 4)), 1);
    }
}
