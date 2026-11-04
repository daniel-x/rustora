// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The menu client: loads the chrome of a mod and shows its user interface.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use rustora_engine::order_manager::Connection;
use rustora_engine::player::PlayerRef;
use rustora_engine::sequence::ReadFile;
use rustora_engine::world::{World, WorldType};
use rustora_fs::{Package, Platform, ReadFs};
use rustora_game::map::MapPackage;
use rustora_game::map_cache::MapCache;
use rustora_game::moddata::ModData;
use rustora_game::settings::{Settings, WorldViewport};
use rustora_launch::net_client::NetworkConnection;
use rustora_launch::skirmish::{SkirmishDriver, skirmish_session};
use rustora_launch::{Step, load_world, load_world_of_type, open_map, shellmaps};
use rustora_net::session::Session;
use rustora_render::ui::UiList;
use rustora_render::{Camera, Quad, Renderer};
use rustora_ui::assets::Assets;
use rustora_ui::cursor::{CursorState, Cursors};
use rustora_ui::draw::{DrawContext, Rect};
use rustora_ui::input::{MouseButton, MouseEvent, MouseInput};
use rustora_ui::loader::WidgetLoader;
use rustora_ui::logic::Services;
use rustora_ui::logic::ingame::Ingame;
use rustora_ui::viewport::Viewport;
use rustora_ui::widget::{AppRequest, Ui, WidgetArgs};

use crate::net::{LazyServerMod, NetClient, color_rules_lookup, lobby_rules_lookup};
use crate::scene::{Scene, world_to_px};

struct ModFs(Rc<ModData>);

impl ReadFs for ModFs {
    fn try_open(&self, filename: &str) -> Option<Vec<u8>> {
        self.0.file_system.try_open(filename)
    }

    fn exists(&self, filename: &str) -> bool {
        self.0.file_system.exists(filename)
    }
}

/// The world behind the main menu. Its Lua script, which moves the camera and sends units, waits for P9.
struct ShellWorld {
    world: Rc<World>,
    driver: SkirmishDriver,
    read_file: ReadFile,
    scene: Option<Scene>,
    quads: Vec<Quad>,
    viewport: Rc<RefCell<Viewport>>,
    viewer: Option<PlayerRef>,
    ingame: Option<Rc<Ingame>>,
    ended: bool,
}

/// The viewport of a world, centered on its map.
fn world_viewport(world: &World, resolution: (i32, i32), distance: WorldViewport) -> Rc<RefCell<Viewport>> {
    let map = world.map();
    let (left, top) = world_to_px(map.projected_top_left());
    let (right, bottom) = world_to_px(map.projected_bottom_right());
    let bounds = Rect::new(left as i32, top as i32, (right - left) as i32, (bottom - top) as i32);
    Rc::new(RefCell::new(Viewport::new(bounds, resolution, distance)))
}

impl ShellWorld {
    /// `Game.StartGame`: the world of a game that the server started.
    fn start_game(
        engine: &Path,
        support: &str,
        package: Arc<dyn Package>,
        lobby: Session,
        connection: NetworkConnection,
        view: (i32, i32),
        distance: WorldViewport,
    ) -> ShellWorld {
        let local_client = connection.local_client_id();
        let is_host = lobby.client_with_index(local_client).is_some_and(|c| c.is_admin);
        let (world, _, read_file) = load_world(engine, support, package, lobby, false);
        let driver = SkirmishDriver::with_connection(&world, Box::new(connection), is_host);
        let viewport = world_viewport(&world, view, distance);
        let viewer = world
            .players()
            .into_iter()
            .find(|p| p.client_index == local_client && p.playable && !p.non_combatant);
        if let Some(p) = &viewer {
            let home = p.home_location;
            let center = rustora_core::world_types::WPos::new(1024 * home.x() + 512, 1024 * home.y() + 512, 0);
            viewport.borrow_mut().center_on(crate::scene::screen_px_position(center));
        }
        ShellWorld {
            world,
            driver,
            read_file,
            scene: None,
            quads: Vec::new(),
            viewport,
            viewer,
            ingame: None,
            ended: false,
        }
    }

    /// Port of `Game.LoadShellMap` and `ChooseShellmap`. `None` without a shell map.
    fn load(engine: &Path, support: &str, view: (i32, i32), distance: WorldViewport) -> Option<ShellWorld> {
        let names = shellmaps(engine);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let name = names.get(now.subsec_nanos() as usize % names.len().max(1))?;
        let package = open_map(engine, name)?;
        let map = MapPackage::load(package.clone()).ok()?;
        let lobby = skirmish_session(&map, &[], now.subsec_nanos() as i32).ok()?;
        let (world, _, read_file) = load_world_of_type(engine, support, package, lobby, false, WorldType::Shellmap);
        let driver = SkirmishDriver::new(&world);
        let viewport = world_viewport(&world, view, distance);
        Some(ShellWorld {
            world,
            driver,
            read_file,
            scene: None,
            quads: Vec::new(),
            viewport,
            viewer: None,
            ingame: None,
            ended: false,
        })
    }

