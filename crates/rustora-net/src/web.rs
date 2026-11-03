// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! HTTP access to the OpenRA web services (`WebServices`, `PlayerDatabase`).

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

/// The response body of a POST request.
pub fn post(url: &str, body: &str) -> Result<String, String> {
    let mut response = agent().post(url).send(body).map_err(|e| format!("{url}: {e}"))?;
    response.body_mut().read_to_string().map_err(|e| format!("{url}: {e}"))
}
