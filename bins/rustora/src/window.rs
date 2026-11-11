// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The client window: port of `Sdl2Input.PumpInput`, `Game.Loop` and the hardware cursors of `Sdl2PlatformWindow`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustora_client::settings::{Settings, WindowMode};
use rustora_render::{Renderer, wgpu};
use rustora_ui::input::{KeyEvent as UiKeyEvent, KeyInput, Modifiers, MouseButton, MouseEvent, MouseInput, TapHistory};
use rustora_ui::widget::{AppRequest, Clipboard};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton as WinitButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyLocation, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CustomCursor, Fullscreen, Window, WindowId};

use crate::client::Client;

/// `MaxLogicTicksBehind` of `Game.Loop`.
const MAX_LOGIC_BEHIND: Duration = Duration::from_millis(250);
/// The render interval of `Game.Loop` with the default settings: the swap chain waits for vertical sync.
const RENDER_INTERVAL: Duration = Duration::from_millis(1);
/// `1000 / MinReplayFps` of `Game.Loop`: the longest time between two renders while the logic falls behind.
const MAX_RENDER_INTERVAL: Duration = Duration::from_millis(100);
/// A requested redraw that has not come after this time means that the window system does not let the window draw,
/// like Wayland for a hidden window, which sends no frame callbacks. The logic then runs without waiting for frames.
const REDRAW_STALL: Duration = Duration::from_millis(100);
const PIXELS_PER_SCROLL_STEP: f64 = 40.0;

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
}

/// How long a link waits for an activation token of the window system before it opens without one.
const ACTIVATION_TOKEN_WAIT: Duration = Duration::from_secs(1);

/// The cursor that the window shows.
#[derive(Clone, PartialEq, Eq)]
enum ShownCursor {
    Hidden,
    /// A frame of a cursor of the mod.
    Mod(String, usize),
    /// The pointing hand of the system, over links.
    Link,
}

/// Asks the window system for an activation token, which X11 and Wayland deliver with `ActivationTokenDone`.
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn request_activation_token(window: &Window) -> Option<winit::event_loop::AsyncRequestSerial> {
    use winit::platform::startup_notify::WindowExtStartupNotify;
    window.request_activation_token().ok()
}

/// Other systems let the browser come to the front without a token.
#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
fn request_activation_token(_window: &Window) -> Option<winit::event_loop::AsyncRequestSerial> {
    None
}

