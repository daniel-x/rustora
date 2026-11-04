// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The menu client: loads the chrome of a mod and shows its user interface.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use rustora_fs::{Platform, ReadFs};
use rustora_game::moddata::ModData;
use rustora_render::Renderer;
use rustora_render::ui::UiList;
use rustora_ui::assets::Assets;
use rustora_ui::cursor::{CursorState, Cursors};
use rustora_ui::draw::DrawContext;
use rustora_ui::input::{MouseButton, MouseEvent, MouseInput};
use rustora_ui::loader::WidgetLoader;
use rustora_ui::logic::Services;
use rustora_ui::widget::{Ui, WidgetArgs};

struct ModFs(Rc<ModData>);

impl ReadFs for ModFs {
    fn try_open(&self, filename: &str) -> Option<Vec<u8>> {
        self.0.file_system.try_open(filename)
    }

    fn exists(&self, filename: &str) -> bool {
        self.0.file_system.exists(filename)
    }
}

/// A loaded user interface with the shell root open.
pub struct Client {
    pub ui: Ui,
    pub cursors: Cursors,
    pub cursor: CursorState,
    list: UiList,
}

impl Client {
    pub fn new(engine: &Path, support: &str, resolution: (i32, i32)) -> Client {
        let platform = Platform::new(engine.to_path_buf(), support);
        let mod_data = Rc::new(ModData::load(platform, "ra").expect("load mod"));
        let fs: Rc<dyn ReadFs> = Rc::new(ModFs(mod_data.clone()));
        let assets = Assets::load(&mod_data, fs.clone()).expect("load chrome");
        let cursors = Cursors::load(&mod_data, &*fs).expect("load cursors");
        let loader = WidgetLoader::new(&mod_data.manifest.chrome_layout, &*fs).expect("load chrome layouts");
        let services = Services {
            mod_version: mod_data.manifest.metadata.version.clone(),
        };
        let mut ui = Ui::new(Rc::new(RefCell::new(assets)), Rc::new(loader), resolution, Rc::new(services));
        ui.open_window("MAINMENU", WidgetArgs::default());
        Client {
            ui,
            cursors,
            cursor: CursorState::default(),
            list: UiList::default(),
        }
    }

    /// Port of the user interface part of `Game.InnerLogicTick`.
    pub fn logic_tick(&mut self) {
        self.cursor.tick(&self.cursors);
        self.ui.tick();
    }

    /// Port of the cursor selection in `Game.RenderTick`.
    pub fn update_cursor(&mut self, mouse: (i32, i32)) {
        let name = self.ui.cursor_at(self.ui.root, mouse.0, mouse.1);
        self.cursor.set(&self.cursors, Some(name.as_deref().unwrap_or("default")));
    }

    /// Fills the draw list of the current frame.
    pub fn prepare(&mut self, renderer: &mut Renderer) {
        self.list.clear();
        let assets = self.ui.assets.clone();
        let mut dc = DrawContext::new(renderer, &mut self.list, assets.borrow_mut());
        self.ui.draw(&mut dc);
        drop(dc);
        self.ui.after_draw();
    }

    pub fn render(&mut self, renderer: &mut Renderer, target: &rustora_render::wgpu::TextureView, size: (u32, u32)) {
        renderer.render(
            target,
            rustora_render::Camera {
                x: 0.0,
                y: 0.0,
                zoom: 1.0,
                width: size.0,
                height: size.1,
            },
            &[],
        );
        renderer.render_ui(target, size, &self.list);
    }

    /// Sends a left click at a screen position, as the window would.
    pub fn click(&mut self, x: i32, y: i32) {
        let event = |event, button| MouseInput {
            event,
            button,
            x,
            y,
            scroll_delta: 0,
            modifiers: Default::default(),
            multi_tap: 1,
        };
        self.ui.handle_input(&event(MouseEvent::Move, MouseButton::None));
        self.ui.handle_input(&event(MouseEvent::Down, MouseButton::Left));
        self.ui.handle_input(&event(MouseEvent::Up, MouseButton::Left));
        self.ui.tick();
    }
}

/// Renders the user interface offscreen after the given clicks and writes it as a lossless WebP file.
pub fn screenshot(engine: &Path, support: &str, size: (u32, u32), clicks: &[(i32, i32)], out: &str) {
    let (device, queue) = rustora_render::headless_device().expect("no graphics adapter");
    let mut renderer = Renderer::new(device, queue, rustora_render::wgpu::TextureFormat::Rgba8Unorm);
    let mut client = Client::new(engine, support, (size.0 as i32, size.1 as i32));
    client.ui.tick();
    for &(x, y) in clicks {
        client.click(x, y);
    }
    client.prepare(&mut renderer);
    let rgba = rustora_render::render_offscreen(&mut renderer, size.0, size.1, |renderer, view| {
        client.render(renderer, view, size);
    });
    let file = std::fs::File::create(out).expect("create screenshot");
    image_webp::WebPEncoder::new(std::io::BufWriter::new(file))
        .encode(&rgba, size.0, size.1, image_webp::ColorType::Rgba8)
        .expect("encode webp");
    for (what, count) in client.ui.loader.missing.borrow().iter() {
        eprintln!("not ported: {what} ({count})");
    }
}
