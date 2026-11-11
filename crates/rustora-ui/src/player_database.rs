// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of the client side of `PlayerDatabase` and `PlayerProfile`: the forum profiles of registered players and the
//! icons of their badges, downloaded in the background. Unlike OpenRA, the results stay cached for the session.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};

/// `PlayerDatabase.IconSize`.
pub const ICON_SIZE: i32 = 24;

/// `PlayerBadge`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerBadge {
    pub label: String,
    pub icon: Option<String>,
    pub icon2x: Option<String>,
    pub icon3x: Option<String>,
}

/// `PlayerProfile`, the fields that the client shows.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerProfile {
    pub profile_name: String,
    pub profile_rank: String,
    pub badges: Vec<PlayerBadge>,
}

/// A download in the background: `None` while it runs, then the body or `None` on failure.
type Download = Arc<Mutex<Option<Option<Vec<u8>>>>>;

fn start_download(url: String) -> Download {
    let result: Download = Arc::default();
    let shared = result.clone();
    std::thread::spawn(move || {
        let body = rustora_net::web::get_bytes(&url).inspect_err(|e| log::debug!("{e}")).ok();
        *shared.lock().expect("download lock") = Some(body);
    });
    result
}

fn finished(download: &Download) -> Option<Option<Vec<u8>>> {
    download.lock().expect("download lock").clone()
}

#[derive(Default)]
struct Database {
    profiles: HashMap<String, Download>,
    icons: HashMap<String, Download>,
}

thread_local! {
    static DATABASE: RefCell<Database> = RefCell::default();
}

fn value(yaml: &MiniYaml, key: &str) -> Option<String> {
    yaml.nodes.iter().find(|n| n.key == key).and_then(|n| n.value.value.clone())
}

/// `PlayerDatabase.LoadBadge`: a badge needs a label.
fn load_badge(yaml: &MiniYaml) -> Option<PlayerBadge> {
    Some(PlayerBadge {
        label: value(yaml, "Label")?,
        icon: value(yaml, "Icon24"),
        icon2x: value(yaml, "Icon48"),
        icon3x: value(yaml, "Icon72"),
    })
}

/// Parses the response of the profile request: a `Player` node.
pub fn parse_profile(text: &str) -> Option<PlayerProfile> {
    let nodes: Vec<MiniYamlNode> = MiniYaml::from_str(text, "profile", true).ok()?;
    let player = nodes.into_iter().next().filter(|n| n.key == "Player")?;
    let badges = player
        .value
        .nodes
        .iter()
        .find(|n| n.key == "Badges")
        .map(|b| b.value.nodes.iter().filter_map(|n| load_badge(&n.value)).collect())
        .unwrap_or_default();
    Some(PlayerProfile {
        profile_name: value(&player.value, "ProfileName").unwrap_or_default(),
        profile_rank: value(&player.value, "ProfileRank").unwrap_or_else(|| "Registered Player".to_string()),
        badges,
    })
}

/// The profile of a fingerprint: `None` while it loads, then the profile or `None` when loading failed.
pub fn profile(fingerprint: &str) -> Option<Option<PlayerProfile>> {
    let download = DATABASE.with(|d| {
        d.borrow_mut()
            .profiles
            .entry(fingerprint.to_string())
            .or_insert_with(|| start_download(format!("{}{fingerprint}", rustora_net::web::PLAYER_PROFILE_URL)))
            .clone()
    });
    let body = finished(&download)?;
    Some(body.and_then(|b| parse_profile(&String::from_utf8_lossy(&b))))
}

/// `PlayerDatabase.GetIcon`: the URL and PNG data of the badge icon for the pixel density, once loaded. The density
/// follows the user interface scale.
pub fn icon(badge: &PlayerBadge, ui_scale: f32) -> Option<(String, Vec<u8>)> {
    let url = match () {
        _ if ui_scale > 2.0 && badge.icon3x.as_deref().is_some_and(|i| !i.is_empty()) => badge.icon3x.clone(),
        _ if ui_scale > 1.0 && badge.icon2x.as_deref().is_some_and(|i| !i.is_empty()) => badge.icon2x.clone(),
        _ => badge.icon.clone(),
    }?;
    let download = DATABASE.with(|d| {
        d.borrow_mut()
            .icons
            .entry(url.clone())
            .or_insert_with(|| start_download(url.clone()))
            .clone()
    });
    finished(&download)?.map(|data| (url, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_profile_with_badges() {
        let text = "Player:\n\tFingerprint: abc\n\tProfileID: 7\n\tProfileName: someone\n\tProfileRank: Moderator\n\tBadges:\n\t\tBadge@0:\n\t\t\tLabel: Supporter\n\t\t\tIcon24: https://x/24.png\n\t\t\tIcon48: https://x/48.png\n\t\tBadge@1:\n\t\t\tIcon24: https://x/no-label.png\n";
        let profile = parse_profile(text).expect("profile");
        assert_eq!(profile.profile_name, "someone");
        assert_eq!(profile.profile_rank, "Moderator");
        assert_eq!(profile.badges.len(), 1);
        assert_eq!(profile.badges[0].icon2x.as_deref(), Some("https://x/48.png"));
        assert_eq!(parse_profile("Error: unknown\n"), None);
    }
}
