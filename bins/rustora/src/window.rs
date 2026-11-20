// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The client window on the main thread: the winit event loop, the window, the surface, the GPU, the hardware cursors
//! of `Sdl2PlatformWindow` and the links. It forwards the window events to the simulation thread of
//! [`crate::game_loop`] and presents the frames that come back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustora_client::settings::{Settings, WindowMode};
use rustora_render::{GpuBackend, wgpu};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, AsyncRequestSerial, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{CustomCursor, Fullscreen, Window, WindowId};

use crate::frames::{FrameQueue, Presented, Skip, Surface};
use crate::game_loop::{CursorChange, ShownCursor, Start, ToMain, ToSim, Wake};

/// A requested redraw that has not come after this time means that the window system does not let the window draw,
/// like Wayland for a hidden window, which sends no frame callbacks. The frames then only upload until it draws.
const REDRAW_STALL: Duration = Duration::from_millis(100);
/// How often a stalled redraw is requested again.
const STALLED_REDRAW_INTERVAL: Duration = Duration::from_millis(100);
/// How long a link waits for an activation token of the window system before it opens without one.
const ACTIVATION_TOKEN_WAIT: Duration = Duration::from_secs(1);

/// Asks the window system for an activation token, which X11 and Wayland deliver with `ActivationTokenDone`.
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn request_activation_token(window: &Window) -> Option<AsyncRequestSerial> {
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
fn request_activation_token(_window: &Window) -> Option<AsyncRequestSerial> {
    None
}

struct Gpu {
    window: Arc<Window>,
    surface: Surface,
}

/// The channels to the simulation thread and its handle.
struct Sim {
    tx: Sender<ToSim>,
    rx: Receiver<ToMain>,
    thread: JoinHandle<()>,
}

struct App {
    engine: PathBuf,
    support: String,
    args: Vec<String>,
    proxy: EventLoopProxy<Wake>,
    size: (u32, u32),
    fullscreen: Option<Fullscreen>,
    /// The benchmark runs without vertical sync.
    no_vsync: bool,
    gpu: Option<Gpu>,
    sim: Option<Sim>,
    /// The frames that wait for the next redraw.
    queue: FrameQueue,
    suspended: bool,
    /// When the redraw that the window waits for was requested.
    redraw_requested: Option<Instant>,
    /// When a stalled redraw is requested again.
    forced_redraw: Instant,
    /// The time spent in the event handlers since the last report to the simulation thread.
    busy: Duration,
    /// The mod cursor frames that the simulation thread sent.
    hardware_cursors: HashMap<(String, usize), CustomCursor>,
    /// The links that wait for an activation token of the window system, with the time they were requested.
    links_awaiting_token: Vec<(AsyncRequestSerial, String, Instant)>,
}

/// Opens the client window. Without an explicit `size`, the window follows the `Graphics.Mode` setting like
/// OpenRA: fullscreen modes cover the monitor, windowed mode uses `Graphics.WindowedSize`. Runs until the window
/// closes or the client quits.
pub fn run(engine: &Path, support: &str, size: Option<(u32, u32)>, args: &[String]) {
    rustora_core::log_channels::init(Path::new(support), rustora_core::log_channels::CLIENT_CHANNELS);
    rustora_core::log_channels::install_panic_hook(Path::new(support));
    let event_loop = EventLoop::<Wake>::with_user_event().build().expect("event loop");
    let mut launcher = Launcher {
        engine: engine.to_path_buf(),
        support: support.to_string(),
        size,
        args: args.to_vec(),
        proxy: event_loop.create_proxy(),
        app: None,
    };
    event_loop.run_app(&mut launcher).expect("run event loop");
    if let Some(app) = launcher.app.take() {
        app.shut_down();
    }
    log::logger().flush();
}

/// Creates the `App` once the event loop can report the monitor size, then forwards all events to it.
struct Launcher {
    engine: PathBuf,
    support: String,
    size: Option<(u32, u32)>,
    args: Vec<String>,
    proxy: EventLoopProxy<Wake>,
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

impl Launcher {
    /// Runs `handle` on the app and adds its time to the busy time of the main thread.
    fn timed(&mut self, handle: impl FnOnce(&mut App)) {
        if let Some(app) = self.app.as_mut() {
            let start = Instant::now();
            handle(app);
            app.busy += start.elapsed();
        }
    }
}

impl ApplicationHandler<Wake> for Launcher {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.app.is_none() {
            let (size, fullscreen) = self.window_layout(el);
            self.app = Some(App {
                engine: self.engine.clone(),
                support: self.support.clone(),
                args: self.args.clone(),
                proxy: self.proxy.clone(),
                size,
                fullscreen,
                no_vsync: self.args.iter().any(|a| a.starts_with("Launch.BenchTicks=")),
                gpu: None,
                sim: None,
                queue: FrameQueue::default(),
                suspended: false,
                redraw_requested: None,
                forced_redraw: Instant::now(),
                busy: Duration::ZERO,
                hardware_cursors: HashMap::new(),
                links_awaiting_token: Vec::new(),
            });
        }
        self.timed(|app| app.resumed(el));
    }

    fn user_event(&mut self, el: &ActiveEventLoop, _event: Wake) {
        self.timed(|app| app.receive(el));
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.timed(|app| app.window_event(el, id, event));
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.timed(|app| app.about_to_wait(el));
    }
}

impl App {
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
        if self.no_vsync {
            config.present_mode = wgpu::PresentMode::AutoNoVsync;
        }
        surface.configure(&device, &config);
        let backend = GpuBackend::new(device, queue, config.format);
        Gpu {
            window,
            surface: Surface { surface, config, backend },
        }
    }

    /// Starts the simulation thread once the window exists.
    fn start_sim(&mut self) {
        let Some(gpu) = &self.gpu else { return };
        let (tx, sim_rx) = channel();
        let (sim_tx, rx) = channel();
        let start = Start {
            engine: self.engine.clone(),
            support: self.support.clone(),
            size: self.size,
            window_size: (gpu.surface.config.width, gpu.surface.config.height),
            scale_factor: gpu.window.scale_factor(),
            args: self.args.clone(),
            queue: self.queue.clone(),
        };
        let thread = crate::game_loop::spawn(start, sim_rx, sim_tx, self.proxy.clone());
        self.sim = Some(Sim { tx, rx, thread });
    }

    fn send(&self, message: ToSim) {
        if let Some(sim) = &self.sim {
            let _ = sim.tx.send(message);
        }
    }

    /// Reports a frame to the simulation thread, with the busy time of the main thread since the last report.
    fn report(&mut self, mut report: Presented) {
        report.main_busy = std::mem::take(&mut self.busy);
        self.send(ToSim::Presented(report));
    }

    /// Takes the messages of the simulation thread.
    fn receive(&mut self, el: &ActiveEventLoop) {
        let Some(sim) = &self.sim else { return };
        let messages: Vec<ToMain> = sim.rx.try_iter().collect();
        for message in messages {
            match message {
                ToMain::Cursor(change) => self.change_cursor(el, change),
                ToMain::OpenUrl(url) => self.open_link(url),
                ToMain::Quit => el.exit(),
            }
        }
        if self.queue.is_empty() {
            return;
        }
        match &self.gpu {
            Some(gpu) if !self.suspended => {
                gpu.window.request_redraw();
                self.redraw_requested.get_or_insert_with(Instant::now);
            }
            _ => self.skip_frames(Skip::Hidden),
        }
    }

    fn change_cursor(&mut self, el: &ActiveEventLoop, change: CursorChange) {
        let Some(gpu) = &self.gpu else { return };
        match change {
            CursorChange::Reset => self.hardware_cursors.clear(),
            CursorChange::Image { key, image } => {
                let source = CustomCursor::from_rgba(image.rgba, image.width, image.height, image.hotspot.0, image.hotspot.1)
                    .expect("valid cursor image");
                self.hardware_cursors.insert(key, el.create_custom_cursor(source));
            }
            CursorChange::Show(ShownCursor::Hidden) => gpu.window.set_cursor_visible(false),
            CursorChange::Show(ShownCursor::Link) => {
                gpu.window.set_cursor(winit::window::CursorIcon::Pointer);
                gpu.window.set_cursor_visible(true);
            }
            CursorChange::Show(ShownCursor::Mod(name, frame)) => {
                if let Some(cursor) = self.hardware_cursors.get(&(name, frame)) {
                    gpu.window.set_cursor(cursor.clone());
                    gpu.window.set_cursor_visible(true);
                }
            }
        }
    }

    /// Opens a link. Where the window system hands out activation tokens, the link waits for one, so that the browser
    /// may come to the front; without a token after [`ACTIVATION_TOKEN_WAIT`], it opens anyway.
    fn open_link(&mut self, url: String) {
        match self.gpu.as_ref().and_then(|gpu| request_activation_token(&gpu.window)) {
            Some(serial) => self.links_awaiting_token.push((serial, url, Instant::now())),
            None => rustora_ui::links::open(&url),
        }
    }

    /// Opens the links that waited for an activation token for [`ACTIVATION_TOKEN_WAIT`] without one.
    fn open_overdue_links(&mut self, now: Instant) {
        self.links_awaiting_token.retain(|(_, url, requested)| {
            let waiting = now.duration_since(*requested) < ACTIVATION_TOKEN_WAIT;
            if !waiting {
                rustora_ui::links::open(url);
            }
            waiting
        });
    }

    /// Runs only the uploads of all waiting frames and reports them to the simulation thread as skipped for `reason`.
    fn skip_frames(&mut self, reason: Skip) {
        while let Some(frame) = self.queue.pop() {
            let Some(gpu) = self.gpu.as_mut() else { return };
            let report = gpu.surface.skip(frame, reason);
            self.report(report);
        }
    }

    /// Takes the oldest waiting frame out of the queue and draws it. A frame without draws only uploads, and the next
    /// one is taken. The simulation thread learns at once that the queue has room, and the next redraw takes the next
    /// frame.
    fn render(&mut self) {
        self.redraw_requested = None;
        let frame = loop {
            let Some(frame) = self.queue.pop() else { return };
            self.send(ToSim::FrameTaken);
            let Some(gpu) = self.gpu.as_mut() else { return };
            if frame.commands.has_draws() {
                break frame;
            }
            let report = gpu.surface.skip(frame, Skip::NoDraws);
            self.report(report);
        };
        let Some(gpu) = self.gpu.as_mut() else { return };
        let window = gpu.window.clone();
        let report = gpu.surface.present(frame, || window.pre_present_notify());
        if !self.queue.is_empty() {
            window.request_redraw();
            self.redraw_requested = Some(Instant::now());
        }
        self.report(report);
    }

    /// Tells the simulation thread to quit and waits for it. A panic of the simulation thread goes on here.
    fn shut_down(mut self) {
        let Some(sim) = self.sim.take() else { return };
        let _ = sim.tx.send(ToSim::Quit);
        drop(sim.rx);
        if let Err(panic) = sim.thread.join() {
            std::panic::resume_unwind(panic);
        }
    }
}