    fn tick(&mut self) {
        if !self.ended && self.driver.step() == Step::Ended {
            self.ended = true;
        }
        if let Some(ingame) = &self.ingame {
            ingame.selection.tick(ingame.render_player.as_ref());
        }
    }

    fn camera(&self, size: (u32, u32)) -> Camera {
        let viewport = self.viewport.borrow();
        let (x, y) = viewport.top_left();
        Camera {
            x: x as f32,
            y: y as f32,
            zoom: viewport.zoom(),
            width: size.0,
            height: size.1,
        }
    }

    fn prepare(&mut self, renderer: &mut Renderer, size: (u32, u32)) {
        let cam = self.camera(size);
        let read_file = self.read_file.clone();
        let world = &self.world;
        let scene = self.scene.get_or_insert_with(|| Scene::new(renderer, world, read_file));
        self.quads = scene.build(renderer, world, self.viewer.as_ref(), &cam);
        if let Some(ingame) = &self.ingame {
            ingame.screen_map.borrow_mut().set(scene.mouse_bounds(renderer, world));
        }
    }
}

/// A loaded user interface with the shell root open.
pub struct Client {
    pub ui: Ui,
    pub cursors: Cursors,
    pub cursor: CursorState,
    list: UiList,
    shell: Option<ShellWorld>,
    engine: std::path::PathBuf,
    support: String,
    mod_data: Rc<ModData>,
    services: Rc<Services>,
    net: NetClient,
}

impl Client {
    pub fn new(engine: &Path, support: &str, resolution: (i32, i32), args: &[String]) -> Client {
        let settings = Settings::load(Path::new(support).join("settings.yaml"), args);
        let platform = Platform::new(engine.to_path_buf(), support);
        let mod_data = Rc::new(ModData::load(platform, "ra").expect("load mod"));
        let fs: Rc<dyn ReadFs> = Rc::new(ModFs(mod_data.clone()));
        let mut assets = Assets::load(&mod_data, fs.clone()).expect("load chrome");
        assets.hotkeys.apply_settings(&settings.keys);
        let cursors = Cursors::load(&mod_data, &*fs).expect("load cursors");
        let loader = WidgetLoader::new(&mod_data.manifest.chrome_layout, &*fs).expect("load chrome layouts");
        let server_mod = LazyServerMod::new(engine, support);
        let services = Services {
            mod_version: mod_data.manifest.metadata.version.clone(),
            maps: Rc::new(RefCell::new(MapCache::load(&mod_data))),
            settings: Rc::new(RefCell::new(settings)),
            net: Rc::default(),
            lobby_rules: RefCell::new(Some(lobby_rules_lookup(server_mod.clone()))),
            color_rules: RefCell::new(Some(color_rules_lookup(server_mod))),
            ingame: RefCell::new(None),
        };
        let net = NetClient::new(engine, support, services.net.clone());
        let services = Rc::new(services);
        let pause_shellmap = services.settings.borrow().game.pause_shellmap;
        let ui_scroll_speed = services.settings.borrow().game.ui_scroll_speed;
        let distance = services.settings.borrow().graphics.viewport_distance;
        let shell = if mod_data.content_available {
            ShellWorld::load(engine, support, resolution, distance)
        } else {
            None
        };
        if let Some(shell) = &shell {
            shell.world.pause_shellmap.set(pause_shellmap);
        }
        let mut ui = Ui::new(Rc::new(RefCell::new(assets)), Rc::new(loader), resolution, services.clone());
        ui.ui_scroll_speed = ui_scroll_speed;
        let mut menu_args = WidgetArgs::default();
        if let Some(target) = args.iter().find_map(|a| a.strip_prefix("Launch.Connect=")) {
            menu_args = menu_args.with_value("directConnect", target.to_string());
        }
        ui.load_widget("MAINMENU", Some(ui.root), &menu_args);
        Client {
            ui,
            cursors,
            cursor: CursorState::default(),
            list: UiList::default(),
            shell,
            engine: engine.to_path_buf(),
            support: support.to_string(),
            mod_data,
            services,
            net,
        }
    }

