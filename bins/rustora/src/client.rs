// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The menu client: loads the chrome of a mod and shows its user interface.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use rustora_audio::MixerDevice;
use rustora_engine::order_manager::{Connection, ReplayConnection};
use rustora_engine::player::PlayerRef;
use rustora_engine::sequence::ReadFile;
use rustora_engine::sound::{AudioDevice, AudioRules, NullAudioDevice, Sound};
use rustora_engine::world::{World, WorldType};
use rustora_fs::{Package, Platform, ReadFs};
use rustora_game::installer::ModContent;
use rustora_game::map::MapPackage;
use rustora_game::map_cache::MapCache;
use rustora_game::moddata::ModData;
use rustora_game::settings::{Settings, WorldViewport};
use rustora_launch::net_client::NetworkConnection;
use rustora_launch::skirmish::{SkirmishDriver, skirmish_session};
use rustora_launch::{Step, load_world_with_sound, open_map, shellmaps};
use rustora_mods_common::sound::StartGameNotificationInfo;
use rustora_net::game_information::ReplayMetadata;
use rustora_net::replay::Replay;
use rustora_net::session::Session;
use rustora_render::ui::UiList;
use rustora_render::{Camera, Quad, Renderer};
use rustora_ui::assets::Assets;
use rustora_ui::cursor::{CursorState, Cursors};
use rustora_ui::draw::{DrawContext, Rect};
use rustora_ui::fluent::Messages;
use rustora_ui::input::{MouseButton, MouseEvent, MouseInput};
use rustora_ui::loader::WidgetLoader;
use rustora_ui::logic::Services;
use rustora_ui::logic::ingame::Ingame;
use rustora_ui::logic::installer::InstallerContext;
use rustora_ui::logic::replay_browser::prompt_confirm_replay_compatibility;
use rustora_ui::music::MusicPlaylist;
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
    game_over_seen: bool,
}

/// `Ui.Timestep`.
pub const UI_TIMESTEP: i32 = 40;
/// `Game.TimestepJankThreshold`.
const TIMESTEP_JANK_THRESHOLD: i64 = 250;

/// `TickTime`: the time of the last tick of a clock whose timestep may change between ticks.
struct TickTime {
    value: i64,
}

impl TickTime {
    fn should_advance(&self, tick: i64, timestep: i32) -> bool {
        timestep != 0 && tick - self.value >= timestep as i64
    }

    fn advance(&mut self, tick: i64, timestep: i32) {
        let step = timestep as i64;
        let integral = (tick - self.value) / step * step;
        self.value += if integral >= TIMESTEP_JANK_THRESHOLD { integral } else { step };
    }
}

/// `WorldRenderer.ProjectedPosition` for a flat map.
fn px_to_world(x: i32, y: i32) -> rustora_core::world_types::WPos {
    rustora_core::world_types::WPos::new(
        x * 1024 / crate::scene::TILE_SIZE as i32,
        y * 1024 / crate::scene::TILE_SIZE as i32,
        0,
    )
}

/// `Game.Sound` with the device and volumes of `settings`. `RUSTORA_AUDIO_WAV` writes the output to a file.
fn open_sound(mod_data: &Rc<ModData>, settings: &Settings) -> Rc<Sound> {
    let device: Box<dyn AudioDevice> = match std::env::var_os("RUSTORA_AUDIO_WAV") {
        Some(path) => match MixerDevice::open_file(Path::new(&path)) {
            Ok(d) => Box::new(d),
            Err(e) => {
                log::warn!("Cannot write the audio output file: {e}");
                Box::new(NullAudioDevice)
            }
        },
        None => match MixerDevice::open() {
            Some(d) => Box::new(d),
            None => Box::new(NullAudioDevice),
        },
    };
    let fs = mod_data.clone();
    let read_file: rustora_engine::sequence::ReadFile = Rc::new(move |name: &str| fs.file_system.try_open(name));
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as i32;
    let sound = Sound::new(device, read_file, seed);
    sound.set_sound_volume(settings.sound.sound_volume);
    sound.set_music_volume(settings.sound.music_volume);
    sound.set_video_volume(settings.sound.video_volume);
    sound.set_repeat(settings.sound.repeat);
    if settings.sound.mute {
        sound.mute_audio();
    }
    Rc::new(sound)
}