impl App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gpu.is_none() {
            let mut gpu = self.create_gpu(el);
            let window = gpu.window.clone();
            gpu.surface.present_black(|| window.pre_present_notify());
            self.gpu = Some(gpu);
            self.start_sim();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    let s = &mut gpu.surface;
                    s.config.width = size.width.max(1);
                    s.config.height = size.height.max(1);
                    s.surface.configure(&s.backend.device, &s.config);
                    let size = (s.config.width, s.config.height);
                    self.send(ToSim::Resized(size));
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.suspended = occluded;
                self.send(ToSim::Occluded(occluded));
            }
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => self.send(ToSim::ScaleFactor(scale_factor)),
            WindowEvent::ActivationTokenDone { serial, token } => {
                if let Some(i) = self.links_awaiting_token.iter().position(|(s, _, _)| *s == serial) {
                    let (_, url, _) = self.links_awaiting_token.remove(i);
                    rustora_ui::links::open_with_activation_token(&url, &token.into_raw());
                }
            }
            WindowEvent::ModifiersChanged(_)
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::KeyboardInput { .. }
            | WindowEvent::Focused(_) => self.send(ToSim::Window(event)),
            _ => {}
        }
    }

    /// While the window is hidden or its redraws stall, the waiting frames only upload, so that the simulation
    /// thread runs on; a stalled redraw is requested again every [`STALLED_REDRAW_INTERVAL`]. Links that waited too long
    /// for an activation token open.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        self.open_overdue_links(now);
        let redraw_stalled = self.redraw_requested.is_some_and(|t| now.duration_since(t) > REDRAW_STALL);
        if self.suspended {
            self.skip_frames(Skip::Hidden);
        } else if redraw_stalled {
            self.skip_frames(Skip::Stalled);
        }
        if redraw_stalled
            && now >= self.forced_redraw
            && let Some(gpu) = &self.gpu
        {
            self.forced_redraw = now + STALLED_REDRAW_INTERVAL;
            gpu.window.request_redraw();
        }
        let redraw_wake = match self.redraw_requested {
            Some(t) if !redraw_stalled => Some(t + REDRAW_STALL),
            Some(_) => Some(self.forced_redraw),
            None => None,
        };
        let link_wake = self.links_awaiting_token.iter().map(|(_, _, t)| *t + ACTIVATION_TOKEN_WAIT).min();
        el.set_control_flow(match redraw_wake.into_iter().chain(link_wake).min() {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}
