// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! rustora's extensions of the OpenRA protocol. They are immediate orders that OpenRA ignores: an OpenRA server
//! ignores unknown orders of a validated client, and an OpenRA client ignores unknown server orders, which have no
//! subject. So rustora and OpenRA programs still play together, and only rustora programs use the extensions.

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode, nodes_to_string};

/// Client to server: the software of the client, like `rustora 0.1.0`. A client sends it once it is validated, which
/// is when its first lobby information arrives, because an OpenRA server drops a client that sends anything but its
/// handshake response before.
pub const CLIENT_SOFTWARE: &str = "ClientSoftware";
/// Server to clients: the software of each client that announced it.
pub const SYNC_CLIENT_SOFTWARE: &str = "SyncClientSoftware";
/// Server to clients, once per second while a game runs: what the server sees of the lag.
pub const LAG_REPORT: &str = "LagReport";

/// The software that a client counts as when it does not announce one.
pub const OPENRA_SOFTWARE: &str = "OpenRA";

/// The `ClientSoftware` text of this program.
pub fn own_software() -> String {
    format!("rustora {}", env!("CARGO_PKG_VERSION"))
}

/// The `SyncClientSoftware` text: `ClientSoftware@<index>: <software>` for each client.
pub fn serialize_client_software(software: &[(i32, String)]) -> String {
    let nodes: Vec<MiniYamlNode> = software
        .iter()
        .map(|(index, s)| MiniYamlNode::with_value_str(format!("ClientSoftware@{index}"), s.clone()))
        .collect();
    nodes_to_string(&nodes)
}

/// The client indices and software of a `SyncClientSoftware` text.
pub fn parse_client_software(text: &str) -> Vec<(i32, String)> {
    MiniYaml::from_str(text, SYNC_CLIENT_SOFTWARE, true)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| {
            let index = n.key.strip_prefix("ClientSoftware@")?.parse().ok()?;
            Some((index, n.value.value_str().trim().to_string()))
        })
        .collect()
}

/// What the server sees of one client.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientLag {
    pub index: i32,
    /// How much later its orders arrive than those of the earliest client, the median of the last frames.
    pub lateness_ms: i32,
    /// The average round trip time of its pings, if known.
    pub ping_ms: Option<i32>,
}

/// The `LagReport` order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LagReport {
    /// The longest time a received packet waited before the server handled it, over the last second.
    pub server_delay_ms: i32,
    pub clients: Vec<ClientLag>,
}

impl LagReport {
    /// `Server: <delay>` and `Client@<index>: <lateness>,<ping>`, with an empty ping if unknown.
    pub fn serialize(&self) -> String {
        let mut nodes = vec![MiniYamlNode::with_value_str("Server", self.server_delay_ms.to_string())];
        for c in &self.clients {
            let ping = c.ping_ms.map_or_else(String::new, |p| p.to_string());
            nodes.push(MiniYamlNode::with_value_str(
                format!("Client@{}", c.index),
                format!("{},{ping}", c.lateness_ms),
            ));
        }
        nodes_to_string(&nodes)
    }

    pub fn parse(text: &str) -> Option<LagReport> {
        let nodes = MiniYaml::from_str(text, LAG_REPORT, true).ok()?;
        let mut report = LagReport::default();
        for n in nodes {
            let value = n.value.value_str().trim();
            if n.key == "Server" {
                report.server_delay_ms = value.parse().ok()?;
            } else if let Some(index) = n.key.strip_prefix("Client@") {
                let (lateness, ping) = value.split_once(',')?;
                report.clients.push(ClientLag {
                    index: index.parse().ok()?,
                    lateness_ms: lateness.trim().parse().ok()?,
                    ping_ms: ping.trim().parse().ok(),
                });
            }
        }
        Some(report)
    }

    pub fn client(&self, index: i32) -> Option<&ClientLag> {
        self.clients.iter().find(|c| c.index == index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_read_back() {
        let software = vec![(0, own_software()), (3, "rustora 9.9.9".to_string())];
        assert_eq!(parse_client_software(&serialize_client_software(&software)), software);
        let report = LagReport {
            server_delay_ms: 12,
            clients: vec![
                ClientLag {
                    index: 0,
                    lateness_ms: 0,
                    ping_ms: Some(40),
                },
                ClientLag {
                    index: 2,
                    lateness_ms: 380,
                    ping_ms: None,
                },
            ],
        };
        assert_eq!(LagReport::parse(&report.serialize()), Some(report));
    }
}
