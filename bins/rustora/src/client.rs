// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The menu client: loads the chrome of a mod and shows its user interface.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use rustora_audio::MixerDevice;
use rustora_client::game_information::ReplayMetadata;
use rustora_client::installer::ModContent;
use rustora_client::lag_monitor::{LagCause, LagDisplay, LagMonitor, LagReport, wait_causes};
use rustora_client::map_cache::MapCache;
use rustora_client::replay::Replay;
use rustora_client::settings::{Settings, WorldViewport};
use rustora_engine::order_manager::{Connection, ReplayConnection};
use rustora_engine::player::PlayerRef;
use rustora_engine::sequence::ReadFile;
use rustora_engine::sound::{AudioDevice, AudioRules, NullAudioDevice, Sound};
use rustora_engine::world::{World, WorldType};
use rustora_fs::{Package, Platform, ReadFs};
use rustora_game::map::MapPackage;
use rustora_game::moddata::ModData;
use rustora_launch::net_client::NetworkConnection;
use rustora_launch::skirmish::{SkirmishDriver, skirmish_session};
use rustora_launch::{Step, load_world_with_sound, open_map, shellmaps};
use rustora_mods_common::sound::StartGameNotificationInfo;
use rustora_mods_common::world::create_game_information;
use rustora_net::order::RawOrder;
use rustora_net::session::Session;
use rustora_render::ui::UiList;
use rustora_render::{Camera, GpuBackend, Quad, Renderer};
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
use rustora_ui::screen_map::ScreenMap;
use rustora_ui::viewport::Viewport;
use rustora_ui::widget::{AppRequest, Ui, WidgetArgs};

use crate::net::{LobbyData, NetClient, color_rules_lookup, lobby_rules_lookup};
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

/// The world behind the main menu, or the world of a game.
struct ShellWorld {
    world: Rc<World>,
    driver: SkirmishDriver,
    read_file: ReadFile,
    scene: Option<Scene>,
    quads: Vec<Quad>,
    passes: Vec<rustora_render::post::PostPass>,
    viewport: Rc<RefCell<Viewport>>,
    viewer: Option<PlayerRef>,
    ingame: Option<Rc<Ingame>>,
    /// The screen map of a world without a game user interface, like the shellmap.
    screen_map: ScreenMap,
    ended: bool,
    game_over_seen: bool,
    /// The slowdown of a running game that is not a replay, from its first tick attempt on.
    lag: Option<LagMonitor>,
    /// The time of the last tick attempt.
    last_attempt: Option<i64>,
}

/// `Ui.Timestep`.
pub const UI_TIMESTEP: i32 = 40;
/// The interval of `Game.Loop` between two frames while a game save loads, 5 frames per second.
const LOADING_RENDER_INTERVAL: i32 = 200;
/// A longer pause between two tick attempts, like a minimized window, starts the lag measurement anew.
const LAG_GAP_MS: i64 = 1000;

/// A lag report of a rustora server counts while it is younger than this.
const LAG_REPORT_MAX_AGE_MS: i64 = 2500;
/// A ping round trip from this length on makes a slow connection.
const SLOW_PING_MS: i32 = 300;
/// A server that let a packet wait this long is slow itself.
const SLOW_SERVER_DELAY_MS: i32 = 100;
/// The orders of a client this much later than the earliest ones make it the client the server slows the game for.
const LATE_ORDERS_MS: i32 = 20;

/// What the lag display reads from the connection state. The connection qualities come from every server; a recent
/// lag report of a rustora server makes the picture sharper.
#[derive(Default)]
struct LagContext {
    /// The own connection alone is slow, so a missing acknowledgement of the own orders points at it rather than at
    /// the server. A stalled server delays the pings of all clients, so slow connections of all clients do not count.
    own_connection_slow: bool,
    /// The client whose late orders make the server stretch the timestep, from a lag report.
    latest_client: Option<i32>,
    /// The clients with a slow connection.
    slow_connections: Vec<i32>,
    /// The software of the clients that announced it; the others count as OpenRA.
    software: rustora_core::hash::HashMap<i32, String>,
    /// The human clients with what the server tells of their connections, for the lag table of the perf text.
    clients: Vec<ClientConnection>,
}

/// What the server tells of the connection of a human client.
struct ClientConnection {
    index: i32,
    /// How much later its orders arrive than the earliest ones and its ping, from a lag report of a rustora server.
    lateness_ms: Option<i32>,
    ping_ms: Option<i32>,
    quality: rustora_net::session::ConnectionQuality,
}

impl LagContext {
    fn new(net: &rustora_ui::net::NetState, now: i64) -> LagContext {
        use rustora_net::session::{Client, ConnectionQuality};
        let report = net
            .lag_report
            .as_ref()
            .filter(|(t, _)| now - t <= LAG_REPORT_MAX_AGE_MS)
            .map(|(_, r)| r);
        let slow = |c: &Client| match report.and_then(|r| r.client(c.index)).and_then(|l| l.ping_ms) {
            Some(ping) => ping >= SLOW_PING_MS,
            None => c.connection_quality == ConnectionQuality::Poor,
        };
        let humans: Vec<&Client> = net.session.non_bot_clients().collect();
        let slow_connections: Vec<i32> = humans.iter().filter(|c| slow(c)).map(|c| c.index).collect();
        let own = net.local_client_id;
        let server_slow = report.is_some_and(|r| r.server_delay_ms >= SLOW_SERVER_DELAY_MS);
        let own_connection_slow = !server_slow
            && slow_connections.contains(&own)
            && humans.iter().any(|c| c.index != own && !slow_connections.contains(&c.index));
        let latest_client = report
            .and_then(|r| {
                r.clients
                    .iter()
                    .filter(|c| c.lateness_ms >= LATE_ORDERS_MS)
                    .max_by_key(|c| c.lateness_ms)
            })
            .map(|c| c.index);
        let clients = humans
            .iter()
            .map(|c| {
                let lag = report.and_then(|r| r.client(c.index));
                ClientConnection {
                    index: c.index,
                    lateness_ms: lag.map(|l| l.lateness_ms),
                    ping_ms: lag.and_then(|l| l.ping_ms),
                    quality: c.connection_quality,
                }
            })
            .collect();
        LagContext {
            own_connection_slow,
            latest_client,
            slow_connections,
            software: net.client_software.clone(),
            clients,
        }
    }

    /// The lines of the lag table of the perf text: the speed and the waits that no player caused, then a line per
    /// human client with its connection and the share of the time that the game waited for it. Empty for fewer than
    /// two human clients.
    fn table(&self, report: Option<&LagReport>, world: &World, local_client: i32) -> Vec<String> {
        if self.clients.len() < 2 {
            return Vec::new();
        }
        let share = |cause: LagCause| {
            report
                .and_then(|r| r.causes.iter().find(|(c, _)| *c == cause))
                .map_or(0, |(_, p)| *p)
        };
        let mut lines = vec![format!(
            "Lag in {} s: speed {}%, this computer {}%, server {}%, own connection {}%",
            rustora_client::lag_monitor::WINDOW_MS / 1000,
            report.map_or(100, |r| r.speed_percent),
            share(LagCause::ThisComputer),
            share(LagCause::Server),
            share(LagCause::OwnConnection),
        )];
        for c in &self.clients {
            let name = world
                .lobby_info
                .client_with_index(c.index)
                .and_then(|c| c.name.clone())
                .unwrap_or_else(|| format!("player {}", c.index));
            let you = if c.index == local_client { " (you)" } else { "" };
            let connection = match (c.lateness_ms, c.ping_ms) {
                (Some(late), Some(ping)) => format!("late {late} ms, ping {ping} ms"),
                (Some(late), None) => format!("late {late} ms"),
                _ => format!("connection {:?}", c.quality),
            };
            lines.push(format!("{name}{you}: {connection}, waited {}%", share(LagCause::Client(c.index))));
        }
        lines
    }