struct App {
    client: Client,
    size: (u32, u32),
    fullscreen: Option<Fullscreen>,
    gpu: Option<Gpu>,
    modifiers: Modifiers,
    mouse: (i32, i32),
    held: [bool; 3],
    pending_motion: bool,
    scroll_remainder: f64,
    mouse_taps: HashMap<WinitButton, TapHistory>,
    key_taps: HashMap<(String, [bool; 4]), TapHistory>,
    /// The mod whose cursors `hardware_cursors` holds.
    cursor_mod: String,
    hardware_cursors: HashMap<(String, usize), CustomCursor>,
    /// The pixel doubling of `hardware_cursors`.
    cursor_scale: u16,
    shown_cursor: ShownCursor,
    /// The links that wait for an activation token of the window system, with the time they were requested.
    links_awaiting_token: Vec<(winit::event_loop::AsyncRequestSerial, String, Instant)>,
    suspended: bool,
    /// When the redraw that the window waits for was requested.
    redraw_requested: Option<Instant>,
    next_logic: Instant,
    next_render: Instant,
    forced_next_render: Instant,
    render_before_next_tick: bool,
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

/// Opens a window with the user interface of the mod and runs until the window closes.
/// Opens the client window. Without an explicit `size`, the window follows the `Graphics.Mode` setting like
/// OpenRA: fullscreen modes cover the monitor, windowed mode uses `Graphics.WindowedSize`.
pub fn run(engine: &Path, support: &str, size: Option<(u32, u32)>, args: &[String]) {
    rustora_core::log_channels::init(Path::new(support), rustora_core::log_channels::CLIENT_CHANNELS);
    rustora_core::log_channels::install_panic_hook(Path::new(support));
    let mut launcher = Launcher {
        engine: engine.to_path_buf(),
        support: support.to_string(),
        size,
        args: args.to_vec(),
        app: None,
    };
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.run_app(&mut launcher).expect("run event loop");
    log::logger().flush();
}

/// Creates the `App` once the event loop can report the monitor size, then forwards all events to it.
struct Launcher {
    engine: PathBuf,
    support: String,
    size: Option<(u32, u32)>,
    args: Vec<String>,
    app: Option<App>,
}

impl Launcher {
    fn window_layout(&self, el: &ActiveEventLoop) -> ((u32, u32), Option<Fullscreen>) {
        if let Some(size) = self.size {
            return (size, None);
        }
        let graphics = Settings::load(Path::new(&self.support).join("settings.yaml"), &self.args).graphics;
        let windowed = (graphics.windowed_size.0.max(1) as u32, graphics.windowed_size.1.max(1) as u32);
        let Some(monitor) = el.primary_monitor().or_else(|| el.available_monitors().next()) else {
            return (windowed, None);
        };
        let desktop = (monitor.size().width, monitor.size().height);
        match graphics.mode {
            WindowMode::Windowed => (windowed, None),
            WindowMode::PseudoFullscreen => (desktop, Some(Fullscreen::Borderless(Some(monitor)))),
            WindowMode::Fullscreen => {
                let wanted = (graphics.fullscreen_size.0 as u32, graphics.fullscreen_size.1 as u32);
                let mode = monitor
                    .video_modes()
                    .filter(|m| (m.size().width, m.size().height) == wanted)
                    .max_by_key(|m| (m.bit_depth(), m.refresh_rate_millihertz()));
                match mode {
                    Some(m) => (wanted, Some(Fullscreen::Exclusive(m))),
                    None => (desktop, Some(Fullscreen::Borderless(Some(monitor)))),
                }
            }
        }
    }
}

impl ApplicationHandler for Launcher {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.app.is_none() {
            let (size, fullscreen) = self.window_layout(el);
            self.app = Some(App::new(&self.engine, &self.support, size, fullscreen, &self.args));
        }
        if let Some(app) = self.app.as_mut() {
            app.resumed(el);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(app) = self.app.as_mut() {
            app.window_event(el, id, event);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(app) = self.app.as_mut() {
            app.about_to_wait(el);
        }
    }
}

impl App {
    fn new(engine: &Path, support: &str, size: (u32, u32), fullscreen: Option<Fullscreen>, args: &[String]) -> App {
        let mut client = Client::new(engine, support, (size.0 as i32, size.1 as i32), args);
        client.ui.clipboard = Rc::new(SystemClipboard(RefCell::new(arboard::Clipboard::new().ok())));
        client.logic_tick();
        let now = Instant::now();
        let client_mod = client.mod_id().to_string();
        App {
            client,
            size,
            fullscreen,
            gpu: None,
            modifiers: Modifiers::default(),
            mouse: (0, 0),
            held: [false; 3],
            pending_motion: false,
            scroll_remainder: 0.0,
            mouse_taps: HashMap::new(),
            key_taps: HashMap::new(),
            cursor_mod: client_mod,
            hardware_cursors: HashMap::new(),
            cursor_scale: 1,
            shown_cursor: ShownCursor::Hidden,
            links_awaiting_token: Vec::new(),
            suspended: false,
            redraw_requested: None,
            next_logic: now,
            next_render: now,
            forced_next_render: now,
            render_before_next_tick: false,
        }
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

impl App {
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

    /// `Sdl2PlatformWindow.CreateHardwareCursor`: cursors are pixel doubled when the window scale is above 1.5 and
    /// again when the player chose doubled cursors.
    fn apply_cursor(&mut self, el: &ActiveEventLoop) {
        if self.cursor_mod != self.client.mod_id() {
            self.cursor_mod = self.client.mod_id().to_string();
            self.hardware_cursors.clear();
            self.shown_cursor = ShownCursor::Hidden;
        }
        let Some(gpu) = &self.gpu else { return };
        let cursor_scale = cursor_scale(self.client.cursor_double(), gpu.window.scale_factor());
        if cursor_scale != self.cursor_scale {
            self.cursor_scale = cursor_scale;
            self.hardware_cursors.clear();
            self.shown_cursor = ShownCursor::Hidden;
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
        match &wanted {
            ShownCursor::Hidden => gpu.window.set_cursor_visible(false),
            ShownCursor::Link => {
                gpu.window.set_cursor(winit::window::CursorIcon::Pointer);
                gpu.window.set_cursor_visible(true);
            }
            ShownCursor::Mod(name, frame) => {
                let key = (name.clone(), *frame);
                if !self.hardware_cursors.contains_key(&key) {
                    let image = self.client.cursors.image(&key.0, key.1).expect("known cursor").scaled(cursor_scale);
                    let source = CustomCursor::from_rgba(image.rgba.clone(), image.width, image.height, image.hotspot.0, image.hotspot.1)
                        .expect("valid cursor image");
                    self.hardware_cursors.insert(key.clone(), el.create_custom_cursor(source));
                }
                gpu.window.set_cursor(self.hardware_cursors[&key].clone());
                gpu.window.set_cursor_visible(true);
            }
        }
        self.shown_cursor = wanted;
    }

    /// Opens the links that the client asked for. Where the window system hands out activation tokens, a link
    /// waits for one, so that the browser may come to the front; without a token after [`ACTIVATION_TOKEN_WAIT`], it
    /// opens anyway.
    fn open_links(&mut self) {
        let now = Instant::now();
        for url in std::mem::take(&mut self.client.open_urls) {
            match self.gpu.as_ref().and_then(|gpu| request_activation_token(&gpu.window)) {
                Some(serial) => self.links_awaiting_token.push((serial, url, now)),
                None => rustora_ui::links::open(&url),
            }
        }
        self.links_awaiting_token.retain(|(_, url, requested)| {
            let waiting = now.duration_since(*requested) < ACTIVATION_TOKEN_WAIT;
            if !waiting {
                rustora_ui::links::open(url);
            }
            waiting
        });
    }

    fn render(&mut self, el: &ActiveEventLoop) {
        let render = rustora_ui::perf::PerfSample::new("render");
        self.render_before_next_tick = false;
        self.redraw_requested = None;
        self.apply_cursor(el);
        let Some(gpu) = self.gpu.as_mut() else { return };
        self.client.prepare(&mut gpu.renderer, (gpu.config.width, gpu.config.height));
        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                gpu.surface.configure(&gpu.renderer.device, &gpu.config);
                return;
            }
        };
        let size = (gpu.config.width, gpu.config.height);
        if std::mem::take(&mut self.client.screenshot_requested) {
            let client = &mut self.client;
            let mut pixels = rustora_render::render_offscreen_as(&mut gpu.renderer, size.0, size.1, gpu.config.format, |r, v| {
                client.render(r, v, size)
            });
            if matches!(gpu.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
                pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p.swap(0, 2));
            }
            pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
            client.save_screenshot(&pixels, size.0, size.1);
        }
        let view = frame.texture.create_view(&Default::default());
        self.client.render(&mut gpu.renderer, &view, size);
        {
            let _flip = rustora_ui::perf::PerfSample::new("render_flip");
            gpu.window.pre_present_notify();
            gpu.renderer.queue.present(frame);
        }
        rustora_ui::perf::increment("batches", gpu.renderer.take_draw_calls() as f64);
        drop(render);
        rustora_ui::perf::frame_tick();
    }

