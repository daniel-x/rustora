// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port forwarding through a UPnP router (`OpenRA.Game/Network/Nat.cs`), off unless `DiscoverNatDevices` is set.
//! NAT-PMP devices, which the C# Mono.Nat library also supports, are not handled.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;

use igd_next::{Gateway, PortMappingProtocol, SearchOptions, search_gateway};

const DESCRIPTION: &str = "OpenRA";
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);

/// A forwarded port that is removed again with [`PortForward::remove`].
pub struct PortForward {
    gateway: Gateway,
    port: u16,
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
        gateway
            .add_port(PortMappingProtocol::TCP, port, SocketAddr::new(ip, port), lifetime, DESCRIPTION)
            .map_err(|e| e.to_string())?;
        Ok(PortForward { gateway, port })
    }

    /// `TryRemovePortForward`.
    pub fn remove(&self) -> Result<(), String> {
        self.gateway
            .remove_port(PortMappingProtocol::TCP, self.port)
            .map_err(|e| e.to_string())
    }
}
