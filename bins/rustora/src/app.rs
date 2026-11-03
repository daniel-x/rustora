// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The replay viewer window: winit event handling, camera, playback timing and presentation.

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustora_engine::world::World;
use rustora_launch::{LoadedGame, ReplayDriver, Step};
use rustora_render::{Camera, Renderer, wgpu};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::scene::{Scene, TILE_SIZE};

/// The world pixel at the center of the map bounds.
pub fn map_center(world: &World) -> (f32, f32) {
    let b = world.map().bounds;
    (
        (b.x as f32 + b.width as f32 / 2.0) * TILE_SIZE,
        (b.y as f32 + b.height as f32 / 2.0) * TILE_SIZE,
    )
}

const SCROLL_PX_PER_SECOND: f32 = 900.0;
const SPEEDS: [f32; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    scene: Scene,
}

struct App {
    world: Rc<World>,
    driver: ReplayDriver,
    read_file: rustora_engine::sequence::ReadFile,
    gpu: Option<Gpu>,
    center: (f32, f32),
    zoom: f32,
    held: [bool; 4],
    paused: bool,
    ended: bool,
    speed: usize,
    viewer: Option<usize>,
    last_frame: Instant,
    tick_debt: Duration,
}

pub fn run(game: LoadedGame) {
    let LoadedGame {
        world, replay, read_file, ..
    } = game;
    let driver = ReplayDriver::new(&world, replay);
    let center = map_center(&world);
    let mut app = App {
        world,
        driver,
        read_file,
        gpu: None,
        center,
        zoom: 1.0,
        held: [false; 4],
        paused: false,
        ended: false,
        speed: 2,
        viewer: None,
        last_frame: Instant::now(),
        tick_debt: Duration::ZERO,
    };
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.run_app(&mut app).expect("run event loop");
}

impl App {
    fn title(&self) -> String {
        let view = match self.viewer {
            Some(i) => self.world.players()[i].internal_name.clone(),
            None => "all".into(),
        };
        let state = if self.ended {
            "ended"
        } else if self.paused {
            "paused"
        } else {
            "playing"
        };
        format!(
            "rustora - tick {} - {state} x{} - view {view}",
            self.world.world_tick(),
            SPEEDS[self.speed]
        )
    }

    fn advance(&mut self, dt: Duration) {
        let s = dt.as_secs_f32();
        let d = SCROLL_PX_PER_SECOND * s / self.zoom;
        let [l, r, u, dn] = self.held;
        self.center.0 += d * (r as i32 - l as i32) as f32;
        self.center.1 += d * (dn as i32 - u as i32) as f32;
        if self.paused || self.ended {
            return;
        }
        let timestep = Duration::from_millis(self.world.timestep as u64).div_f32(SPEEDS[self.speed]);
        self.tick_debt = (self.tick_debt + dt).min(timestep * 8);
        while self.tick_debt >= timestep {
            self.tick_debt -= timestep;
            match self.driver.step() {
                Step::Ended => {
                    self.ended = true;
                    break;
                }
                Step::Ticked | Step::Waiting => {}
            }
        }
    }

    fn draw(&mut self) {
        let title = self.title();
        let Some(gpu) = self.gpu.as_mut() else { return };
        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                gpu.surface.configure(&gpu.renderer.device, &gpu.config);
                return;
            }
        };
        let (w, h) = (gpu.config.width, gpu.config.height);
        let cam = Camera {
            x: self.center.0 - w as f32 / 2.0 / self.zoom,
            y: self.center.1 - h as f32 / 2.0 / self.zoom,
            zoom: self.zoom,
            width: w,
            height: h,
        };
        let viewer = self.viewer.map(|i| self.world.players()[i].clone());
        let quads = gpu.scene.build(&mut gpu.renderer, &self.world, viewer.as_ref(), &cam);
        let view = frame.texture.create_view(&Default::default());
        gpu.renderer.render(&view, cam, &quads);
        gpu.renderer.queue.present(frame);
        gpu.window.set_title(&title);
    }

    fn key(&mut self, event: &KeyEvent, el: &ActiveEventLoop) {
        let down = event.state == ElementState::Pressed;
        let dir = match &event.logical_key {
            Key::Named(NamedKey::ArrowLeft) => Some(0),
            Key::Named(NamedKey::ArrowRight) => Some(1),
            Key::Named(NamedKey::ArrowUp) => Some(2),
            Key::Named(NamedKey::ArrowDown) => Some(3),
            Key::Character(c) => match c.as_str() {
                "a" => Some(0),
                "d" => Some(1),
                "w" => Some(2),
                "s" => Some(3),
                _ => None,
            },
            _ => None,
        };
        if let Some(d) = dir {
            self.held[d] = down;
            return;
        }
        if !down || event.repeat {
            return;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => el.exit(),
            Key::Named(NamedKey::Space) => self.paused = !self.paused,
            Key::Named(NamedKey::Tab) => {
                let n = self.world.players().len();
                self.viewer = match self.viewer {
                    None if n > 0 => Some(0),
                    Some(i) if i + 1 < n => Some(i + 1),
                    _ => None,
                };
            }
            Key::Character(c) if c == "+" || c == "=" => self.speed = (self.speed + 1).min(SPEEDS.len() - 1),
            Key::Character(c) if c == "-" => self.speed = self.speed.saturating_sub(1),
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let window = Arc::new(
            el.create_window(
                Window::default_attributes()
                    .with_title("rustora")
                    .with_inner_size(winit::dpi::LogicalSize::new(1280, 800)),
            )
            .expect("create window"),
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
            el.owned_display_handle(),
        )));
        let surface = instance.create_surface(window.clone()).expect("create surface");
        let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("graphics adapter");
        let (device, queue) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("graphics device");
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface config");
        let caps = surface.get_capabilities(&adapter);
        config.format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(config.format);
        surface.configure(&device, &config);
        let mut renderer = Renderer::new(device, queue, config.format);
        let scene = Scene::new(&mut renderer, &self.world, self.read_file.clone());
        self.gpu = Some(Gpu {
            window,
            surface,
            config,
            renderer,
            scene,
        });
        self.last_frame = Instant::now();
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
            WindowEvent::KeyboardInput { event, .. } => self.key(&event, el),
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                self.zoom = (self.zoom * 1.25f32.powf(steps)).clamp(0.25, 4.0);
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = now - self.last_frame;
                self.last_frame = now;
                self.advance(dt);
                self.draw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}

fn pollster_block<F: std::future::Future>(f: F) -> F::Output {
    rustora_render::pollster_block_on(f)
}
