// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port forwarding through a UPnP router (`OpenRA.Game/Network/Nat.cs`), off unless `DiscoverNatDevices` is set.
//! NAT-PMP devices, which the C# Mono.Nat library also supports, are not handled.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use igd_next::{Gateway, PortMappingProtocol, SearchOptions, search_gateway};

const DESCRIPTION: &str = "OpenRA";
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);
/// The time after which the mapping is added again, which renews its lease. A rustora addition: OpenRA maps the port
/// once per server instance, so a server that waits longer than the lifetime for players is no longer reachable.
const RENEW_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// A forwarded port that is removed again with [`PortForward::remove`].
pub struct PortForward {
    gateway: Gateway,
    port: u16,
    local: SocketAddr,
    lifetime: u32,
    renewed: Instant,
}

/// The local address of the route to `gateway`.
fn local_ip_towards(gateway: SocketAddr) -> std::io::Result<IpAddr> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))?;
    socket.connect(gateway)?;
    Ok(socket.local_addr()?.ip())
}

impl PortForward {
    /// `TryForwardPort`: maps the external TCP `port` to this host for `lifetime` seconds.
    pub fn try_forward(port: u16, lifetime: u32) -> Result<PortForward, String> {
        let mut options = SearchOptions::default();
        options.timeout = Some(SEARCH_TIMEOUT);
        let gateway = search_gateway(options).map_err(|e| e.to_string())?;
        let ip = local_ip_towards(gateway.addr).map_err(|e| e.to_string())?;
        let local = SocketAddr::new(ip, port);
        gateway
            .add_port(PortMappingProtocol::TCP, port, local, lifetime, DESCRIPTION)
            .map_err(|e| e.to_string())?;
        Ok(PortForward {
            gateway,
            port,
            local,
            lifetime,
            renewed: Instant::now(),
        })
    }

    /// Adds the mapping again on a thread once `RENEW_INTERVAL` has passed since the last time.
    pub fn renew_if_due(&mut self) {
        if self.renewed.elapsed() < RENEW_INTERVAL {
            return;
        }
        self.renewed = Instant::now();
        let (gateway, port, local, lifetime) = (self.gateway.clone(), self.port, self.local, self.lifetime);
        std::thread::spawn(
            move || match gateway.add_port(PortMappingProtocol::TCP, port, local, lifetime, DESCRIPTION) {
                Ok(()) => log::info!(target: "nat", "Renewed the port forwarding of port {port}"),
                Err(e) => log::info!(target: "nat", "Port forwarding renewal failed: {e}"),
            },
        );
    }

    /// `TryRemovePortForward`.
    pub fn remove(&self) -> Result<(), String> {
        self.gateway
            .remove_port(PortMappingProtocol::TCP, self.port)
            .map_err(|e| e.to_string())
    }
}