    fn create_gpu(&self, el: &ActiveEventLoop) -> Gpu {
        let window = Arc::new(
            el.create_window(
                Window::default_attributes()
                    .with_title("rustora")
                    .with_resizable(false)
                    .with_fullscreen(self.fullscreen.clone())
                    .with_inner_size(winit::dpi::PhysicalSize::new(self.size.0, self.size.1)),
            )
            .expect("create window"),
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
            el.owned_display_handle(),
        )));
        let surface = instance.create_surface(window.clone()).expect("create surface");
        let adapter = rustora_render::pollster_block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("graphics adapter");
        let (device, queue) =
            rustora_render::pollster_block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("graphics device");
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface config");
        let caps = surface.get_capabilities(&adapter);
        config.format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(config.format);
        surface.configure(&device, &config);
        let renderer = Renderer::new(device, queue, config.format);
        Gpu {
            window,
            surface,
            config,
            renderer,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gpu.is_none() {
            self.gpu = Some(self.create_gpu(el));
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.config.width = size.width.max(1);
                    gpu.config.height = size.height.max(1);
                    gpu.surface.configure(&gpu.renderer.device, &gpu.config);
                }
            }
            WindowEvent::Occluded(occluded) => self.suspended = occluded,
            WindowEvent::ModifiersChanged(state) => {
                self.modifiers = to_modifiers(state.state());
                self.client.ui.modifiers = self.modifiers;
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = f64::from(self.client.ui_scale());
                self.mouse = ((position.x / scale) as i32, (position.y / scale) as i32);
                self.pending_motion = true;
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse_button(state, button),
            WindowEvent::MouseWheel { delta, .. } => self.scroll(delta),
            WindowEvent::KeyboardInput { event, .. } => self.keyboard(&event),
            WindowEvent::Focused(focused) => self.client.ui.window_has_focus = focused,
            WindowEvent::RedrawRequested => self.render(el),
            WindowEvent::ActivationTokenDone { serial, token } => {
                if let Some(i) = self.links_awaiting_token.iter().position(|(s, _, _)| *s == serial) {
                    let (_, url, _) = self.links_awaiting_token.remove(i);
                    rustora_ui::links::open_with_activation_token(&url, &token.into_raw());
                }
            }
            _ => {}
        }
        if self.client.ui.requests.iter().any(|r| matches!(r, AppRequest::Quit)) {
            el.exit();
        }
    }

