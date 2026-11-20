// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The simulation thread: the client with its world, network, user interface and audio, the input translation of
//! `Sdl2Input.PumpInput`, and the timing of `Game.Loop`. It builds the frames and sends them to the main thread,
//! which owns the window and the GPU and presents them.
//!
//! The simulation thread builds at most one frame after every logic tick and none in between, and puts it into the
//! [`FrameQueue`]. The main thread takes the frames out one by one and draws each. Only the simulation thread decides
//! which frames are skipped, with `Graphics.FrameSkip`:
//!
//! - Off: every tick is drawn. A frame is built after every tick. The queue holds at most one frame. A built frame
//!   that finds the queue full waits outside of it, and the next tick waits until the frame is in the queue. Like
//!   `Game.Loop` with `ThreadedGraphicsContext`, a tick runs while the main thread draws the frame of the tick before,
//!   and the game slows down instead of skipping frames. At most three frames exist: the one drawn, the one in the
//!   queue and the one waiting to get in.
//! - On: ticks never wait for frames. No frame is built after a tick when the logic is behind, that is when the next
//!   tick is due already, unless the last build is `1 / Graphics.FrameSkipMinFps` ago; the uploads that the tick recorded
//!   go into the queue as a frame without draws. The queue holds at most two frames. A frame that finds it full
//!   replaces the newer of the two, which keeps only its uploads; a frame without draws adds its uploads to the newer
//!   one.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rustora_render::Renderer;
use rustora_ui::cursor::CursorImage;
use rustora_ui::input::{KeyEvent as UiKeyEvent, KeyInput, Modifiers, MouseButton, MouseEvent, MouseInput, TapHistory};
use rustora_ui::widget::{AppRequest, Clipboard};
use winit::event::{ElementState, KeyEvent, MouseButton as WinitButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{Key, KeyLocation, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

use crate::client::Client;
use crate::frames::{Frame, FrameQueue, Presented};

/// `MaxLogicTicksBehind` of `Game.Loop`.
const MAX_LOGIC_BEHIND: Duration = Duration::from_millis(250);
/// The longest wait for a message while a frame waits to get into the queue.
const QUEUE_WAIT: Duration = Duration::from_millis(100);
const PIXELS_PER_SCROLL_STEP: f64 = 40.0;
/// The stack size of the simulation thread, that of the main thread on Linux.
const STACK_SIZE: usize = 8 << 20;

/// The user event that wakes the main thread when the simulation thread sent it a message.
#[derive(Debug)]
pub struct Wake;

/// Messages of the main thread to the simulation thread.
pub enum ToSim {
    /// The main thread took a frame out of the queue, so the queue has room.
    FrameTaken,
    /// An input or focus event of the window.
    Window(WindowEvent),
    /// The new size of the window in its pixels.
    Resized((u32, u32)),
    /// The window is hidden, or visible again.
    Occluded(bool),
    /// The scale factor of the window system for the window.
    ScaleFactor(f64),
    /// The main thread is done with a frame.
    Presented(Presented),
    /// The window closes.
    Quit,
}

/// Messages of the simulation thread to the main thread.
pub enum ToMain {
    Cursor(CursorChange),
    /// A link to open in the browser.
    OpenUrl(String),
    /// The client quits.
    Quit,
}

/// The cursor that the window shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShownCursor {
    Hidden,
    /// A frame of a cursor of the mod.
    Mod(String, usize),
    /// The pointing hand of the system, over links.
    Link,
}

/// A change of the hardware cursors. The images of the mod cursors go to the main thread once per mod and scale,
/// the first time they are shown; after that only their key does.
pub enum CursorChange {
    /// The mod or the cursor scale changed: the main thread drops its cursors.
    Reset,
    /// The image of a mod cursor frame, already scaled.
    Image {
        key: (String, usize),
        image: CursorImage,
    },
    Show(ShownCursor),
}

/// The sending side of the simulation thread: messages to the main thread, each followed by a [`Wake`].
struct MainLink {
    tx: Sender<ToMain>,
    proxy: EventLoopProxy<Wake>,
}

impl MainLink {
    fn send(&self, message: ToMain) {
        if self.tx.send(message).is_ok() {
            self.wake();
        }
    }

    /// Wakes the main thread, which then looks at its messages and the frame queue.
    fn wake(&self) {
        let _ = self.proxy.send_event(Wake);
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
    /// The scale factor of the window system for the window.
    pub scale_factor: f64,
    pub args: Vec<String>,
    pub queue: FrameQueue,
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
    scale_factor: f64,
    /// The mod whose cursor images the main thread has.
    cursor_mod: String,
    /// The pixel doubling of the cursor images.
    cursor_scale: u16,
    /// The mod cursor frames whose images the main thread has.
    sent_cursors: HashSet<(String, usize)>,
    shown_cursor: ShownCursor,
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
    bench: Option<crate::bench::Bench>,
    /// When the last logic tick ran, to log gaps.
    last_logic_tick: Instant,
    quit: bool,
    queue: FrameQueue,
    /// A built frame that found the queue full.
    waiting: Option<Frame>,
    /// When the last frame was built.
    last_build: Instant,
    /// The time spent waiting for messages.
    idle: Duration,
}

impl GameLoop {
    fn new(start: Start, rx: Receiver<ToSim>, link: MainLink) -> GameLoop {
        let resolution = (start.size.0 as i32, start.size.1 as i32);
        let mut client = Client::new(&start.engine, &start.support, resolution, &start.args);
        client.ui.clipboard = Rc::new(SystemClipboard(RefCell::new(arboard::Clipboard::new().ok())));
        client.logic_tick();
        let now = Instant::now();
        let bench = client.bench_ticks().map(crate::bench::Bench::new);
        let cursor_mod = client.mod_id().to_string();
        GameLoop {
            client,
            renderer: Renderer::new(),
            rx,
            link,
            window_size: start.window_size,
            scale_factor: start.scale_factor,
            cursor_mod,
            cursor_scale: 1,
            sent_cursors: HashSet::new(),
            shown_cursor: ShownCursor::Hidden,
            suspended: false,
            modifiers: Modifiers::default(),
            mouse: (0, 0),
            held: [false; 3],
            pending_motion: false,
            scroll_remainder: 0.0,
            mouse_taps: HashMap::new(),
            key_taps: HashMap::new(),
            next_logic: now,
            bench,
            last_logic_tick: now,
            quit: false,
            queue: start.queue,
            waiting: None,
            last_build: now,
            idle: Duration::ZERO,
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
            let wait = Instant::now();
            let received = self.rx.recv_timeout(self.next_wake().saturating_duration_since(wait));
            self.idle += wait.elapsed();
            match received {
                Ok(message) => self.handle(message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.handle_waiting();
        }
    }

    /// Handles the messages of the main thread that arrived so far.
    fn handle_waiting(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            self.handle(message);
        }
    }

    /// The time of the next logic tick. While a frame waits to get into the queue, the next tick waits for a message
    /// instead, up to [`QUEUE_WAIT`].
    fn next_wake(&self) -> Instant {
        if self.waiting.is_some() {
            Instant::now() + QUEUE_WAIT
        } else {
            self.next_logic
        }
    }

    fn handle(&mut self, message: ToSim) {
        match message {
            ToSim::Window(event) => self.window_event(event),
            ToSim::Resized(size) => self.window_size = size,
            ToSim::Occluded(occluded) => self.suspended = occluded,
            ToSim::ScaleFactor(scale) => self.scale_factor = scale,
            ToSim::FrameTaken => {}
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

    /// Hands the links that the client asked for to the main thread, which opens them.
    fn open_links(&mut self) {
        for url in std::mem::take(&mut self.client.open_urls) {
            self.link.send(ToMain::OpenUrl(url));
        }
    }

    /// `Sdl2PlatformWindow.CreateHardwareCursor`: cursors are pixel doubled when the window scale is above 1.5 and
    /// again when the player chose doubled cursors. Sends the changes of the shown cursor to the main thread.
    fn apply_cursor(&mut self) {
        let cursor_scale = cursor_scale(self.client.cursor_double(), self.scale_factor);
        if self.cursor_mod != self.client.mod_id() || cursor_scale != self.cursor_scale {
            self.cursor_mod = self.client.mod_id().to_string();
            self.cursor_scale = cursor_scale;
            self.sent_cursors.clear();
            self.shown_cursor = ShownCursor::Hidden;
            self.link.send(ToMain::Cursor(CursorChange::Reset));
        }
        self.client.update_cursor(self.mouse);
        let wanted = if self.client.link_cursor {
            ShownCursor::Link
        } else {
            match self.client.cursor.current(&self.client.cursors) {
                Some((name, frame)) => ShownCursor::Mod(name.to_string(), frame),
                None => ShownCursor::Hidden,
            }
        };
        if wanted == self.shown_cursor {
            return;
        }
        if let ShownCursor::Mod(name, frame) = &wanted {
            let key = (name.clone(), *frame);
            if !self.sent_cursors.contains(&key) {
                let image = self.client.cursors.image(name, *frame).expect("known cursor").scaled(cursor_scale);
                self.sent_cursors.insert(key.clone());
                self.link.send(ToMain::Cursor(CursorChange::Image { key, image }));
            }
        }
        self.link.send(ToMain::Cursor(CursorChange::Show(wanted.clone())));
        self.shown_cursor = wanted;
    }

    /// True if the frame of the tick that just ran is built: always without `Graphics.FrameSkip`, with it only when
    /// the logic keeps up, or when the last build is `1 / Graphics.FrameSkipMinFps` ago.
    fn builds_after_tick(&self) -> bool {
        let now = Instant::now();
        !self.client.frame_skip() || now < self.next_logic || now.duration_since(self.last_build) >= self.client.frame_skip_max_interval()
    }

    /// Puts the uploads that the ticks since the last frame recorded into the queue, for a tick whose build is skipped.
    fn send_uploads(&mut self) {
        let commands = self.renderer.take_frame();
        if commands.is_empty() {
            self.renderer.recycle(commands);
            return;
        }
        self.push_merging(Frame {
            commands,
            size: self.window_size,
            screenshot: false,
        });
    }

    /// Puts `frame` into the queue with `Graphics.FrameSkip`, merging it if the queue is full.
    fn push_merging(&mut self, frame: Frame) {
        let draws = frame.commands.has_draws();
        if let Some(left) = self.queue.push_merging(frame) {
            self.renderer.recycle(left);
            if draws && let Some(bench) = &mut self.bench {
                bench.merged();
            }
        }
        self.link.wake();
    }

    /// Builds a frame and puts it into the queue: `Game.RenderTick` up to the present. The perf items of the frame
    /// take their sample when a frame is drawn, so that they and the frame rate describe the frames on screen.
    fn render(&mut self) {
        let start = Instant::now();
        let render = rustora_ui::perf::PerfSample::new("render");
        self.apply_cursor();
        let frame = build_frame(&mut self.client, &mut self.renderer, self.window_size);
        self.last_build = Instant::now();
        if let Some(bench) = &mut self.bench {
            bench.built(start.elapsed());
        }
        if self.client.frame_skip() {
            self.push_merging(frame);
        } else {
            self.waiting = Some(frame);
            self.enqueue_waiting();
        }
        drop(render);
    }

    /// Puts the waiting frame into the queue if it has room.
    fn enqueue_waiting(&mut self) {
        if self.queue.push_waiting(&mut self.waiting) {
            self.link.wake();
        }
    }

    /// Takes the report of a presented frame: the perf counters, the screenshot, and the buffers for reuse.
    fn presented(&mut self, report: Presented) {
        if let Some(bench) = &mut self.bench {
            bench.presented(&report);
        }
        rustora_ui::perf::increment("batches", report.draw_calls as f64);
        if report.skipped.is_none() {
            rustora_ui::perf::increment("render_flip", 1000.0 * report.present.as_secs_f64());
            rustora_ui::perf::frame_tick();
        }
        if let Some(pixels) = &report.screenshot {
            self.client.save_screenshot(pixels, report.size.0, report.size.1);
        }
        self.renderer.recycle(report.commands);
    }

    /// One pass of `Game.Loop`: at most one logic tick, followed by its frame unless `Graphics.FrameSkip` skips it. A
    /// tick waits while a frame waits to get into the queue. While the window is hidden, the logic runs on without
    /// frames.
    fn pass(&mut self) {
        self.flush_motion();
        self.open_links();
        self.enqueue_waiting();
        let now = Instant::now();
        if now.saturating_duration_since(self.next_logic) > MAX_LOGIC_BEHIND {
            self.next_logic = now;
        }
        if self.waiting.is_none() && now >= self.next_logic {
            if self.client.benchmarking() {
                self.next_logic = now;
            } else {
                self.next_logic += Duration::from_millis(self.client.logic_interval() as u64);
            }
            let gap = now.saturating_duration_since(self.last_logic_tick);
            if gap > rustora_launch::net_client::DIAGNOSTIC_GAP {
                log::warn!(target: "client", "No logic tick ran for {:.1} s.", gap.as_secs_f64());
            }
            self.last_logic_tick = now;
            self.client.logic_tick();
            if let Some(bench) = &mut self.bench
                && bench.after_logic_tick(self.client.running_world_tick(), self.idle)
            {
                self.quit = true;
                return;
            }
            if !self.suspended {
                if self.builds_after_tick() {
                    self.render();
                } else {
                    self.send_uploads();
                    if let Some(bench) = &mut self.bench {
                        bench.skipped_build();
                    }
                }
            }
        }
        if self.client.ui.requests.iter().any(|r| matches!(r, AppRequest::Quit)) {
            self.quit = true;
        }
    }
}

/// The pixel doubling of the hardware cursors for the `CursorDouble` setting and the scale of the window system.
fn cursor_scale(cursor_double: bool, window_scale: f64) -> u16 {
    let native = if window_scale > 1.5 { 2 } else { 1 };
    if cursor_double { 2 * native } else { native }
}

/// Prepares and records one frame of `client` for a window of `size` pixels.
fn build_frame(client: &mut Client, renderer: &mut Renderer, size: (u32, u32)) -> Frame {
    client.prepare(renderer, size);
    client.render(renderer, size);
    Frame {
        commands: renderer.take_frame(),
        size,
        screenshot: std::mem::take(&mut client.screenshot_requested),
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
