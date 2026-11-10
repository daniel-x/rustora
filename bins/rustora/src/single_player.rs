// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The single player server of `Game.CreateLocalServer`: a thread of the client that hosts one server after another.
//! It loads the mod and scans the maps once, in the background from the client start on, so a single player game
//! starts without that wait. The client connects through an in-memory pipe.

use std::path::Path;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rustora_server::mod_info::ServerMod;
use rustora_server::server::{LocalConnector, Server, ServerSettings};

#[derive(Default)]
struct StartState {
    result: Option<Result<LocalConnector, String>>,
    cancelled: bool,
}

/// The outcome of a server start that the host thread reports and connecting threads wait for.
#[derive(Default)]
pub struct StartedServer {
    state: Mutex<StartState>,
    ready: Condvar,
}

impl StartedServer {
    fn lock(&self) -> std::sync::MutexGuard<'_, StartState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reports the outcome. Returns false if the server was cancelled before it started.
    fn set(&self, result: Result<LocalConnector, String>) -> bool {
        let mut state = self.lock();
        state.result = Some(result);
        self.ready.notify_all();
        !state.cancelled
    }

    /// Waits up to `patience` for the server to accept clients.
    pub fn wait(&self, patience: Duration) -> Result<LocalConnector, String> {
        let deadline = Instant::now() + patience;
        let mut state = self.lock();
        while state.result.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("the server did not start".to_string());
            }
            state = self.ready.wait_timeout(state, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        state.result.clone().unwrap_or_else(|| Err("the server did not start".to_string()))
    }

    /// Ends the server, or keeps it from running if it has not started yet.
    pub fn shutdown(&self) {
        let mut state = self.lock();
        state.cancelled = true;
        if let Some(Ok(connector)) = &state.result {
            connector.shutdown();
        }
    }
}

struct Request {
    settings: ServerSettings,
    started: Arc<StartedServer>,
}

/// The thread that hosts the single player servers.
pub struct SinglePlayerHost {
    requests: Sender<Request>,
}

impl SinglePlayerHost {
    /// Starts the thread, which loads the mod `mod_id` at once.
    pub fn start(engine: &Path, support: &str, mod_id: &str) -> SinglePlayerHost {
        let (requests, received) = channel::<Request>();
        let (engine, support, mod_id) = (engine.to_path_buf(), support.to_string(), mod_id.to_string());
        let spawned = std::thread::Builder::new().name("Single player server".to_string()).spawn(move || {
            let mut game_mod = load_mod(&engine, &support, &mod_id);
            for request in received {
                let current = match game_mod.take().map_or_else(|| load_mod(&engine, &support, &mod_id), Some) {
                    Some(m) => m,
                    None => {
                        request.started.set(Err("the mod did not load".to_string()));
                        continue;
                    }
                };
                match Server::new(request.settings, current) {
                    Ok(mut server) => {
                        if request.started.set(Ok(server.local_connector())) {
                            server.run();
                        }
                        game_mod = Some(server.into_mod());
                    }
                    Err(e) => {
                        log::error!(target: "server", "The single player server failed: {e}");
                        request.started.set(Err(e));
                    }
                }
            }
        });
        if let Err(e) = spawned {
            log::error!("Cannot start the single player server thread: {e}");
        }
        SinglePlayerHost { requests }
    }

    /// Starts a server with `settings` once the previous one ended.
    pub fn start_server(&self, settings: ServerSettings) -> Arc<StartedServer> {
        let started = Arc::new(StartedServer::default());
        let request = Request {
            settings,
            started: started.clone(),
        };
        if self.requests.send(request).is_err() {
            started.set(Err("the single player server thread is gone".to_string()));
        }
        started
    }
}

/// The mod with all maps scanned, or `None` with a log line.
fn load_mod(engine: &Path, support: &str, mod_id: &str) -> Option<ServerMod> {
    match ServerMod::load(engine, support, mod_id, &[]) {
        Ok(mut game_mod) => {
            game_mod.rescan_maps();
            Some(game_mod)
        }
        Err(e) => {
            log::error!(target: "server", "The single player server cannot load the mod: {e}");
            None
        }
    }
}