    /// The lines of the lag display for `report`.
    fn text(&self, report: &LagReport, world: &World) -> String {
        let mut lines = vec![format!("Game slowed to {}%", report.speed_percent)];
        for &(cause, percent) in &report.causes {
            let what = match cause {
                LagCause::Client(index) => {
                    let name = world
                        .lobby_info
                        .client_with_index(index)
                        .and_then(|c| c.name.clone())
                        .unwrap_or_else(|| format!("player {index}"));
                    let software = self
                        .software
                        .get(&index)
                        .map_or(rustora_net::extensions::OPENRA_SOFTWARE, String::as_str);
                    if self.slow_connections.contains(&index) {
                        format!("{name} ({software}, slow connection)")
                    } else {
                        format!("{name} ({software})")
                    }
                }
                LagCause::Server => "server".to_string(),
                LagCause::OwnConnection => "your connection".to_string(),
                LagCause::SlowerPlayers => "slower players".to_string(),
                LagCause::ThisComputer => "this computer".to_string(),
            };
            lines.push(format!("Waiting for {what}: {percent}%"));
        }
        lines.join("\n")
    }
}

/// The timesteps of the `GameSpeeds` of the mod with their names in the language of the player.
fn game_speed_names(mod_data: &ModData, messages: &Messages) -> Vec<(i32, String)> {
    let Some(speeds) = mod_data
        .manifest
        .yaml
        .get("GameSpeeds")
        .and_then(|s| s.node_with_key("Speeds").ok())
    else {
        return Vec::new();
    };
    speeds
        .value
        .nodes
        .iter()
        .filter_map(|n| {
            let get = |k: &str| n.value.nodes.iter().find(|c| c.key == k).map(|c| c.value.value_str().to_string());
            let timestep = get("Timestep")?.trim().parse().ok()?;
            Some((timestep, get("Name").map_or_else(|| n.key.clone(), |key| messages.get(&key))))
        })
        .collect()
}

/// The tick rate that `world` aims at: that of the game speed, or of the replay speed that the player chose.
fn tick_target(world: &World, speed_names: &[(i32, String)]) -> rustora_ui::perf_stats::Target {
    let timestep = if world.is_replay() {
        world.replay_timestep.get()
    } else {
        world.timestep
    };
    if timestep <= 0 {
        return rustora_ui::perf_stats::Target {
            label: "Paused".to_string(),
            rate: None,
        };
    }
    let label = speed_names
        .iter()
        .find(|(t, _)| *t == timestep)
        .map_or_else(|| format!("{timestep} ms per tick"), |(_, name)| name.clone());
    rustora_ui::perf_stats::Target {
        label,
        rate: Some(1000.0 / timestep as f64),
    }
}

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

/// `MusicPlaylist.WorldLoaded` and `StartGameNotification.PostWorldLoaded`, or `GameLoaded` for a loaded game save.
fn start_music(services: &Rc<Services>, shell: Option<&ShellWorld>) {
    let Some(shell) = shell else { return };
    if shell.world.is_loading_game_save() {
        return;
    }
    services.sound.load_music_files(shell.world.rules().audio());
    let music = MusicPlaylist::new(&shell.world, services.sound.clone(), services.settings.clone());
    if let Some(music) = &music {
        music.post_world_loaded();
    }
    *services.music.borrow_mut() = music;
    if let Some(info) = shell.world.world_actor().info.trait_info::<StartGameNotificationInfo>() {
        let loaded = shell.world.game_save_last_frame() >= 0;
        if loaded && shell.world.is_replay() {
            return;
        }
        let (notification, text) = if loaded {
            (&info.loaded_notification, info.loaded_text_notification.as_deref())
        } else {
            (&info.notification, info.text_notification.as_deref())
        };
        let faction = shell.viewer.as_ref().map(|p| p.faction.as_str());
        services
            .sound
            .play_notification(shell.world.rules().audio(), "Speech", notification, faction);
        shell.world.sound.add_transient_line(None, text);
    }
}

/// Whether a game that starts now still has to load a game save before `GameLoaded`. A game resumed from a state
/// snapshot can start past the saved orders, and it still goes through `GameLoaded` once.
fn awaits_game_loaded(world: &World) -> bool {
    world.is_loading_game_save() || (world.game_save_last_frame() >= 0 && !world.is_replay())
}

/// The shortest time between two load screen frames, the 5 frames per second limit of `SheetLoadScreen.Display`.
const LOAD_SCREEN_INTERVAL: Duration = Duration::from_millis(200);

/// Shows the load screen while the client starts, as `LoadScreen.Display` does during `Game.InitializeMod`.
pub struct Startup<'a> {
    renderer: &'a mut Renderer,
    /// The size of the window in its pixels.
    window: (u32, u32),
    /// Takes the frame that `renderer` recorded and sends it to the window.
    send: &'a mut dyn FnMut(&mut Renderer),
    /// When the last frame was sent.
    last_display: Option<std::time::Instant>,
}

impl<'a> Startup<'a> {
    pub fn new(renderer: &'a mut Renderer, window: (u32, u32), send: &'a mut dyn FnMut(&mut Renderer)) -> Startup<'a> {
        Startup {
            renderer,
            window,
            send,
            last_display: None,
        }
    }

    /// `SheetLoadScreen.Display`: records a frame of `screen` at the user interface scale `ui_scale` and sends it,
    /// unless the last one is less than 200 ms old.
    fn show(&mut self, screen: &mut rustora_ui::load_screen::LoadScreen, assets: &Rc<RefCell<Assets>>, ui_scale: f32) {
        if self.last_display.is_some_and(|at| at.elapsed() < LOAD_SCREEN_INTERVAL) {
            return;
        }
        let renderer = &mut *self.renderer;
        renderer.ui_scale = ui_scale;
        let (w, h) = effective_resolution((self.window.0 as i32, self.window.1 as i32), ui_scale);
        let mut list = UiList::default();
        screen.draw(&mut DrawContext::new(renderer, &mut list, assets.borrow_mut()), (w, h));
        let camera = Camera {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
            width: w.max(1) as u32,
            height: h.max(1) as u32,
        };
        renderer.render(camera, &[]);
        renderer.render_ui(self.window, &list);
        (self.send)(renderer);
        self.last_display = Some(std::time::Instant::now());
    }
}

