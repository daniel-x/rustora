// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The simulation thread: the client with its world, network, user interface and audio, the input translation of
//! `Sdl2Input.PumpInput`, and the timing of `Game.Loop`. It builds the frames and sends them to the main thread,
//! which owns the window and the GPU and presents them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rustora_render::Renderer;
use rustora_ui::input::{KeyEvent as UiKeyEvent, KeyInput, Modifiers, MouseButton, MouseEvent, MouseInput, TapHistory};
use rustora_ui::widget::{AppRequest, Clipboard};
use winit::event::{ElementState, KeyEvent, MouseButton as WinitButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{Key, KeyLocation, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

use crate::client::Client;
use crate::frames::{Frame, Presented};

/// `MaxLogicTicksBehind` of `Game.Loop`.
const MAX_LOGIC_BEHIND: Duration = Duration::from_millis(250);
/// The render interval of `Game.Loop` with the default settings, where OpenRA waits for vertical sync.
const RENDER_INTERVAL: Duration = Duration::from_millis(1);
/// `1000 / MinReplayFps` of `Game.Loop`: the longest time between two renders while the logic falls behind.
pub const MAX_RENDER_INTERVAL: Duration = Duration::from_millis(100);
const PIXELS_PER_SCROLL_STEP: f64 = 40.0;
/// The stack size of the simulation thread, that of the main thread on Linux.
const STACK_SIZE: usize = 8 << 20;

/// The user event that wakes the main thread when the simulation thread sent it a message.
#[derive(Debug)]
pub struct Wake;

/// Messages of the main thread to the simulation thread.
pub enum ToSim {
    /// An input or focus event of the window.
    Window(WindowEvent),
    /// The new size of the window in its pixels.
    Resized((u32, u32)),
    /// The window is hidden, or visible again.
    Occluded(bool),
    /// The main thread is done with a frame.
    Presented(Presented),
    /// The window closes.
    Quit,
}

/// Messages of the simulation thread to the main thread.
pub enum ToMain {
    Frame(Frame),
    /// The client quits.
    Quit,
}

/// The sending side of the simulation thread: messages to the main thread, each followed by a [`Wake`].
struct MainLink {
    tx: Sender<ToMain>,
    proxy: EventLoopProxy<Wake>,
}

impl MainLink {
    fn send(&self, message: ToMain) {
        if self.tx.send(message).is_ok() {
            let _ = self.proxy.send_event(Wake);
        }
    }
}

impl Drop for MainLink {
    /// Tells the main thread to quit when the simulation thread ends, also when it panics.
    fn drop(&mut self) {
        self.send(ToMain::Quit);
    }
}

/// What the simulation thread needs to start the client.
pub struct Start {
    pub engine: PathBuf,
    pub support: String,
    /// The requested window size, the resolution of the client.
    pub size: (u32, u32),
    /// The size of the created window in its pixels.
    pub window_size: (u32, u32),
    pub args: Vec<String>,
}

/// Starts the simulation thread, which builds the client and runs [`GameLoop::run`].
pub fn spawn(start: Start, rx: Receiver<ToSim>, tx: Sender<ToMain>, proxy: EventLoopProxy<Wake>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("simulation".to_string())
        .stack_size(STACK_SIZE)
        .spawn(move || {
            let link = MainLink { tx, proxy };
            GameLoop::new(start, rx, link).run();
        })
        .expect("spawn the simulation thread")
}

/// The system clipboard. It is empty and ignores writes where the system offers none.
struct SystemClipboard(RefCell<Option<arboard::Clipboard>>);

impl Clipboard for SystemClipboard {
    fn get(&self) -> String {
        self.0.borrow_mut().as_mut().and_then(|c| c.get_text().ok()).unwrap_or_default()
    }

    fn set(&self, text: &str) {
        if let Some(clipboard) = self.0.borrow_mut().as_mut()
            && let Err(e) = clipboard.set_text(text)
        {
            eprintln!("cannot write the clipboard: {e}");
        }
    }
}

/// The state of the simulation thread.
struct GameLoop {
    client: Client,
    renderer: Renderer,
    rx: Receiver<ToSim>,
    link: MainLink,
    window_size: (u32, u32),
    /// The window is hidden: `Renderer.WindowIsSuspended`.
    suspended: bool,
    modifiers: Modifiers,
    mouse: (i32, i32),
    held: [bool; 3],
    pending_motion: bool,
    scroll_remainder: f64,
    mouse_taps: HashMap<WinitButton, TapHistory>,
    key_taps: HashMap<(String, [bool; 4]), TapHistory>,
    next_logic: Instant,
    next_render: Instant,
    forced_next_render: Instant,
    render_before_next_tick: bool,
    bench: Option<crate::bench::Bench>,
    /// When the last logic tick ran, to log gaps.
    last_logic_tick: Instant,
    quit: bool,
}

impl GameLoop {
    fn new(start: Start, rx: Receiver<ToSim>, link: MainLink) -> GameLoop {
        let resolution = (start.size.0 as i32, start.size.1 as i32);
        let mut client = Client::new(&start.engine, &start.support, resolution, &start.args);
        client.ui.clipboard = Rc::new(SystemClipboard(RefCell::new(arboard::Clipboard::new().ok())));
        client.logic_tick();
        let now = Instant::now();
        let bench = client.bench_ticks().map(crate::bench::Bench::new);
        GameLoop {
            client,
            renderer: Renderer::new(),
            rx,
            link,
            window_size: start.window_size,
            suspended: false,
            modifiers: Modifiers::default(),
            mouse: (0, 0),
            held: [false; 3],
            pending_motion: false,
            scroll_remainder: 0.0,
            mouse_taps: HashMap::new(),
            key_taps: HashMap::new(),
            next_logic: now,
            next_render: now,
            forced_next_render: now,
            render_before_next_tick: false,
            bench,
            last_logic_tick: now,
            quit: false,
        }
    }

    /// Runs passes of `Game.Loop` and handles the messages of the main thread in between until the client or the
    /// window quits.
    fn run(mut self) {
        while !self.quit {
            self.pass();
            if self.quit {
                break;
            }
            let timeout = self.next_wake().saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(timeout) {
                Ok(message) => self.handle(message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            while let Ok(message) = self.rx.try_recv() {
                self.handle(message);
            }
        }
    }

    /// The time of the next logic tick or render.
    fn next_wake(&self) -> Instant {
        self.next_logic.min(self.next_render)
    }

    fn handle(&mut self, message: ToSim) {
        match message {
            ToSim::Window(event) => self.window_event(event),
            ToSim::Resized(size) => self.window_size = size,
            ToSim::Occluded(occluded) => self.suspended = occluded,
            ToSim::Presented(report) => self.presented(report),
            ToSim::Quit => self.quit = true,
        }
    }

    fn window_event(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::ModifiersChanged(state) => {
                self.modifiers = to_modifiers(state.state());
                self.client.ui.modifiers = self.modifiers;
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.client.user_acted();
                let scale = f64::from(self.client.ui_scale());
                self.mouse = ((position.x / scale) as i32, (position.y / scale) as i32);
                self.pending_motion = true;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.client.user_acted();
                self.mouse_button(state, button)
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.client.user_acted();
                self.scroll(delta)
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.client.user_acted();
                self.keyboard(&event)
            }
            WindowEvent::Focused(focused) => self.client.ui.window_has_focus = focused,
            _ => {}
        }
    }

    fn mouse_input(&self, event: MouseEvent, button: MouseButton, scroll_delta: i32, multi_tap: i32) -> MouseInput {
        MouseInput {
            event,
            button,
            x: self.mouse.0,
            y: self.mouse.1,
            scroll_delta,
            modifiers: self.modifiers,
            multi_tap,
        }
    }

    /// The button of a move event. C# passes all held buttons as flags, here the first held one is enough for `==` tests.
    fn held_button(&self) -> MouseButton {
        match self.held {
            [true, _, _] => MouseButton::Left,
            [_, true, _] => MouseButton::Right,
            [_, _, true] => MouseButton::Middle,
            _ => MouseButton::None,
        }
    }

    fn flush_motion(&mut self) {
        if std::mem::take(&mut self.pending_motion) {
            let mi = self.mouse_input(MouseEvent::Move, self.held_button(), 0, 0);
            self.client.ui.handle_input(&mi);
        }
    }

    fn mouse_button(&mut self, state: ElementState, button: WinitButton) {
        let (ui_button, index) = match button {
            WinitButton::Left => (MouseButton::Left, 0),
            WinitButton::Right => (MouseButton::Right, 1),
            WinitButton::Middle => (MouseButton::Middle, 2),
            WinitButton::Back => return self.key_press(state, "MOUSE4".into(), false),
            WinitButton::Forward => return self.key_press(state, "MOUSE5".into(), false),
            WinitButton::Other(_) => return,
        };
        self.flush_motion();
        self.held[index] = state == ElementState::Pressed;
        let taps = self.mouse_taps.entry(button).or_default();
        let (event, count) = match state {
            ElementState::Pressed => (MouseEvent::Down, taps.press(Instant::now(), self.mouse)),
            ElementState::Released => (MouseEvent::Up, taps.count()),
        };
        let mi = self.mouse_input(event, ui_button, 0, count);
        self.client.ui.handle_input(&mi);
    }

    fn scroll(&mut self, delta: MouseScrollDelta) {
        let steps = match delta {
            MouseScrollDelta::LineDelta(_, y) => y as f64,
            MouseScrollDelta::PixelDelta(p) => p.y / PIXELS_PER_SCROLL_STEP,
        };
        self.scroll_remainder += steps;
        let whole = self.scroll_remainder.trunc();
        self.scroll_remainder -= whole;
        if whole != 0.0 {
            self.flush_motion();
            let mi = self.mouse_input(MouseEvent::Scroll, MouseButton::None, whole as i32, 0);
            self.client.ui.handle_input(&mi);
        }
    }

    fn key_press(&mut self, state: ElementState, key: String, repeat: bool) {
        let m = self.modifiers;
        let taps = self.key_taps.entry((key.clone(), [m.shift, m.ctrl, m.alt, m.meta])).or_default();
        let (event, multi_tap) = match state {
            ElementState::Pressed => (UiKeyEvent::Down, taps.press(Instant::now(), (0, 0))),
            ElementState::Released => (UiKeyEvent::Up, taps.count()),
        };
        self.client.ui.handle_key(&KeyInput {
            event,
            key,
            modifiers: m,
            multi_tap,
            repeat,
        });
    }

    fn keyboard(&mut self, event: &KeyEvent) {
        self.flush_motion();
        if let Some(name) = key_name(&event.key_without_modifiers(), event.location) {
            self.key_press(event.state, name, event.repeat);
        }
        if event.state == ElementState::Pressed
            && let Some(text) = event.text_with_all_modifiers().filter(|t| !t.chars().any(char::is_control))
        {
            self.client.ui.handle_text(text);
        }
    }

    /// Opens the links that the client asked for.
    fn open_links(&mut self) {
        for url in std::mem::take(&mut self.client.open_urls) {
            rustora_ui::links::open(&url);
        }
    }

    /// Builds a frame and sends it to the main thread: `Game.RenderTick`.
    fn render(&mut self) {
        let start = Instant::now();
        let render = rustora_ui::perf::PerfSample::new("render");
        self.render_before_next_tick = false;
        self.client.update_cursor(self.mouse);
        let mut frame = build_frame(&mut self.client, &mut self.renderer, self.window_size);
        frame.build = start.elapsed();
        self.link.send(ToMain::Frame(frame));
        drop(render);
        rustora_ui::perf::frame_tick();
    }

    /// Takes the report of a presented frame: the perf counters, the screenshot, and the buffers for reuse.
    fn presented(&mut self, report: Presented) {
        if report.drawn {
            rustora_ui::perf::increment("render_flip", 1000.0 * report.present.as_secs_f64());
            if let Some(bench) = &mut self.bench {
                bench.frame(report.build + report.execute, report.build + report.execute + report.present);
            }
        }
        rustora_ui::perf::increment("batches", report.draw_calls as f64);
        if let Some(pixels) = &report.screenshot {
            self.client.save_screenshot(pixels, report.size.0, report.size.1);
        }
        self.renderer.recycle(report.commands);
    }

    /// One pass of `Game.Loop`: at most one logic tick and one render. A game renders after each world tick, and a
    /// replay that falls behind renders at least every [`MAX_RENDER_INTERVAL`]. While the window is hidden, the
    /// logic runs on without frames.
    fn pass(&mut self) {
        self.flush_motion();
        self.open_links();
        let now = Instant::now();
        if now.saturating_duration_since(self.next_logic) > MAX_LOGIC_BEHIND {
            self.next_logic = now;
        }
        let force_render = self.render_before_next_tick || now >= self.forced_next_render;
        if now >= self.next_logic && !self.render_before_next_tick {
            self.next_logic += Duration::from_millis(self.client.logic_interval() as u64);
            let gap = now.saturating_duration_since(self.last_logic_tick);
            if gap > rustora_launch::net_client::DIAGNOSTIC_GAP {
                log::warn!(target: "client", "No logic tick ran for {:.1} s.", gap.as_secs_f64());
            }
            self.last_logic_tick = now;
            self.client.logic_tick();
            if let Some(bench) = &mut self.bench
                && bench.after_logic_tick(self.client.running_world_tick())
            {
                self.quit = true;
                return;
            }
            if self.client.renders_every_tick() {
                self.render_before_next_tick = true;
            }
        }
        let is_time_to_render = now >= self.next_render;
        if self.suspended {
            if is_time_to_render {
                self.next_render = now + RENDER_INTERVAL;
                self.render_before_next_tick = false;
            }
        } else if (is_time_to_render && now < self.next_logic) || force_render {
            self.next_render = now + RENDER_INTERVAL;
            self.forced_next_render = now + MAX_RENDER_INTERVAL.max(RENDER_INTERVAL);
            self.render();
        }
        if self.client.ui.requests.iter().any(|r| matches!(r, AppRequest::Quit)) {
            self.quit = true;
        }
    }
}

/// Prepares and records one frame of `client` for a window of `size` pixels.
fn build_frame(client: &mut Client, renderer: &mut Renderer, size: (u32, u32)) -> Frame {
    client.prepare(renderer, size);
    client.render(renderer, size);
    Frame {
        commands: renderer.take_frame(),
        size,
        screenshot: std::mem::take(&mut client.screenshot_requested),
        build: Duration::ZERO,
    }
}

fn char_key_name(c: char) -> Option<&'static str> {
    Some(match c {
        ' ' => "SPACE",
        '!' => "EXCLAIM",
        '"' => "QUOTEDBL",
        '#' => "HASH",
        '%' => "PERCENT",
        '$' => "DOLLAR",
        '&' => "AMPERSAND",
        '\'' => "QUOTE",
        '(' => "LEFTPAREN",
        ')' => "RIGHTPAREN",
        '*' => "ASTERISK",
        '+' => "PLUS",
        ',' => "COMMA",
        '-' => "MINUS",
        '.' => "PERIOD",
        '/' => "SLASH",
        ':' => "COLON",
        ';' => "SEMICOLON",
        '<' => "LESS",
        '=' => "EQUALS",
        '>' => "GREATER",
        '?' => "QUESTION",
        '@' => "AT",
        '[' => "LEFTBRACKET",
        '\\' => "BACKSLASH",
        ']' => "RIGHTBRACKET",
        '^' => "CARET",
        '_' => "UNDERSCORE",
        '`' => "BACKQUOTE",
        _ => return None,
    })
}

fn named_key_name(key: NamedKey, location: KeyLocation) -> Option<&'static str> {
    let right = location == KeyLocation::Right;
    Some(match key {
        NamedKey::Enter if location == KeyLocation::Numpad => "KP_ENTER",
        NamedKey::Enter => "RETURN",
        NamedKey::Escape => "ESCAPE",
        NamedKey::Backspace => "BACKSPACE",
        NamedKey::Tab => "TAB",
        NamedKey::Space => "SPACE",
        NamedKey::CapsLock => "CAPSLOCK",
        NamedKey::PrintScreen => "PRINTSCREEN",
        NamedKey::ScrollLock => "SCROLLLOCK",
        NamedKey::Pause => "PAUSE",
        NamedKey::Insert => "INSERT",
        NamedKey::Home => "HOME",
        NamedKey::PageUp => "PAGEUP",
        NamedKey::Delete => "DELETE",
        NamedKey::End => "END",
        NamedKey::PageDown => "PAGEDOWN",
        NamedKey::ArrowRight => "RIGHT",
        NamedKey::ArrowLeft => "LEFT",
        NamedKey::ArrowDown => "DOWN",
        NamedKey::ArrowUp => "UP",
        NamedKey::NumLock => "NUMLOCKCLEAR",
        NamedKey::Control if right => "RCTRL",
        NamedKey::Control => "LCTRL",
        NamedKey::Shift if right => "RSHIFT",
        NamedKey::Shift => "LSHIFT",
        NamedKey::Alt if right => "RALT",
        NamedKey::Alt => "LALT",
        NamedKey::Super if right => "RGUI",
        NamedKey::Super => "LGUI",
        NamedKey::F1 => "F1",
        NamedKey::F2 => "F2",
        NamedKey::F3 => "F3",
        NamedKey::F4 => "F4",
        NamedKey::F5 => "F5",
        NamedKey::F6 => "F6",
        NamedKey::F7 => "F7",
        NamedKey::F8 => "F8",
        NamedKey::F9 => "F9",
        NamedKey::F10 => "F10",
        NamedKey::F11 => "F11",
        NamedKey::F12 => "F12",
        NamedKey::AudioVolumeMute => "MUTE",
        NamedKey::AudioVolumeUp => "VOLUMEUP",
        NamedKey::AudioVolumeDown => "VOLUMEDOWN",
        NamedKey::MediaTrackNext => "AUDIONEXT",
        NamedKey::MediaTrackPrevious => "AUDIOPREV",
        NamedKey::MediaStop => "AUDIOSTOP",
        NamedKey::MediaPlayPause => "AUDIOPLAY",
        _ => return None,
    })
}

fn numpad_char_name(c: char) -> Option<String> {
    Some(match c {
        '0'..='9' => format!("KP_{c}"),
        '/' => "KP_DIVIDE".into(),
        '*' => "KP_MULTIPLY".into(),
        '-' => "KP_MINUS".into(),
        '+' => "KP_PLUS".into(),
        '.' => "KP_PERIOD".into(),
        '=' => "KP_EQUALS".into(),
        _ => return None,
    })
}

/// Names a key as the `Keycode` enum does. A character without a name becomes its code point, which `Hotkey.TryParse`
/// accepts too. Letters and digits ignore the shift state, as SDL key symbols do.
fn key_name(key: &Key, location: KeyLocation) -> Option<String> {
    match key {
        Key::Named(named) => named_key_name(*named, location).map(str::to_string),
        Key::Character(text) => {
            let c = text.chars().next()?;
            if location == KeyLocation::Numpad
                && let Some(name) = numpad_char_name(c)
            {
                return Some(name);
            }
            Some(match c {
                'a'..='z' | 'A'..='Z' => c.to_ascii_uppercase().to_string(),
                '0'..='9' => format!("NUMBER_{c}"),
                _ => char_key_name(c).map_or_else(|| (c as u32).to_string(), str::to_string),
            })
        }
        _ => None,
    }
}

fn to_modifiers(state: ModifiersState) -> Modifiers {
    Modifiers {
        shift: state.shift_key(),
        ctrl: state.control_key(),
        alt: state.alt_key(),
        meta: state.super_key(),
    }
}

#[cfg(test)]
mod tests {
    use winit::keyboard::SmolStr;

    use super::*;

    fn character(text: &str, location: KeyLocation) -> Option<String> {
        key_name(&Key::Character(SmolStr::new(text)), location)
    }

    #[test]
    fn names_keys_as_the_keycode_enum() {
        assert_eq!(character("a", KeyLocation::Standard).as_deref(), Some("A"));
        assert_eq!(character("7", KeyLocation::Standard).as_deref(), Some("NUMBER_7"));
        assert_eq!(character("7", KeyLocation::Numpad).as_deref(), Some("KP_7"));
        assert_eq!(character("-", KeyLocation::Standard).as_deref(), Some("MINUS"));
        assert_eq!(character("ä", KeyLocation::Standard).as_deref(), Some("228"));
        assert_eq!(
            key_name(&Key::Named(NamedKey::Enter), KeyLocation::Standard).as_deref(),
            Some("RETURN")
        );
        assert_eq!(
            key_name(&Key::Named(NamedKey::Enter), KeyLocation::Numpad).as_deref(),
            Some("KP_ENTER")
        );
        assert_eq!(
            key_name(&Key::Named(NamedKey::Shift), KeyLocation::Right).as_deref(),
            Some("RSHIFT")
        );
    }
}