    /// Port of the user interface part of `Game.InnerLogicTick`.
    pub fn logic_tick(&mut self) {
        self.pump_network();
        if let Some(shell) = &mut self.shell {
            shell.tick();
        }
        self.cursor.tick(&self.cursors);
        self.ui.tick();
    }

    /// Carries out the network requests of the user interface, then receives and sends.
    fn pump_network(&mut self) {
        let requests = std::mem::take(&mut self.ui.requests);
        for request in requests {
            match request {
                AppRequest::CreateLocalServer { map, skirmish } => self.net.create_local_server(map, skirmish),
                AppRequest::JoinServer { target, password } => self.net.join(&target, &password),
                AppRequest::Disconnect => self.net.disconnect(),
                AppRequest::Quit => self.ui.requests.push(AppRequest::Quit),
            }
        }
        let (name, color) = {
            let settings = self.services.settings.borrow();
            (settings.player.name.clone(), settings.player.color)
        };
        let run_time = self.ui.run_time();
        let assets = self.ui.assets.clone();
        let started = self.net.pump(&self.mod_data, &assets.borrow().messages, run_time, &name, color);
        if started {
            self.start_game();
        }
    }

    /// `Game.StartGame`. The in-game user interface follows with slice 6 of P8.
    fn start_game(&mut self) {
        let handler = self.services.net.borrow_mut().before_game_start.take();
        if let Some(handler) = handler {
            handler(&mut self.ui);
        }
        let session = self.services.net.borrow().session.clone();
        let package = session
            .global_settings
            .map
            .as_deref()
            .and_then(|uid| self.services.maps.borrow().get(uid))
            .and_then(|map| map.open_package(&self.mod_data.file_system));
        let (Some(package), Some(connection)) = (package, self.net.take_connection()) else {
            log::error!("The game cannot start without its map");
            return;
        };
        self.ui.reset_all();
        self.shell = None;
        let distance = self.services.settings.borrow().graphics.viewport_distance;
        let mut shell = ShellWorld::start_game(
            &self.engine,
            &self.support,
            package,
            session,
            connection,
            self.ui.resolution,
            distance,
        );
        let (select_cursor, default_cursor) = {
            let assets = self.ui.assets.borrow();
            (
                assets.metrics.string("WorldSelectCursor"),
                assets.metrics.string("WorldDefaultCursor"),
            )
        };
        let ingame = Rc::new(Ingame::new(
            shell.world.clone(),
            shell.viewport.clone(),
            shell.viewer.clone(),
            Rc::new(crate::ui_sprites::UiSprites::new(&shell.world, shell.read_file.clone())),
            select_cursor,
            default_cursor,
        ));
        shell.ingame = Some(ingame.clone());
        *self.services.ingame.borrow_mut() = Some(ingame);
        self.shell = Some(shell);
        self.ui.load_widget("INGAME_ROOT", Some(self.ui.root), &WidgetArgs::default());
        for (what, count) in self.ui.loader.missing.borrow().iter() {
            log::info!("not ported: {what} ({count})");
        }
    }

    /// Port of the cursor selection in `Game.RenderTick`.
    pub fn update_cursor(&mut self, mouse: (i32, i32)) {
        let name = self.ui.cursor_at(self.ui.root, mouse.0, mouse.1);
        self.cursor.set(&self.cursors, Some(name.as_deref().unwrap_or("default")));
    }

    /// Fills the draw list of the current frame.
    pub fn prepare(&mut self, renderer: &mut Renderer, size: (u32, u32)) {
        if let Some(shell) = &mut self.shell {
            shell.prepare(renderer, size);
        }
        self.list.clear();
        let assets = self.ui.assets.clone();
        let mut dc = DrawContext::new(renderer, &mut self.list, assets.borrow_mut());
        if let Some(ingame) = self.shell.as_ref().and_then(|s| s.ingame.as_ref()) {
            rustora_ui::annotations::draw_selection_annotations(&mut dc, ingame, &self.services.settings.borrow());
        }
        self.ui.draw(&mut dc);
        drop(dc);
        self.ui.after_draw();
    }

    pub fn render(&mut self, renderer: &mut Renderer, target: &rustora_render::wgpu::TextureView, size: (u32, u32)) {
        match &self.shell {
            Some(shell) => renderer.render(target, shell.camera(size), &shell.quads),
            None => renderer.render(
                target,
                Camera {
                    x: 0.0,
                    y: 0.0,
                    zoom: 1.0,
                    width: size.0,
                    height: size.1,
                },
                &[],
            ),
        }
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
    let mut client = Client::new(engine, support, (size.0 as i32, size.1 as i32), &[]);
    client.logic_tick();
    for &(x, y) in clicks {
        client.click(x, y);
    }
    client.prepare(&mut renderer, size);
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