/// The `Voices`, `Notifications` and `Music` of `ModRules`.
fn load_mod_audio(mod_data: &ModData) -> Rc<AudioRules> {
    let load = |files: &[String]| ModData::load_yaml(&mod_data.file_system, files, None).unwrap_or_default();
    Rc::new(AudioRules::load(
        &load(&mod_data.manifest.voices),
        &load(&mod_data.manifest.notifications),
        &load(&mod_data.manifest.music),
    ))
}

/// `IPostWorldLoaded.PostWorldLoaded` of `MusicPlaylist`.
fn start_music(services: &Rc<Services>, shell: Option<&ShellWorld>) {
    let Some(shell) = shell else { return };
    services.sound.load_music_files(shell.world.rules().audio());
    let music = MusicPlaylist::new(&shell.world, services.sound.clone(), services.settings.clone());
    if let Some(music) = &music {
        music.post_world_loaded();
    }
    *services.music.borrow_mut() = music;
    if let Some(info) = shell.world.world_actor().info.trait_info::<StartGameNotificationInfo>() {
        let faction = shell.viewer.as_ref().map(|p| p.faction.as_str());
        services
            .sound
            .play_notification(shell.world.rules().audio(), "Speech", &info.notification, faction);
        shell.world.sound.add_transient_line(None, info.text_notification.as_deref());
    }
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
        let (world, _, read_file) = load_world_with_sound(engine, support, package, lobby, false, WorldType::Regular, true);
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
            game_over_seen: false,
        }
    }

    /// `Game.JoinReplay` and `Game.StartGame` for a replay: the world plays the recorded orders.
    fn start_replay(
        engine: &Path,
        support: &str,
        package: Arc<dyn Package>,
        lobby: Session,
        replay: Replay,
        view: (i32, i32),
        distance: WorldViewport,
    ) -> ShellWorld {
        let (world, _, read_file) = load_world_with_sound(engine, support, package, lobby, true, WorldType::Regular, true);
        let connection = ReplayConnection::new(replay.chunks, world.order_latency);
        let driver = SkirmishDriver::with_connection(&world, Box::new(connection), false);
        let viewport = world_viewport(&world, view, distance);
        ShellWorld {
            world,
            driver,
            read_file,
            scene: None,
            quads: Vec::new(),
            viewport,
            viewer: None,
            ingame: None,
            ended: false,
            game_over_seen: false,
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
        let (world, _, read_file) = load_world_with_sound(engine, support, package, lobby, false, WorldType::Shellmap, true);
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
            game_over_seen: false,
        })
    }

    /// `OrderManager.SuggestedTimestep`.
    fn suggested_timestep(&self) -> i32 {
        let world = &self.world;
        if world.is_replay() {
            world.replay_timestep.get()
        } else if self.driver.orders.tick_scale != 1.0 {
            ((self.driver.orders.tick_scale * world.timestep as f32) as i32).max(1)
        } else {
            world.timestep
        }
    }

    fn tick(&mut self) {
        if !self.ended && self.driver.step() == Step::Ended {
            self.ended = true;
        }
        if self.world.is_replay() && self.driver.orders.out_of_sync.is_some() {
            self.world.replay_timestep.set(0);
            if let Some(replay) = self.ingame.as_ref().and_then(|i| i.replay.as_ref()) {
                replay.out_of_sync.set(true);
            }
        }
        if let Some(ingame) = &self.ingame {
            ingame.selection.tick(ingame.render_player().as_ref());
            if let Some(groups) = rustora_mods_common::selection::ControlGroups::of(&ingame.world) {
                groups.tick();
            }
        }
    }

    /// Feeds the user interface with the orders that the world ignores, and sends the chat lines it wrote.
    fn exchange_client_orders(&mut self, messages: &Messages, net: &rustora_ui::net::Net, run_time: i64) {
        let orders = &mut self.driver.orders;
        orders.collect_client_orders = true;
        let mut state = net.borrow_mut();
        for (client_id, order) in std::mem::take(&mut orders.client_orders) {
            if let Err(e) = crate::net::process_client_order(messages, &mut state, client_id, &order, run_time) {
                log::warn!("{e}");
            }
        }
        for order in std::mem::take(&mut state.outgoing) {
            orders.issue_order(order);
        }
        if !orders.connection.is_connected() {
            state.connection_state = rustora_ui::net::ConnectionState::NotConnected;
        }
    }

    /// Hands the sound calls of the last tick to the audio system, and what else `Game.InnerLogicTick` tells it.
    fn update_audio(&mut self, services: &Services, messages: &Messages) {
        let local_player = self.ingame.as_ref().and_then(|i| i.local_player.clone());
        let selection = self.ingame.as_ref().map(|i| i.selection.clone());
        let is_selected = |a: &rustora_engine::actor::ActorRef| selection.as_ref().is_some_and(|s| s.contains(a));
        services
            .sound
            .process_events(self.world.rules().audio(), &self.world, local_player.as_ref(), &is_selected);
        for (player, key) in self.world.sound.take_transient_lines() {
            let for_local_player = player.is_none_or(|p| local_player.as_ref().is_some_and(|l| Rc::ptr_eq(&p, l)));
            if for_local_player {
                services.net.borrow_mut().notifications.push(rustora_ui::net::TextNotification {
                    pool: rustora_ui::net::TextNotificationPool::Transients,
                    prefix: None,
                    text: messages.get(&key),
                    prefix_color: None,
                    text_color: None,
                    time: (0, 0),
                });
            }
        }
        for (key, player) in self.world.sound.take_system_lines() {
            let text = messages.get_with(&key, &[("player", rustora_ui::fluent::MessageArg::Text(&player))]);
            services.net.borrow_mut().notifications.push(rustora_ui::net::TextNotification {
                pool: rustora_ui::net::TextNotificationPool::System,
                prefix: Some("Battlefield Control".to_string()),
                text,
                prefix_color: None,
                text_color: None,
                time: (0, 0),
            });
        }
        let (x, y) = self.viewport.borrow().center;
        services.sound.set_listener_position(px_to_world(x, y));
        if !self.game_over_seen && self.world.is_game_over() {
            self.game_over_seen = true;
            if let Some(music) = services.music.borrow().as_ref() {
                music.game_over(local_player.as_ref());
            }
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
        let overlay = self.ingame.as_ref().map_or_else(Vec::new, |i| i.order_generator_renderables());
        let viewer = self.ingame.as_ref().map_or_else(|| self.viewer.clone(), |i| i.render_player());
        self.quads = scene.build(renderer, world, viewer.as_ref(), &cam, &overlay);
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
    ui_tick_time: TickTime,
    world_tick_time: TickTime,
}

impl Client {
    pub fn new(engine: &Path, support: &str, resolution: (i32, i32), args: &[String]) -> Client {
        Client::initialize_mod(engine, support, resolution, args, "ra")
    }

    pub fn mod_id(&self) -> &str {
        &self.mod_data.manifest.id
    }

    /// Port of `Game.InitializeMod`: a mod with missing content opens its content installer mod instead.
    fn initialize_mod(engine: &Path, support: &str, resolution: (i32, i32), args: &[String], mod_id: &str) -> Client {
        let settings = Settings::load(Path::new(support).join("settings.yaml"), args);
        let platform = Platform::new(engine.to_path_buf(), support);
        let mut mod_data = ModData::load(platform.clone(), mod_id).expect("load mod");
        let installer_mod = mod_data.content_installer_mod();
        if !mod_data.content_available
            && let Some(installer) = &installer_mod
        {
            mod_data = ModData::load(platform.clone(), installer).expect("load content installer mod");
        }
        let content = mod_data
            .content_installer_mod()
            .is_none()
            .then(|| ModContent::load(&mod_data).expect("load mod content"));
        let mod_data = Rc::new(mod_data);
        let fs: Rc<dyn ReadFs> = Rc::new(ModFs(mod_data.clone()));
        let mut assets = Assets::load(&mod_data, fs.clone()).expect("load chrome");
        assets.hotkeys.apply_settings(&settings.keys);
        let cursors = Cursors::load(&mod_data, &*fs).expect("load cursors");
        let loader = WidgetLoader::new(&mod_data.manifest.chrome_layout, &*fs).expect("load chrome layouts");
        let server_mod = LazyServerMod::new(engine, support);
        let sound = open_sound(&mod_data, &settings);
        let mod_audio = load_mod_audio(&mod_data);
        sound.load_music_files(&mod_audio);
        let services = Services {
            mod_id: mod_data.manifest.id.clone(),
            mod_version: mod_data.manifest.metadata.version.clone(),
            installed_mods: mod_data.installed.clone(),
            replay_dir: Path::new(support)
                .join("Replays")
                .join(&mod_data.manifest.id)
                .join(&mod_data.manifest.metadata.version),
            content_installer_mod: installer_mod,
            maps: Rc::new(RefCell::new(MapCache::load(&mod_data))),
            settings: Rc::new(RefCell::new(settings)),
            net: Rc::default(),
            lobby_rules: RefCell::new(Some(lobby_rules_lookup(server_mod.clone()))),
            color_rules: RefCell::new(Some(color_rules_lookup(server_mod))),
            ingame: RefCell::new(None),
            sound,
            mod_audio,
            music: RefCell::new(None),
        };
        let net = NetClient::new(engine, support, services.net.clone());
        let services = Rc::new(services);
        let pause_shellmap = services.settings.borrow().game.pause_shellmap;
        let ui_scroll_speed = services.settings.borrow().game.ui_scroll_speed;
        let distance = services.settings.borrow().graphics.viewport_distance;
        let shell = if mod_data.content_available && content.is_none() {
            ShellWorld::load(engine, support, resolution, distance)
        } else {
            None
        };
        if let Some(shell) = &shell {
            shell.world.pause_shellmap.set(pause_shellmap);
        }
        start_music(&services, shell.as_ref());
        let mut ui = Ui::new(Rc::new(RefCell::new(assets)), Rc::new(loader), resolution, services.clone());
        ui.ui_scroll_speed = ui_scroll_speed;
        ui.cursor_names = Rc::new(cursors.names());
        let mut menu_args = WidgetArgs::default();
        if let Some(target) = args.iter().find_map(|a| a.strip_prefix("Launch.Connect=")) {
            menu_args = menu_args.with_value("directConnect", target.to_string());
        }
        match content {
            Some(content) => {
                let context = Rc::new(InstallerContext {
                    content: Arc::new(content),
                    platform,
                });
                let args = WidgetArgs::default().with_value("content", context);
                ui.load_widget("MODCONTENT_BACKGROUND", Some(ui.root), &args);
            }
            None => {
                ui.load_widget("MAINMENU", Some(ui.root), &menu_args);
                if let Some(path) = args.iter().find_map(|a| a.strip_prefix("Launch.Replay=")) {
                    let meta = ReplayMetadata::read(Path::new(path));
                    if prompt_confirm_replay_compatibility(&mut ui, meta.as_ref(), Rc::new(|_| {})) {
                        ui.requests.push(AppRequest::JoinReplay(path.to_string()));
                    }
                }
            }
        }
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
            ui_tick_time: TickTime {
                value: -(UI_TIMESTEP as i64),
            },
            world_tick_time: TickTime {
                value: -(UI_TIMESTEP as i64),
            },
        }
    }

    /// The timestep of the world clock: `OrderManager.SuggestedTimestep`, or `Ui.Timestep` without a world.
    fn world_timestep(&self) -> i32 {
        self.shell.as_ref().map_or(UI_TIMESTEP, ShellWorld::suggested_timestep)
    }

    /// The interval of `Game.Loop` between two calls of [`Client::logic_tick`] in milliseconds.
    pub fn logic_interval(&self) -> i32 {
        match self.world_timestep() {
            0 => UI_TIMESTEP,
            t => t,
        }
    }

    /// True while a world other than a replay runs: `Game.Loop` renders at least once per logic tick then.
    pub fn renders_every_tick(&self) -> bool {
        self.shell.as_ref().is_some_and(|s| !s.world.is_replay())
    }

    /// Port of `Game.InnerLogicTick`: the user interface ticks at `Ui.Timestep`, the world at its own timestep.
    pub fn logic_tick(&mut self) {
        let now = self.ui.run_time();
        let ui_due = self.ui_tick_time.should_advance(now, UI_TIMESTEP);
        if ui_due {
            self.ui_tick_time.advance(now, UI_TIMESTEP);
            self.pump_network();
        }
        let timestep = self.world_timestep();
        if self.world_tick_time.should_advance(now, timestep) {
            self.world_tick_time.advance(now, timestep);
            if let Some(shell) = &mut self.shell {
                shell.tick();
                if shell.ingame.is_some() {
                    let assets = self.ui.assets.clone();
                    shell.exchange_client_orders(&assets.borrow().messages, &self.services.net, now);
                }
                let assets = self.ui.assets.clone();
                shell.update_audio(&self.services, &assets.borrow().messages);
            }
            self.services.sound.tick();
        }
        if ui_due {
            self.cursor.tick(&self.cursors);
            self.ui.tick();
        }
    }

    /// Carries out the network requests of the user interface, then receives and sends.
    fn pump_network(&mut self) {
        let requests = std::mem::take(&mut self.ui.requests);
        let mut leave = false;
        let mut reload = None;
        let mut replay = None;
        for request in requests {
            match request {
                AppRequest::LeaveGame => leave = true,
                AppRequest::CreateLocalServer { map, skirmish } => self.net.create_local_server(map, skirmish),
                AppRequest::CreateServer {
                    name,
                    port,
                    password,
                    advertise_online,
                    map,
                } => self.net.create_server(name, port, password, advertise_online, map),
                AppRequest::JoinServer { target, password } => self.net.join(&target, &password),
                AppRequest::JoinReplay(path) => replay = Some(path),
                AppRequest::Disconnect => self.net.disconnect(),
                AppRequest::Quit => self.ui.requests.push(AppRequest::Quit),
                AppRequest::InitializeMod(mod_id) => reload = Some(mod_id),
            }
        }
        if leave {
            self.leave_game();
        }
        if let Some(mod_id) = reload {
            self.replace_mod(&mod_id);
            return;
        }
        if let Some(path) = replay {
            self.join_replay(&path);
            return;
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

    /// `IngameMenuLogic.OnQuit` without the exit delay and fade: `Game.Disconnect`, `Ui.ResetAll` and
    /// `Game.LoadShellMap`.
    fn leave_game(&mut self) {
        self.net.disconnect();
        self.ui.reset_all();
        *self.services.ingame.borrow_mut() = None;
        self.dispose_world();
        let (distance, pause_shellmap) = {
            let settings = self.services.settings.borrow();
            (settings.graphics.viewport_distance, settings.game.pause_shellmap)
        };
        self.shell = if self.mod_data.content_available {
            ShellWorld::load(&self.engine, &self.support, self.ui.resolution, distance)
        } else {
            None
        };
        if let Some(shell) = &self.shell {
            shell.world.pause_shellmap.set(pause_shellmap);
        }
        start_music(&self.services, self.shell.as_ref());
        self.ui.load_widget("MAINMENU", Some(self.ui.root), &WidgetArgs::default());
    }

    /// The rest of `Game.InitializeMod`: drops everything of the running mod and starts `mod_id`.
    fn replace_mod(&mut self, mod_id: &str) {
        self.net.disconnect();
        self.ui.reset_all();
        *self.services.ingame.borrow_mut() = None;
        self.dispose_world();
        let clipboard = self.ui.clipboard.clone();
        let (engine, support, resolution) = (self.engine.clone(), self.support.clone(), self.ui.resolution);
        *self = Client::initialize_mod(&engine, &support, resolution, &[], mod_id);
        self.ui.clipboard = clipboard;
    }

    /// `World.Dispose`: the audio of the old world ends with it.
    fn dispose_world(&mut self) {
        if let Some(music) = self.services.music.borrow_mut().take() {
            music.dispose();
        }
        self.services.sound.stop_audio();
        self.shell = None;
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
        self.dispose_world();
        let distance = self.services.settings.borrow().graphics.viewport_distance;
        let shell = ShellWorld::start_game(
            &self.engine,
            &self.support,
            package,
            session,
            connection,
            self.ui.resolution,
            distance,
        );
        self.enter_game(shell, None);
    }

    /// `Game.JoinReplay`: the session of the replay replaces the lobby, and the world starts at once.
    fn join_replay(&mut self, path: &str) {
        let replay = std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|data| Replay::parse(&data).map_err(|e| e.to_string()));
        let mut replay = match replay {
            Ok(r) => r,
            Err(e) => {
                log::error!("cannot read the replay {path}: {e}");
                return;
            }
        };
        let Some(lobby) = replay.lobby_info.take() else {
            log::error!("the replay {path} has no lobby information");
            return;
        };
        let package = lobby
            .global_settings
            .map
            .as_deref()
            .and_then(|uid| self.services.maps.borrow().get(uid))
            .and_then(|map| map.open_package(&self.mod_data.file_system));
        let Some(package) = package else {
            log::error!("The replay cannot start without its map");
            return;
        };
        self.net.disconnect();
        {
            let mut net = self.services.net.borrow_mut();
            net.session = lobby.clone();
            net.local_client_id = -1;
            net.revision += 1;
        }
        let handler = self.services.net.borrow_mut().before_game_start.take();
        if let Some(handler) = handler {
            handler(&mut self.ui);
        }
        self.ui.reset_all();
        self.dispose_world();
        let tick_count = replay.tick_count;
        let final_game_tick = replay
            .metadata
            .as_deref()
            .and_then(|m| rustora_net::game_information::GameInformation::deserialize(m, path).ok())
            .map_or(0, |info| info.root.final_game_tick);
        let distance = self.services.settings.borrow().graphics.viewport_distance;
        let shell = ShellWorld::start_replay(&self.engine, &self.support, package, lobby, replay, self.ui.resolution, distance);
        let status = rustora_ui::logic::ingame::ReplayStatus {
            tick_count,
            final_game_tick,
            out_of_sync: std::cell::Cell::new(false),
        };
        self.enter_game(shell, Some(status));
    }

    /// The part of `Game.StartGame` after the world loaded: the in-game user interface.
    fn enter_game(&mut self, mut shell: ShellWorld, replay: Option<rustora_ui::logic::ingame::ReplayStatus>) {
        let (select_cursor, default_cursor) = {
            let assets = self.ui.assets.borrow();
            (
                assets.metrics.string("WorldSelectCursor"),
                assets.metrics.string("WorldDefaultCursor"),
            )
        };
        let mut ingame = Ingame::new(
            shell.world.clone(),
            shell.viewport.clone(),
            shell.viewer.clone(),
            Rc::new(crate::ui_sprites::UiSprites::new(&shell.world, shell.read_file.clone())),
            select_cursor,
            default_cursor,
        );
        ingame.replay = replay;
        let ingame = Rc::new(ingame);
        shell.ingame = Some(ingame.clone());
        *self.services.ingame.borrow_mut() = Some(ingame);
        self.shell = Some(shell);
        start_music(&self.services, self.shell.as_ref());
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
            rustora_ui::annotations::draw_selection_annotations(&mut dc, ingame, &self.services.settings.borrow(), self.ui.modifiers.shift);
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
