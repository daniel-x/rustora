// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The remote parts of `MapCache.QueryRemoteMapDetails` and `MapPreview.Install`. Worker threads make the HTTP
//! requests, and `poll` applies their results to the map cache on the UI thread.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};

use rustora_core::miniyaml::MiniYaml;
use rustora_client::map_cache::{MapCache, MapStatus};
use rustora_net::web;

use crate::logic::services;
use crate::widget::Ui;

/// `MapCache.QueryRemoteMapDetails` asks for at most this many maps per request.
const QUERY_BATCH_SIZE: usize = 50;

enum Event {
    Details {
        batch: Vec<String>,
        result: Result<String, String>,
    },
    Progress {
        uid: String,
        bytes: i64,
        percentage: i32,
    },
    Downloaded {
        uid: String,
        result: Result<(String, Vec<u8>), String>,
    },
}

/// The client of the map repository of `WebServices`.
pub struct MapRepository {
    url: String,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    on_installed: RefCell<HashMap<String, Vec<Box<dyn FnOnce(&mut Ui)>>>>,
}

impl Default for MapRepository {
    fn default() -> Self {
        MapRepository::new(web::MAP_REPOSITORY_URL)
    }
}

impl MapRepository {
    pub fn new(url: &str) -> MapRepository {
        let (sender, receiver) = channel();
        MapRepository {
            url: url.to_string(),
            sender,
            receiver,
            on_installed: RefCell::default(),
        }
    }

    /// `QueryRemoteMapDetails`: asks the repository for the maps among `uids` that are unavailable.
    pub fn query<'a>(&self, maps: &RefCell<MapCache>, uids: impl IntoIterator<Item = &'a str>) {
        let mut cache = maps.borrow_mut();
        let mut seen = HashSet::new();
        let query: Vec<String> = uids
            .into_iter()
            .filter(|uid| seen.insert(*uid))
            .filter(|uid| cache.entry(uid).status == MapStatus::Unavailable)
            .map(str::to_string)
            .collect();
        if query.is_empty() {
            return;
        }
        for uid in &query {
            cache.update_remote_search(uid, MapStatus::Searching, None);
        }
        let (url, sender) = (self.url.clone(), self.sender.clone());
        std::thread::spawn(move || {
            for batch in query.chunks(QUERY_BATCH_SIZE) {
                let request = format!("{url}hash/{}/yaml", batch.join(","));
                let result = web::get(&request);
                if let Err(e) = &result {
                    log::debug!("Remote map query failed with error: {e}");
                }
                if sender
                    .send(Event::Details {
                        batch: batch.to_vec(),
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
    }

    /// `MapPreview.Install`: downloads the map `uid` and runs `on_success` after it is installed.
    pub fn install(&self, ui: &Ui, uid: &str, on_success: Option<Box<dyn FnOnce(&mut Ui)>>) {
        let services = services(ui);
        if !services.settings.borrow().game.allow_downloading {
            return;
        }
        let mut cache = services.maps.borrow_mut();
        if !matches!(cache.entry(uid).status, MapStatus::DownloadError | MapStatus::DownloadAvailable) {
            return;
        }
        cache.update(uid, |p| {
            p.status = MapStatus::Downloading;
            p.download_bytes = 0;
            p.download_percentage = 0;
        });
        if let Some(on_success) = on_success {
            self.on_installed.borrow_mut().entry(uid.to_string()).or_default().push(on_success);
        }
        let (url, sender, uid) = (format!("{}{uid}", self.url), self.sender.clone(), uid.to_string());
        std::thread::spawn(move || {
            let last = std::cell::Cell::new((-1i64, -1i32));
            let progress = |received: u64, total: Option<u64>| {
                let bytes = total.unwrap_or(0) as i64;
                let percentage = total.filter(|&t| t > 0).map_or(0, |t| (received * 100 / t) as i32);
                if last.replace((bytes, percentage)) != (bytes, percentage) {
                    let _ = sender.send(Event::Progress {
                        uid: uid.clone(),
                        bytes,
                        percentage,
                    });
                }
            };
            let result = match web::download_file(&url, &progress) {
                Ok((Some(filename), data)) => Ok((filename, data)),
                Ok((None, _)) => Err(format!("{url}: map not found")),
                Err(e) => Err(format!("{url}: {e:?}")),
            };
            let _ = sender.send(Event::Downloaded { uid, result });
        });
    }

    /// Applies the finished requests to the map cache.
    pub fn poll(ui: &mut Ui) {
        let services = services(ui);
        let repository = services.map_repository.clone();
        while let Ok(event) = repository.receiver.try_recv() {
            match event {
                Event::Details { batch, result } => {
                    let mut cache = services.maps.borrow_mut();
                    let nodes = result.and_then(|text| MiniYaml::from_str(&text, "map repository", true).map_err(|e| e.to_string()));
                    match nodes {
                        Ok(nodes) => {
                            for node in &nodes {
                                cache.update_remote_search(&node.key, MapStatus::DownloadAvailable, Some(&node.value));
                                ui.assets.borrow_mut().minimaps.remove(&node.key);
                            }
                        }
                        Err(e) => log::debug!("Remote map query failed with error: {e}"),
                    }
                    for uid in &batch {
                        if cache.entry(uid).status != MapStatus::DownloadAvailable {
                            cache.update_remote_search(uid, MapStatus::Unavailable, None);
                        }
                    }
                }
                Event::Progress { uid, bytes, percentage } => {
                    services.maps.borrow_mut().update(&uid, |p| {
                        p.download_bytes = bytes;
                        p.download_percentage = percentage;
                    });
                }
                Event::Downloaded { uid, result } => {
                    let installed = result.and_then(|(filename, data)| {
                        services
                            .maps
                            .borrow_mut()
                            .install(&uid, &filename, &data)
                            .map_err(|e| e.to_string())
                    });
                    ui.assets.borrow_mut().minimaps.remove(&uid);
                    let callbacks = repository.on_installed.borrow_mut().remove(&uid).unwrap_or_default();
                    match installed {
                        Ok(()) => {
                            for callback in callbacks {
                                callback(ui);
                            }
                        }
                        Err(e) => {
                            log::debug!("Map installation failed with error: {e}");
                            services.maps.borrow_mut().update(&uid, |p| p.status = MapStatus::DownloadError);
                        }
                    }
                }
            }
        }
    }
}
