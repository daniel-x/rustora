// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `IInputHandler.cs`: mouse and keyboard events.

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