/// The size in user interface pixels of a window of `window_size` pixels at the user interface scale `scale`.
fn effective_resolution(window_size: (i32, i32), scale: f32) -> (i32, i32) {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    ((window_size.0 as f32 / scale) as i32, (window_size.1 as f32 / scale) as i32)
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
    /// `Game.StartGame`: the world of a game that the server started. With `snapshot`, the world restores that
    /// save game and continues from it.
    #[allow(clippy::too_many_arguments)]
    fn start_game(
        engine: &Path,
        support: &str,
        package: Arc<dyn Package>,
        lobby: Session,
        mut connection: NetworkConnection,
        mod_data: &ModData,
        view: (i32, i32),
        distance: WorldViewport,
        snapshot: Option<Vec<u8>>,
    ) -> ShellWorld {
        let local_client = connection.local_client_id();
        let is_host = lobby.client_with_index(local_client).is_some_and(|c| c.is_admin);
        let restored = snapshot.and_then(|save| {
            rustora_launch::restore_game_from(engine, support, package.clone(), &save, false, true)
                .inspect_err(|e| log::warn!("The state snapshot cannot be restored: {e}"))
                .ok()
        });
        let resumed = restored.is_some();
        if let Some((world, _, _)) = &restored {
            log::info!("Restored the state snapshot at net frame {}", world.net_frame_number());
        }
        let (world, _, read_file) =
            restored.unwrap_or_else(|| load_world_with_sound(engine, support, package, lobby, false, WorldType::Regular, true, &mut || {}));
        let manifest = &mod_data.manifest;
        connection.set_replay_metadata(create_game_information(&world, &manifest.id, &manifest.metadata.version));
        let driver = if resumed {
            SkirmishDriver::resumed_with_connection(&world, Box::new(connection))
        } else {
            SkirmishDriver::with_connection(&world, Box::new(connection), is_host)
        };
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
            passes: Vec::new(),
            viewport,
            viewer,
            ingame: None,
            screen_map: ScreenMap::default(),
            ended: false,
            game_over_seen: false,
            lag: None,
            last_attempt: None,
        }
    }

    /// `Game.JoinReplay` and `Game.StartGame` for a replay: the world plays the recorded orders.
    #[allow(clippy::too_many_arguments)]
    fn start_replay(
        engine: &Path,
        support: &str,
        package: Arc<dyn Package>,
        lobby: Session,
        replay: Replay,
        view: (i32, i32),
        distance: WorldViewport,
        snapshot: Option<Vec<u8>>,
    ) -> ShellWorld {
        let restored = snapshot.and_then(|save| {
            rustora_launch::restore_game_from(engine, support, package.clone(), &save, true, true)
                .inspect_err(|e| log::warn!("The state snapshot of the replay cannot be restored: {e}"))
                .ok()
        });
        let (world, driver, read_file) = match restored {
            Some((world, _, read_file)) => {
                log::info!(
                    "Restored the state snapshot of the replay at net frame {}",
                    world.net_frame_number()
                );
                let driver = SkirmishDriver::replay_resumed(&world, replay.chunks);
                (world, driver, read_file)
            }
            None => {
                let (world, _, read_file) =
                    load_world_with_sound(engine, support, package, lobby, true, WorldType::Regular, true, &mut || {});
                let connection = ReplayConnection::new(replay.chunks, world.order_latency);
                let driver = SkirmishDriver::replay(&world, Box::new(connection));
                (world, driver, read_file)
            }
        };
        let viewport = world_viewport(&world, view, distance);
        ShellWorld {
            world,
            driver,
            read_file,
            scene: None,
            quads: Vec::new(),
            passes: Vec::new(),
            viewport,
            viewer: None,
            ingame: None,
            screen_map: ScreenMap::default(),
            ended: false,
            game_over_seen: false,
            lag: None,
            last_attempt: None,
        }
    }

    /// Port of `Game.LoadShellMap` and `ChooseShellmap`. `None` without a shell map. `progress` runs between the
    /// loading steps.
    fn load(
        engine: &Path,
        support: &str,
        mod_id: &str,
        view: (i32, i32),
        distance: WorldViewport,
        progress: &mut dyn FnMut(),
    ) -> Option<ShellWorld> {
        let names = shellmaps(engine, mod_id);
        progress();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let name = names.get(now.subsec_nanos() as usize % names.len().max(1))?;
        let package = open_map(engine, mod_id, name)?;
        let map = MapPackage::load(package.clone()).ok()?;
        let lobby = skirmish_session(&map, &[], now.subsec_nanos() as i32).ok()?;
        let (world, _, read_file) = load_world_with_sound(engine, support, package, lobby, false, WorldType::Shellmap, true, progress);
        let driver = SkirmishDriver::new(&world);
        let viewport = world_viewport(&world, view, distance);
        Some(ShellWorld {
            world,
            driver,
            read_file,
            scene: None,
            quads: Vec::new(),
            passes: Vec::new(),
            viewport,
            viewer: None,
            ingame: None,
            screen_map: ScreenMap::default(),
            ended: false,
            game_over_seen: false,
            lag: None,
            last_attempt: None,
        })
    }

    /// `OrderManager.SuggestedTimestep`.
    fn suggested_timestep(&self) -> i32 {
        let world = &self.world;
        if world.is_loading_game_save() {
            1
        } else if world.is_replay() {
            world.replay_timestep.get()
        } else if self.driver.orders.tick_scale != 1.0 {
            ((self.driver.orders.tick_scale * world.timestep as f32) as i32).max(1)
        } else {
            world.timestep
        }
    }

    /// Attempts a world tick at `now` that was scheduled with `timestep`. `lag` is what the lag display reads from
    /// the connection state.
    fn tick(&mut self, now: i64, timestep: i32, lag: &LagContext) {
        if self.ended {
            // The simulation has stopped advancing, but keep exchanging immediate orders, like OpenRA's
            // decoupled `OrderManager.TickImmediate`, so chat still works after the game ends.
            self.driver.orders.tick_immediate();
        } else {
            let step = self.driver.step();
            if step == Step::Ended {
                self.ended = true;
            }
            self.measure_lag(now, timestep, step, lag);
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

    /// Records the tick attempt in the lag monitor and puts its report into the in-game state.
    fn measure_lag(&mut self, now: i64, timestep: i32, step: Step, lag: &LagContext) {
        let Some(ingame) = self.ingame.clone() else {
            rustora_ui::perf_stats::set_lag(Vec::new());
            return;
        };
        let orders = &self.driver.orders;
        if self.world.is_replay() || self.world.is_loading_game_save() || !orders.game_started() || step == Step::Ended {
            self.lag = None;
            ingame.lag_text.borrow_mut().clear();
            rustora_ui::perf_stats::set_lag(Vec::new());
            return;
        }
        let monitor = self.lag.get_or_insert_with(|| LagMonitor::new(self.world.timestep));
        if self.last_attempt.is_some_and(|t| now - t > LAG_GAP_MS) {
            monitor.reset();
        }
        self.last_attempt = Some(now);
        match step {
            Step::Waiting => {
                let local = orders.connection.local_client_id();
                let causes = wait_causes(&orders.missing_clients(), local, lag.own_connection_slow);
                monitor.wait(now, timestep, causes);
            }
            _ => monitor.tick(now, timestep, lag.latest_client),
        }
        let text = match monitor.report(now) {
            Some(LagDisplay::Slow(report)) => lag.text(&report, &self.world),
            Some(LagDisplay::Recovered(percent)) => format!("Game speed normal ({percent}%)"),
            None => String::new(),
        };
        *ingame.lag_text.borrow_mut() = text;
        let local = self.driver.orders.connection.local_client_id();
        let measured = self.lag.as_ref().and_then(|m| m.measure(now));
        rustora_ui::perf_stats::set_lag(lag.table(measured.as_ref(), &self.world, local));
    }

    /// Feeds the user interface with the orders that the world ignores, and sends the chat lines it wrote.
    fn exchange_client_orders(&mut self, messages: &Messages, net: &rustora_ui::net::Net, run_time: i64, save_dir: &Path) {
        self.driver.orders.collect_client_orders = true;
        let client_orders = std::mem::take(&mut self.driver.orders.client_orders);
        let mut state = net.borrow_mut();
        let chat_world = (!client_orders.is_empty()).then(|| crate::net::ChatWorld::of(&self.world));
        for (client_id, order) in client_orders {
            if order.order_string == "GameSaved" {
                if let Some(name) = order.target_string.as_deref().filter(|_| !self.world.is_replay()) {
                    crate::snapshot::write(&self.world, &save_dir.join(name));
                }
                self.game_saved(messages, &mut state);
                continue;
            }
            if order.order_string == "PauseGame" {
                self.game_paused(messages, &mut state, client_id, &order);
                continue;
            }
            if let Err(e) = crate::net::process_client_order(messages, &mut state, chat_world.as_ref(), client_id, &order, run_time) {
                log::warn!("{e}");
            }
        }
        let orders = &mut self.driver.orders;
        for order in std::mem::take(&mut state.outgoing) {
            orders.issue_order(order);
        }
        if !orders.connection.is_connected() {
            state.connection_state = rustora_ui::net::ConnectionState::NotConnected;
        }
    }

    /// `UnitOrders.ProcessOrder` of `GameSaved` and `StartGameNotification.GameSaved`.
    fn game_saved(&self, messages: &Messages, net: &mut rustora_ui::net::NetState) {
        if self.world.is_replay() {
            return;
        }
        net.notifications.push(rustora_ui::net::TextNotification {
            pool: rustora_ui::net::TextNotificationPool::System,
            client_id: rustora_ui::net::SYSTEM_CLIENT_ID,
            prefix: Some("Battlefield Control".to_string()),
            text: messages.get("notification-game-saved"),
            prefix_color: None,
            text_color: None,
            time: rustora_ui::net::local_time(),
        });
        if let Some(info) = self.world.world_actor().info.trait_info::<StartGameNotificationInfo>() {
            let faction = self.viewer.as_ref().map(|p| p.faction.as_str());
            rustora_engine::sound::play_notification(&self.world, None, "Speech", &info.saved_notification, faction);
            self.world.sound.add_transient_line(None, info.saved_text_notification.as_deref());
        }
    }

    /// `UnitOrders.ProcessOrder` of a `PauseGame` order that changed the pause state. Like OpenRA, it announces the
    /// change only in games with more than one human client; a single player game announces it in the pause hotkey
    /// and the `/pause` command, so that the pause of the ingame menu and of videos stays silent.
    fn game_paused(&self, messages: &Messages, net: &mut rustora_ui::net::NetState, client_id: i32, order: &RawOrder) {
        let lobby = &self.world.lobby_info;
        if lobby.clients.iter().filter(|c| c.bot.is_none()).count() <= 1 {
            return;
        }
        let name = lobby.client_with_index(client_id).and_then(|c| c.name.clone()).unwrap_or_default();
        let paused = order.target_string.as_deref() == Some("Pause");
        net.notifications
            .push(rustora_ui::logic::ingame_menu::pause_notification(messages, paused, &name));
    }

    /// Hands the sound calls of the last tick to the audio system, and what else `Game.InnerLogicTick` tells it.
    fn update_audio(&mut self, services: &Services, messages: &Messages) {
        if self.world.is_loading_game_save() {
            self.world.sound.take();
            self.world.sound.take_transient_lines();
            self.world.sound.take_system_lines();
            return;
        }
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
                    client_id: -1,
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
                client_id: -1,
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
        scene.min_zoom = self.viewport.borrow().min_zoom;
        let overlay = self.ingame.as_ref().map_or_else(Vec::new, |i| i.order_generator_renderables());
        let viewer = self.ingame.as_ref().map_or_else(|| self.viewer.clone(), |i| i.render_player());
        let mut ingame_screen_map = self.ingame.as_ref().map(|i| i.screen_map.borrow_mut());
        let screen_map = match ingame_screen_map.as_deref_mut() {
            Some(m) => m,
            None => &mut self.screen_map,
        };
        let on_screen = scene.update_screen_map(renderer, world, &cam, screen_map);
        self.quads = scene.build(renderer, world, viewer.as_ref(), &cam, &on_screen, &overlay);
        self.passes = crate::scene::post_passes(world, scene.after_actors, scene.after_world, self.quads.len());
        if self.ingame.is_some() {
            screen_map.set_frozen(scene.frozen_mouse_bounds(renderer, world, viewer.as_ref(), &cam));
        }
    }
}

/// A loaded user interface with the shell root open.
/// What starts once the load screen is visible.
enum PendingLoad {
    Game,
    Replay(String),
    /// The world is loaded; the first frame builds the scene and loads the sprites, after another load screen frame.
    Scene,
}

pub struct Client {
    pub ui: Ui,
    pub cursors: Cursors,
    pub cursor: CursorState,
    /// The mouse is over a link, which shows the pointing hand of the system.
    pub link_cursor: bool,
    /// The links to open in the browser, which the window opens.
    pub open_urls: Vec<String>,
    /// `Game.TakeScreenshot` was called: the window saves its next frame.
    pub screenshot_requested: bool,
    /// The size of the window in its pixels.
    window_size: (i32, i32),
    /// The user interface scale in use: window pixels per user interface pixel.
    ui_scale: f32,
    list: UiList,
    shell: Option<ShellWorld>,
    engine: std::path::PathBuf,
    support: String,
    mod_data: Rc<ModData>,
    services: Rc<Services>,
    net: NetClient,
    /// The replay file of the running replay, for `Game.RestartGame`.
    replay_path: Option<String>,
    /// The game save that the single player server loads, whose state snapshot the client restores if it can.
    loading_save: Option<PathBuf>,
    /// The hidden in-game root while a game save loads.
    loading_root: Option<rustora_ui::widget::WidgetId>,
    /// A world load that waits until the load screen was drawn once, and whether it was.
    pending_load: Option<PendingLoad>,
    /// The state snapshot that the replay of `Launch.Replay` starts from (`Launch.ReplayFromSave`).
    replay_snapshot: Option<String>,
    /// The position in world pixels that the view of the first started world centers on (`Launch.ViewCenter`).
    view_center: Option<(i32, i32)>,
    /// The world ticks of the benchmark of `--bench-ticks` (`Launch.BenchTicks`).
    bench_ticks: Option<i32>,
    /// `--bench-realtime`: the benchmark plays at the timestep of the world.
    bench_realtime: bool,
    /// `--bench-timestep`: the timestep of a replay in milliseconds instead of the one of its game speed.
    bench_timestep: Option<i32>,
    /// The time that [`ShellWorld::tick`] took so far, the world ticks with their orders, for the benchmark.
    world_busy: Duration,
    /// The timesteps of the game speeds of the mod with their names, for the tick rate target of the perf text.
    speed_names: Vec<(i32, String)>,
    /// The world tick at which the replay pauses itself (`Launch.PauseAtTick`), for reproducible screenshots.
    pause_at_tick: Option<i32>,
    load_screen_drawn: bool,
    load_screen: rustora_ui::load_screen::LoadScreen,
    ui_tick_time: TickTime,
    world_tick_time: TickTime,
    script_host: Rc<crate::script_host::ClientScriptHost>,
    widget_roots: rustora_game::lobby_rules::WidgetRoots,
}

impl Client {
    /// Starts the mod of the `Game.Mod=<id>` argument, `ra` without one. With `startup`, the load screen shows
    /// while the mod and the shellmap load.
    pub fn new(engine: &Path, support: &str, resolution: (i32, i32), args: &[String], startup: Option<&mut Startup>) -> Client {
        let mod_id = args.iter().rev().find_map(|a| a.strip_prefix("Game.Mod=")).unwrap_or("ra");
        Client::initialize_mod(engine, support, resolution, args, mod_id, startup)
    }

    pub fn mod_id(&self) -> &str {
        &self.mod_data.manifest.id
    }

    /// Port of `Game.InitializeMod`: a mod with missing content opens its content installer mod instead.
    fn initialize_mod(
        engine: &Path,
        support: &str,
        resolution: (i32, i32),
        args: &[String],
        mod_id: &str,
        mut startup: Option<&mut Startup>,
    ) -> Client {
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
        let widget_roots = rustora_game::lobby_rules::WidgetRoots::load(&mod_data);
        let mut load_screen = rustora_ui::load_screen::LoadScreen::new(&mod_data.manifest.load_screen);
        let fs: Rc<dyn ReadFs> = Rc::new(ModFs(mod_data.clone()));
        let mut assets = Assets::load(&mod_data, fs.clone()).expect("load chrome");
        assets.hotkeys.apply_settings(&settings.keys);
        let ui_scale = Some(settings.graphics.ui_scale).filter(|s| *s > 0.0).unwrap_or(1.0);
        assets.set_font_scale(ui_scale);
        let speed_names = game_speed_names(&mod_data, &assets.messages);
        let assets = Rc::new(RefCell::new(assets));
        let mut progress = || {
            if let Some(startup) = startup.as_mut() {
                startup.show(&mut load_screen, &assets, ui_scale);
            }
        };
        progress();
        let cursors = Cursors::load(&mod_data, &*fs).expect("load cursors");
        let loader = WidgetLoader::new(&mod_data.manifest.chrome_layout, &*fs).expect("load chrome layouts");
        progress();
        let maps = Rc::new(RefCell::new(MapCache::load(&mod_data)));
        progress();
        let lobby_data = LobbyData::new(mod_data.clone(), maps.clone());
        let sound = open_sound(&mod_data, &settings);
        let mod_audio = load_mod_audio(&mod_data);
        sound.load_music_files(&mod_audio);
        let services = Services {
            mod_id: mod_data.manifest.id.clone(),
            mod_version: mod_data.manifest.metadata.version.clone(),
            installed_mods: mod_data.installed.clone(),
            replay_dir: rustora_core::own_files::replays_dir(
                Path::new(support),
                &mod_data.manifest.id,
                &mod_data.manifest.metadata.version,
            ),
            save_dir: rustora_net::game_save::saves_dir(Path::new(support), &mod_data.manifest.id, &mod_data.manifest.metadata.version),
            content_installer_mod: installer_mod,
            missions: mod_data.manifest.missions.clone(),
            maps,
            map_repository: Rc::default(),
            settings: Rc::new(RefCell::new(settings)),
            net: Rc::default(),
            lobby_rules: RefCell::new(Some(lobby_rules_lookup(lobby_data.clone()))),
            color_rules: RefCell::new(Some(color_rules_lookup(lobby_data))),
            ingame: RefCell::new(None),
            sound,
            mod_audio,
            music: RefCell::new(None),
            general_chat: RefCell::default(),
        };
        let net = NetClient::new(engine, support, &mod_data.manifest.id, services.net.clone());
        let services = Rc::new(services);
        let pause_shellmap = services.settings.borrow().game.pause_shellmap;
        let ui_scroll_speed = services.settings.borrow().game.ui_scroll_speed;
        let distance = services.settings.borrow().graphics.viewport_distance;
        let window_size = resolution;
        let resolution = effective_resolution(window_size, ui_scale);
        let script_host = Rc::new(crate::script_host::ClientScriptHost::new(assets.clone()));
        rustora_scripting::host::set_host(script_host.clone());
        let shell = if mod_data.content_available && content.is_none() {
            ShellWorld::load(engine, support, &mod_data.manifest.id, resolution, distance, &mut progress)
        } else {
            None
        };
        if let Some(shell) = &shell {
            shell.world.pause_shellmap.set(pause_shellmap);
        }
        start_music(&services, shell.as_ref());
        let mut ui = Ui::new(assets, Rc::new(loader), resolution, services.clone());
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
                ui.load_widget(&widget_roots.shellmap, Some(ui.root), &menu_args);
                if let Some(name) = args.iter().find_map(|a| a.strip_prefix("Launch.LoadGame=")) {
                    let path = services.save_dir.join(name);
                    if let Err(e) = rustora_ui::logic::game_save_browser::start_game_save(&mut ui, &path) {
                        log::error!("{e}");
                    }
                }
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
            link_cursor: false,
            open_urls: Vec::new(),
            screenshot_requested: false,
            window_size,
            ui_scale,
            list: UiList::default(),
            shell,
            engine: engine.to_path_buf(),
            support: support.to_string(),
            mod_data,
            services,
            net,
            replay_path: None,
            loading_save: None,
            loading_root: None,
            pending_load: None,
            replay_snapshot: args
                .iter()
                .find_map(|a| a.strip_prefix("Launch.ReplayFromSave="))
                .map(str::to_string),
            view_center: args
                .iter()
                .find_map(|a| a.strip_prefix("Launch.ViewCenter="))
                .and_then(|v| v.split_once(','))
                .and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?))),
            bench_ticks: args
                .iter()
                .find_map(|a| a.strip_prefix("Launch.BenchTicks="))
                .and_then(|n| n.parse().ok()),
            bench_realtime: args.iter().any(|a| a.eq_ignore_ascii_case("Launch.BenchRealtime=True")),
            world_busy: Duration::ZERO,
            speed_names,
            bench_timestep: args
                .iter()
                .find_map(|a| a.strip_prefix("Launch.BenchTimestep="))
                .and_then(|n| n.parse().ok()),
            pause_at_tick: args
                .iter()
                .find_map(|a| a.strip_prefix("Launch.PauseAtTick="))
                .and_then(|n| n.parse().ok()),
            load_screen_drawn: false,
            load_screen,
            ui_tick_time: TickTime {
                value: -(UI_TIMESTEP as i64),
            },
            world_tick_time: TickTime {
                value: -(UI_TIMESTEP as i64),
            },
            script_host,
            widget_roots,
        }
    }

    /// Gives the scripts the camera position and the local player before a world tick.
    fn update_script_host(&self) {
        let Some(shell) = &self.shell else {
            return;
        };
        let local_player = shell
            .ingame
            .as_ref()
            .and_then(|i| i.local_player.clone())
            .or_else(|| shell.viewer.clone());
        *self.script_host.local_player.borrow_mut() = local_player;
        let (x, y) = shell.viewport.borrow().center;
        self.script_host.camera.set(Some(px_to_world(x, y)));
    }

    /// The `MediaGlobal` music calls: `GetMusicTrack` looks the track up in the rules of the map.
    fn apply_music_requests(&self) {
        let requests = std::mem::take(&mut *self.script_host.music.borrow_mut());
        let playlist = self.services.music.borrow().clone();
        let (Some(playlist), Some(shell)) = (playlist, &self.shell) else {
            return;
        };
        let rules = shell.world.rules();
        let track = |name: &str| {
            let music = rules.audio().music.get(name).cloned();
            if music.is_none() {
                log::info!("Missing music track: {name}");
            }
            music
        };
        for request in requests {
            if !playlist.is_music_available() {
                if matches!(request, crate::script_host::MusicRequest::Stop) {
                    playlist.stop();
                }
                continue;
            }
            match request {
                crate::script_host::MusicRequest::Play(name, on_complete) => {
                    let music = match name {
                        Some(name) => track(&name),
                        None => playlist.next_song(),
                    };
                    if let Some(music) = music {
                        let on_complete = RefCell::new(Some(on_complete));
                        playlist.play_then(
                            &music,
                            Rc::new(move || {
                                if let Some(f) = on_complete.borrow_mut().take() {
                                    f();
                                }
                            }),
                        );
                    }
                }
                crate::script_host::MusicRequest::Background(name) => {
                    playlist.set_background_music(name.and_then(|n| track(&n)));
                }
                crate::script_host::MusicRequest::Stop => playlist.stop(),
            }
        }
    }

    /// Carries out what the scripts asked for during the last world tick.
    fn drain_script_host(&mut self) {
        let movies = std::mem::take(&mut *self.script_host.movies.borrow_mut());
        for request in movies {
            let on_complete = request.on_complete;
            let action: rustora_ui::widgets::video_player::VideoAction = Box::new(move |_| on_complete());
            match (&self.shell, request.fullscreen) {
                (Some(shell), true) => {
                    let world = shell.world.clone();
                    rustora_ui::logic::media::play_fmv_fullscreen(&mut self.ui, &world, &request.movie, action);
                }
                _ => rustora_ui::logic::media::play_fmv_in_radar(&mut self.ui, &request.movie, action),
            }
        }
        if let Some(pos) = self.script_host.camera_request.take()
            && let Some(shell) = &self.shell
        {
            let (x, y) = crate::scene::world_to_px(pos);
            shell.viewport.borrow_mut().center_on((x as i32, y as i32));
        }
        self.apply_music_requests();
        let lines = std::mem::take(&mut *self.script_host.lines.borrow_mut());
        let mut net = self.services.net.borrow_mut();
        for line in lines {
            net.notifications.push(rustora_ui::net::TextNotification {
                pool: line.pool,
                client_id: rustora_ui::net::SYSTEM_CLIENT_ID,
                prefix: Some(line.prefix),
                text: line.text,
                prefix_color: line.prefix_color,
                text_color: None,
                time: rustora_ui::net::local_time(),
            });
        }
    }

    /// The timestep of the world clock: `OrderManager.SuggestedTimestep`, or `Ui.Timestep` without a world.
    fn world_timestep(&self) -> i32 {
        if self.benchmarking() {
            return 1;
        }
        self.shell.as_ref().map_or(UI_TIMESTEP, ShellWorld::suggested_timestep)
    }

    /// The world ticks of the benchmark of `--bench-ticks`.
    pub fn bench_ticks(&self) -> Option<i32> {
        self.bench_ticks
    }

    /// Whether the benchmark of `--bench-ticks` plays at the timestep of the world (`--bench-realtime`).
    pub fn bench_realtime(&self) -> bool {
        self.bench_realtime
    }

    /// The time that the world ticks with their orders took so far.
    pub fn world_busy(&self) -> Duration {
        self.world_busy
    }

    /// The state of the game speed for the benchmark: the text of the lag display, the world and replay timesteps,
    /// whether the world is a replay, the tick scale of the server and the logic interval.
    pub fn speed_state(&self) -> String {
        let Some(shell) = self.shell.as_ref() else { return String::new() };
        let world = &shell.world;
        let lag = shell
            .ingame
            .as_ref()
            .map(|i| i.lag_text.borrow().replace('\n', " | "))
            .unwrap_or_default();
        format!(
            "timestep {} replay_timestep {} replay {} tick_scale {} interval {} lag [{}]",
            world.timestep,
            world.replay_timestep.get(),
            world.is_replay(),
            shell.driver.orders.tick_scale,
            self.logic_interval(),
            lag
        )
    }

    /// The world position at the center of the viewport while a world is shown.
    pub fn view_center(&self) -> Option<(i32, i32)> {
        self.shell.as_ref().map(|s| s.viewport.borrow().center)
    }

    /// The world tick once a game or replay runs in the user interface and no game save is loading.
    pub fn running_world_tick(&self) -> Option<i32> {
        let shell = self.shell.as_ref()?;
        (shell.ingame.is_some() && self.loading_root.is_none() && !shell.world.is_loading_game_save()).then(|| shell.world.world_tick())
    }

    /// True while the benchmark of `--bench-ticks` without `--bench-realtime` measures a running world. The world
    /// then ticks at every call of [`Client::logic_tick`], without waiting for its timestep.
    pub fn benchmarking(&self) -> bool {
        self.bench_ticks.is_some() && !self.bench_realtime && self.running_world_tick().is_some()
    }

    /// The interval of `Game.Loop` between two calls of [`Client::logic_tick`] in milliseconds.
    pub fn logic_interval(&self) -> i32 {
        match self.world_timestep() {
            0 => UI_TIMESTEP,
            t => t,
        }
    }

    /// The interval of `Game.Loop` between two frames in milliseconds: 200 while a game save loads, the logic interval
    /// with `CapFramerateToGameFps`, `1000 / MaxFramerate` with `CapFramerate`, and 1 otherwise.
    pub fn render_interval(&self) -> i32 {
        if self.shell.as_ref().is_some_and(|s| s.world.is_loading_game_save()) {
            return LOADING_RENDER_INTERVAL;
        }
        let (to_game_fps, max_framerate) = {
            let graphics = &self.services.settings.borrow().graphics;
            let max_framerate = if graphics.cap_framerate {
                graphics.max_framerate.clamp(1, 1000)
            } else {
                1000
            };
            (graphics.cap_framerate_to_game_fps, max_framerate)
        };
        if to_game_fps { self.logic_interval() } else { 1000 / max_framerate }
    }

    /// The frame rate that the client aims at: the tick rate with `CapFramerateToGameFps`, else the lower of
    /// `MaxFramerate` with `CapFramerate` and the refresh rate `refresh_hz` of the display with `vsync`.
    pub fn frame_target(&self, vsync: bool, refresh_hz: Option<f64>) -> rustora_ui::perf_stats::Target {
        use rustora_ui::perf_stats::Target;
        let (to_game_fps, cap) = {
            let graphics = &self.services.settings.borrow().graphics;
            let cap = graphics.cap_framerate.then(|| f64::from(graphics.max_framerate.clamp(1, 1000)));
            (graphics.cap_framerate_to_game_fps, cap)
        };
        if to_game_fps {
            return Target {
                label: "Game speed".to_string(),
                rate: Some(1000.0 / f64::from(self.logic_interval().max(1))),
            };
        }
        let sync = vsync.then_some(refresh_hz);
        match (cap, sync) {
            (Some(cap), Some(Some(hz))) if hz < cap => Target {
                label: "VSync".to_string(),
                rate: Some(hz),
            },
            (Some(cap), _) => Target {
                label: "Cap".to_string(),
                rate: Some(cap),
            },
            (None, Some(hz)) => Target {
                label: "VSync".to_string(),
                rate: hz,
            },
            (None, None) => Target {
                label: "Uncapped".to_string(),
                rate: None,
            },
        }
    }

    /// True if `Game.Loop` builds a frame before the next logic tick: in regular gameplay, which excludes replays and
    /// the loading of game saves, and while the benchmark of `--bench-ticks` measures.
    pub fn renders_every_tick(&self) -> bool {
        self.benchmarking()
            || self
                .shell
                .as_ref()
                .is_some_and(|s| !s.world.is_replay() && !s.world.is_loading_game_save())
    }

    /// Port of `Game.InnerLogicTick`: the user interface ticks at `Ui.Timestep`, the world at its own timestep.
    pub fn logic_tick(&mut self) {
        if self.load_screen_drawn
            && let Some(load) = self.pending_load.take_if(|l| !matches!(l, PendingLoad::Scene))
        {
            self.load_screen_drawn = false;
            match load {
                PendingLoad::Game => self.start_game(),
                PendingLoad::Replay(path) => self.join_replay(&path),
                PendingLoad::Scene => {}
            }
            self.pending_load = Some(PendingLoad::Scene);
            return;
        }
        if self.pending_load.is_some() {
            return;
        }
        let now = self.ui.run_time();
        let ui_due = self.ui_tick_time.should_advance(now, UI_TIMESTEP);
        if ui_due {
            self.ui_tick_time.advance(now, UI_TIMESTEP);
            self.pump_network();
        }
        let timestep = self.world_timestep();
        if self.benchmarking() || self.world_tick_time.should_advance(now, timestep) {
            self.world_tick_time.advance(now, timestep);
            self.update_script_host();
            let lag = LagContext::new(&self.services.net.borrow(), now);
            let sample = rustora_ui::perf::PerfSample::new("tick_time");
            let tick_started = std::time::Instant::now();
            let mut ticked_world = None;
            if let Some(shell) = &mut self.shell {
                let tick_before = shell.world.world_tick();
                let started = std::time::Instant::now();
                shell.tick(now, timestep, &lag);
                let world_time = started.elapsed();
                self.world_busy += world_time;
                if shell.world.is_replay() && self.pause_at_tick.is_some_and(|t| shell.world.world_tick() >= t) {
                    shell.world.replay_timestep.set(0);
                    self.pause_at_tick = None;
                }
                if shell.world.world_tick() != tick_before {
                    if tick_before == 0 {
                        rustora_ui::perf::reset();
                        rustora_ui::perf_stats::reset_ticks();
                    }
                    rustora_ui::perf::tick();
                    ticked_world = Some((shell.world.world_tick(), world_time));
                }
                if shell.ingame.is_some() {
                    let assets = self.ui.assets.clone();
                    shell.exchange_client_orders(&assets.borrow().messages, &self.services.net, now, &self.services.save_dir);
                }
                let assets = self.ui.assets.clone();
                shell.update_audio(&self.services, &assets.borrow().messages);
            }
            self.drain_script_host();
            self.finish_loading_game_save();
            self.services.sound.tick();
            drop(sample);
            if let Some((tick, world_time)) = ticked_world {
                rustora_ui::perf_stats::record_tick(tick, tick_started.elapsed(), world_time);
            }
        }
        if ui_due {
            let target = self.shell.as_ref().map(|s| tick_target(&s.world, &self.speed_names));
            rustora_ui::perf_stats::set_tick_target(target);
            self.cursor.tick(&self.cursors);
            rustora_ui::map_repository::MapRepository::poll(&mut self.ui);
            self.ui.tick();
        }
    }

    /// Carries out the network requests of the user interface, then receives and sends.
    /// Notes an input of the user, for the idle timeout of the lobby.
    pub fn user_acted(&self) {
        let now = self.ui.run_time();
        self.services.net.borrow_mut().user_acted(now);
    }

    fn pump_network(&mut self) {
        if self.pending_load.is_some() {
            return;
        }
        let requests = std::mem::take(&mut self.ui.requests);
        let mut leave = false;
        let mut reload = None;
        let mut replay = None;
        let mut restart = false;
        let mut scale_change = None;
        for request in requests {
            match request {
                AppRequest::LeaveGame => leave = true,
                AppRequest::StartSinglePlayer { map, skirmish } => self.net.start_single_player(map, skirmish),
                AppRequest::CreateServer {
                    name,
                    port,
                    password,
                    advertise_online,
                    map,
                } => {
                    let packages = self.map_packages(&map);
                    self.net.create_server(name, port, password, advertise_online, map, packages)
                }
                AppRequest::JoinServer { target, password } => self.net.join(&target, &password),
                AppRequest::JoinReplay(path) => replay = Some(path),
                AppRequest::RestartGame => restart = true,
                AppRequest::StartSinglePlayerGame { map, orders } => self.start_single_player_game(map, orders),
                AppRequest::Disconnect => self.net.disconnect(),
                AppRequest::Quit => self.ui.requests.push(AppRequest::Quit),
                AppRequest::InitializeMod(mod_id) => reload = Some(mod_id),
                AppRequest::OpenUrl(url) => self.open_urls.push(url),
                AppRequest::SetUiScale(scale) => scale_change = Some(scale),
                AppRequest::TakeScreenshot => self.screenshot_requested = true,
            }
        }
        if let Some(scale) = scale_change {
            self.set_ui_scale(scale);
            return;
        }
        if leave {
            self.leave_game();
        }
        if let Some(mod_id) = reload {
            self.replace_mod(&mod_id);
            return;
        }
        if restart {
            match self.replay_path.clone() {
                Some(path) => replay = Some(path),
                None => {
                    self.restart_game();
                    return;
                }
            }
        }
        if let Some(path) = replay {
            self.pending_load = Some(PendingLoad::Replay(path));
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
            self.pending_load = Some(PendingLoad::Game);
        }
    }

    /// `LoadWidgetAtGameStart.GameLoaded` and the rest of `INotifyGameLoaded`, once the orders of a game save ran.
    fn finish_loading_game_save(&mut self) {
        let Some(root) = self.loading_root else { return };
        let Some(world) = self.shell.as_ref().map(|s| s.world.clone()) else {
            self.loading_root = None;
            return;
        };
        if world.is_loading_game_save() {
            return;
        }
        self.loading_root = None;
        self.ui.close_window();
        self.ui.set_visible_fn(root, || true);
        start_music(&self.services, self.shell.as_ref());
        if let Some(fade) = world
            .world_actor()
            .trait_or_default::<rustora_mods_common::post_effects::MenuPostProcessEffect>()
        {
            fade.game_loaded(&world);
        }
        if !world.is_replay() {
            world.set_pause_state(false);
            if self.bench_ticks.is_none()
                && let Some(options) = self.ui.find(root, "OPTIONS_BUTTON")
            {
                let action = self.ui.button(options).on_click.clone();
                if let Some(action) = action {
                    action(&mut self.ui);
                }
            }
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
            ShellWorld::load(
                &self.engine,
                &self.support,
                &self.mod_data.manifest.id,
                self.ui.resolution,
                distance,
                &mut || {},
            )
        } else {
            None
        };
        if let Some(shell) = &self.shell {
            shell.world.pause_shellmap.set(pause_shellmap);
        }
        start_music(&self.services, self.shell.as_ref());
        let root = self.widget_roots.shellmap.clone();
        self.ui.load_widget(&root, Some(self.ui.root), &WidgetArgs::default());
    }

    /// `Game.RestartGame` for a game: a new single player server gets the lobby of the current game with a new random seed,
    /// and the game starts at once.
    fn restart_game(&mut self) {
        let mut lobby = self.services.net.borrow().session.clone();
        lobby.global_settings.random_seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos() as i32);
        let map = lobby
            .global_settings
            .map
            .clone()
            .filter(|uid| self.services.maps.borrow().get(uid).is_some());
        let Some(map) = map else {
            self.leave_game();
            return;
        };
        self.net.disconnect();
        self.ui.reset_all();
        *self.services.ingame.borrow_mut() = None;
        self.dispose_world();
        let orders = vec![
            RawOrder::command(&format!("sync_lobby {}", lobby.serialize())),
            RawOrder::command("startgame"),
        ];
        self.start_single_player_game(map, orders);
    }

    /// The package path of the map `uid`, if it is installed.
    fn map_packages(&self, uid: &str) -> Vec<String> {
        self.services
            .maps
            .borrow()
            .get(uid)
            .map(|p| p.package_name.clone())
            .filter(|name| !name.is_empty())
            .into_iter()
            .collect()
    }

    /// `Game.CreateAndStartLocalServer`.
    fn start_single_player_game(&mut self, map: String, orders: Vec<RawOrder>) {
        self.loading_save = orders
            .iter()
            .find(|o| o.order_string == "LoadGameSave")
            .and_then(|o| o.target_string.as_deref())
            .map(|name| self.services.save_dir.join(name));
        self.net.start_single_player(map, false);
        self.net.join(&rustora_ui::net::JoinTarget::SinglePlayer, "");
        self.net.set_setup_orders(orders);
    }

    /// The rest of `Game.InitializeMod`: drops everything of the running mod and starts `mod_id`.
    fn replace_mod(&mut self, mod_id: &str) {
        self.net.disconnect();
        self.ui.reset_all();
        *self.services.ingame.borrow_mut() = None;
        self.dispose_world();
        let clipboard = self.ui.clipboard.clone();
        let (engine, support, resolution) = (self.engine.clone(), self.support.clone(), self.window_size);
        *self = Client::initialize_mod(&engine, &support, resolution, &[], mod_id, None);
        self.ui.clipboard = clipboard;
    }

    /// `World.Dispose`: the audio of the old world ends with it.
    fn dispose_world(&mut self) {
        self.loading_root = None;
        if let Some(music) = self.services.music.borrow_mut().take() {
            music.dispose();
        }
        self.services.sound.stop_audio();
        self.shell = None;
    }

    /// `Game.StartGame`. The in-game user interface follows with slice 6 of P8.
    fn start_game(&mut self) {
        self.replay_path = None;
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
        let game_save_frames = self.net.take_game_save_frames();
        let loading_save = self.loading_save.take();
        let snapshot = game_save_frames
            .zip(loading_save)
            .and_then(|((last_frame, _), path)| crate::snapshot::read(&path, &session, last_frame));
        self.ui.reset_all();
        self.dispose_world();
        let distance = self.services.settings.borrow().graphics.viewport_distance;
        let shell = ShellWorld::start_game(
            &self.engine,
            &self.support,
            package,
            session,
            connection,
            &self.mod_data,
            self.ui.resolution,
            distance,
            snapshot,
        );
        if let Some((last_frame, last_sync_frame)) = game_save_frames {
            shell.world.begin_loading_game_save(last_frame, last_sync_frame);
        }
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
        self.replay_path = Some(path.to_string());
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
            .and_then(|m| rustora_client::game_information::GameInformation::deserialize(m, path).ok())
            .map_or(0, |info| info.root.final_game_tick);
        let distance = self.services.settings.borrow().graphics.viewport_distance;
        let snapshot = self.replay_snapshot.take().and_then(|p| {
            std::fs::read(&p)
                .inspect_err(|e| log::error!("cannot read the state snapshot {p}: {e}"))
                .ok()
        });
        let shell = ShellWorld::start_replay(
            &self.engine,
            &self.support,
            package,
            lobby,
            replay,
            self.ui.resolution,
            distance,
            snapshot,
        );
        let status = rustora_ui::logic::ingame::ReplayStatus {
            tick_count,
            final_game_tick,
            out_of_sync: std::cell::Cell::new(false),
        };
        self.enter_game(shell, Some(status));
    }

    /// The part of `Game.StartGame` after the world loaded: the in-game user interface.
    fn enter_game(&mut self, mut shell: ShellWorld, replay: Option<rustora_ui::logic::ingame::ReplayStatus>) {
        if let Some(timestep) = self.bench_timestep
            && shell.world.is_replay()
        {
            shell.world.replay_timestep.set(timestep);
        }
        if let Some(center) = self.view_center.take() {
            shell.viewport.borrow_mut().center_on(center);
        }
        let map = shell.world.map();
        rustora_core::log_channels::set_panic_context(vec![
            format!("{} mod version {}", self.services.mod_id, self.services.mod_version),
            format!("on map {} ({}).", map.uid, map.title),
        ]);
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
        ingame.setup_relationship_colors(
            self.services.settings.borrow().game.use_player_stance_colors,
            &self.ui.assets.borrow().metrics,
        );
        let ingame = Rc::new(ingame);
        shell.ingame = Some(ingame.clone());
        *self.services.ingame.borrow_mut() = Some(ingame);
        self.shell = Some(shell);
        let world = self.shell.as_ref().map(|s| s.world.clone());
        let loading = world.as_ref().is_some_and(|w| awaits_game_loaded(w));
        if !loading {
            start_music(&self.services, self.shell.as_ref());
        }
        let ingame_root = self.widget_roots.ingame.clone();
        let root = self.ui.load_widget(&ingame_root, Some(self.ui.root), &WidgetArgs::default());
        let lag_display = self.ui.load_widget("LAG_DISPLAY", Some(root), &WidgetArgs::default());
        if let Some(ingame) = self.services.ingame.borrow().clone() {
            self.ui.label(lag_display).get_text = Some(Rc::new(move || ingame.lag_text.borrow().clone()));
        }
        if let Some(label) = self.ui.find(root, "MISSION_TEXT") {
            let (host, color_host) = (self.script_host.clone(), self.script_host.clone());
            let mut label = self.ui.label(label);
            label.get_text = Some(Rc::new(move || host.mission_text.borrow().0.clone()));
            label.get_color = Some(Rc::new(move || color_host.mission_text.borrow().1));
        }
        if let Some(world) = world.filter(|_| loading) {
            self.ui.set_visible_fn(root, || false);
            self.ui.open_window(
                &self.widget_roots.game_save_loading,
                WidgetArgs::default().with_value("world", world),
            );
            self.loading_root = Some(root);
        }
        for (what, count) in self.ui.loader.missing.borrow().iter() {
            log::info!("not ported: {what} ({count})");
        }
    }

    /// Port of the cursor selection in `Game.RenderTick`.
    pub fn update_cursor(&mut self, mouse: (i32, i32)) {
        let name = self.ui.cursor_at(self.ui.root, mouse.0, mouse.1);
        self.link_cursor = name.as_deref() == Some(rustora_ui::cursor::LINK_CURSOR);
        self.cursor.set(&self.cursors, Some(name.as_deref().unwrap_or("default")));
    }

    /// Fills the draw list of the current frame.
    pub fn prepare(&mut self, renderer: &mut Renderer, size: (u32, u32)) {
        let size = self.ui_size(size);
        if matches!(self.pending_load, Some(PendingLoad::Scene)) && self.load_screen_drawn {
            self.pending_load = None;
            self.load_screen_drawn = false;
        }
        if self.pending_load.is_some() {
            self.list.clear();
            let assets = self.ui.assets.clone();
            let mut dc = DrawContext::new(renderer, &mut self.list, assets.borrow_mut());
            self.load_screen.draw(&mut dc, (size.0 as i32, size.1 as i32));
            self.load_screen_drawn = true;
            return;
        }
        let loading = self.shell.as_ref().is_some_and(|s| s.world.is_loading_game_save());
        if let Some(shell) = self.shell.as_mut().filter(|_| !loading) {
            if let Some(ingame) = &shell.ingame {
                ingame.last_mouse.set(self.ui.last_mouse);
            }
            let _sample = rustora_ui::perf::PerfSample::new("render_world");
            shell.prepare(renderer, size);
        }
        let _sample = rustora_ui::perf::PerfSample::new("render_widgets");
        self.list.clear();
        let assets = self.ui.assets.clone();
        let mut dc = DrawContext::new(renderer, &mut self.list, assets.borrow_mut());
        if let Some(ingame) = self.shell.as_ref().and_then(|s| s.ingame.as_ref()).filter(|_| !loading) {
            rustora_ui::annotations::draw_selection_annotations(
                &mut dc,
                ingame,
                &self.services.settings.borrow(),
                self.ui.modifiers.shift,
                self.ui.last_mouse,
            );
        }
        self.ui.draw(&mut dc);
        drop(dc);
        self.ui.after_draw();
    }

    /// The user interface scale in use: window pixels per user interface pixel.
    /// The `CursorDouble` setting.
    pub fn cursor_double(&self) -> bool {
        self.services.settings.borrow().graphics.cursor_double
    }

    /// The `FrameSkip` setting.
    pub fn frame_skip(&self) -> bool {
        self.services.settings.borrow().graphics.frame_skip
    }

    /// With `FrameSkip`, the longest time between two built frames: `1 / FrameSkipMinFps`.
    pub fn frame_skip_max_interval(&self) -> std::time::Duration {
        let fps = self.services.settings.borrow().graphics.frame_skip_min_fps.max(1);
        std::time::Duration::from_secs(1) / fps as u32
    }

    pub fn ui_scale(&self) -> f32 {
        self.ui_scale
    }

    /// `size` window pixels in user interface pixels.
    fn ui_size(&self, size: (u32, u32)) -> (u32, u32) {
        let (w, h) = effective_resolution((size.0 as i32, size.1 as i32), self.ui_scale);
        (w.max(1) as u32, h.max(1) as u32)
    }

    /// `Renderer.SetUIScale` and `RecalculateWidgetLayout`: the user interface gets the size of the window at the new
    /// scale, and the main menu is built anew for it, with the settings open, from where the scale is changed. The
    /// menu world keeps running with a viewport of the new size.
    fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = if scale > 0.0 { scale } else { 1.0 };
        let resolution = effective_resolution(self.window_size, self.ui_scale);
        self.ui.resolution = resolution;
        self.ui.assets.borrow_mut().set_font_scale(self.ui_scale);
        if let Some(shell) = &self.shell {
            let distance = self.services.settings.borrow().graphics.viewport_distance;
            let viewport = world_viewport(&shell.world, resolution, distance);
            *shell.viewport.borrow_mut() = viewport.borrow().clone();
        }
        self.ui.reset_all();
        let args = WidgetArgs::default().with_value("openSettings", true);
        let root = self.widget_roots.shellmap.clone();
        self.ui.load_widget(&root, Some(self.ui.root), &args);
    }

    /// `Game.TakeScreenshotInner` with the RGBA rows of a rendered frame.
    pub fn save_screenshot(&mut self, rgba: &[u8], width: u32, height: u32) {
        let support_dir = self.mod_data.platform.support_dir.clone();
        rustora_ui::screenshot::save(&self.ui, &support_dir, rgba, width, height);
    }

    /// Records the draws of the frame that [`Client::prepare`] filled, for a window of `size` pixels.
    pub fn render(&mut self, renderer: &mut Renderer, size: (u32, u32)) {
        renderer.ui_scale = self.ui_scale;
        let window = size;
        let size = self.ui_size(size);
        match self.shell.as_ref().filter(|_| self.pending_load.is_none()) {
            Some(shell) => renderer.render_with_passes(shell.camera(size), &shell.quads, &shell.passes),
            None => renderer.render(
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
        renderer.render_ui(window, &self.list);
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
    let mut backend = GpuBackend::new(device, queue, rustora_render::wgpu::TextureFormat::Rgba8Unorm);
    let mut renderer = Renderer::new();
    let mut client = Client::new(engine, support, (size.0 as i32, size.1 as i32), &[], None);
    client.logic_tick();
    for &(x, y) in clicks {
        client.click(x, y);
    }
    client.prepare(&mut renderer, size);
    client.render(&mut renderer, size);
    let rgba = rustora_render::render_offscreen(&mut backend, &renderer.take_frame(), size.0, size.1);
    let file = std::fs::File::create(out).expect("create screenshot");
    image_webp::WebPEncoder::new(std::io::BufWriter::new(file))
        .encode(&rgba, size.0, size.1, image_webp::ColorType::Rgba8)
        .expect("encode webp");
    for (what, count) in client.ui.loader.missing.borrow().iter() {
        eprintln!("not ported: {what} ({count})");
    }
}