    /// One pass of `Game.Loop`: at most one logic tick and one render. A game renders after each world tick, and a
    /// replay that falls behind renders at least every [`MAX_RENDER_INTERVAL`]. While the window cannot draw,
    /// because it is suspended or its redraws stall, the logic runs on without waiting for frames.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.flush_motion();
        self.open_links();
        let now = Instant::now();
        if now.saturating_duration_since(self.next_logic) > MAX_LOGIC_BEHIND {
            self.next_logic = now;
        }
        let force_render = self.render_before_next_tick || now >= self.forced_next_render;
        if now >= self.next_logic && !self.render_before_next_tick {
            self.next_logic += Duration::from_millis(self.client.logic_interval() as u64);
            self.client.logic_tick();
            if self.client.renders_every_tick() {
                self.render_before_next_tick = true;
            }
        }
        let is_time_to_render = now >= self.next_render;
        let redraw_stalled = self.redraw_requested.is_some_and(|t| now.duration_since(t) > REDRAW_STALL);
        if self.suspended || redraw_stalled {
            if is_time_to_render {
                self.next_render = now + RENDER_INTERVAL;
                self.render_before_next_tick = false;
            }
            if redraw_stalled && now >= self.forced_next_render {
                self.forced_next_render = now + MAX_RENDER_INTERVAL;
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
        } else if (is_time_to_render && now < self.next_logic) || force_render {
            self.next_render = now + RENDER_INTERVAL;
            self.forced_next_render = now + MAX_RENDER_INTERVAL.max(RENDER_INTERVAL);
            match &self.gpu {
                Some(gpu) => {
                    gpu.window.request_redraw();
                    self.redraw_requested.get_or_insert(now);
                }
                None => self.render_before_next_tick = false,
            }
        }
        if self.client.ui.requests.iter().any(|r| matches!(r, AppRequest::Quit)) {
            el.exit();
        }
        el.set_control_flow(ControlFlow::WaitUntil(self.next_logic.min(self.next_render)));
    }
}

/// The pixel doubling of the hardware cursors for the `CursorDouble` setting and the scale of the window system.
fn cursor_scale(cursor_double: bool, window_scale: f64) -> u16 {
    let native = if window_scale > 1.5 { 2 } else { 1 };
    if cursor_double { 2 * native } else { native }
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
