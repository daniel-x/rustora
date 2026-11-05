// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! HTTP access to the OpenRA web services (`WebServices`, `PlayerDatabase`).

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// `WebServices.ServerList`.
pub const SERVER_LIST_URL: &str = "https://master.openra.net/games";
/// `WebServices.ServerAdvertise`.
pub const SERVER_ADVERTISE_URL: &str = "https://master.openra.net/ping";
/// `PlayerDatabase.Profile`.
pub const PLAYER_PROFILE_URL: &str = "https://forum.openra.net/openra/info/";

const TIMEOUT: Duration = Duration::from_secs(30);

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into()
}

/// The response body of a GET request.
pub fn get(url: &str) -> Result<String, String> {
    let mut response = agent().get(url).call().map_err(|e| format!("{url}: {e}"))?;
    response.body_mut().read_to_string().map_err(|e| format!("{url}: {e}"))
}

/// Streams the response body of a GET request to `output`. `on_progress` receives the bytes read so far and the total
/// when the server sends it. Returns an error for a status other than 200 and when `cancelled` gets set. The time limit
/// covers only the connection, because the transfer of a large file takes as long as it takes.
pub fn download(
    url: &str,
    output: &mut dyn Write,
    cancelled: &AtomicBool,
    on_progress: &dyn Fn(u64, Option<u64>),
) -> Result<(), DownloadError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|e| DownloadError::Failed(format!("{url}: {e}")))?;
    if response.status() != 200 {
        return Err(DownloadError::Status(response.status().as_u16()));
    }
    let total = response.body().content_length();
    let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut buffer = vec![0u8; 1 << 16];
    let mut received = 0u64;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let read = reader.read(&mut buffer).map_err(|e| DownloadError::Failed(format!("{url}: {e}")))?;
        if read == 0 {
            return Ok(());
        }
        output
            .write_all(&buffer[..read])
            .map_err(|e| DownloadError::Failed(e.to_string()))?;
        received += read as u64;
        on_progress(received, total);
    }
}

/// The ways `download` fails.
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    Status(u16),
    Failed(String),
}

/// The response body of a POST request.
pub fn post(url: &str, body: &str) -> Result<String, String> {
    let mut response = agent().post(url).send(body).map_err(|e| format!("{url}: {e}"))?;
    response.body_mut().read_to_string().map_err(|e| format!("{url}: {e}"))
}
