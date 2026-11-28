// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Records for the investigation of attacks, a rustora addition. Everything goes to the `malformed_packets` log
//! channel as `key=value` lines:
//!
//! - `kind=malformed`: a packet that breaks the protocol, with a hex dump of its first bytes.
//! - `kind=mass_drop`: several clients dropped within a second, followed by the packets that the server forwarded
//!   last (`kind=recent`), which hold the packet that crashed the clients if one did.
//! - `kind=connection_flood` and `kind=packet_flood`: many connections from one address or many packets from one
//!   client.

use std::collections::VecDeque;
use std::net::IpAddr;

use rustora_core::hash::HashMap;
use rustora_net::order::hex_dump;

pub const CHANNEL: &str = "malformed_packets";
/// The bytes of a malformed packet in its log line.
const DUMP_BYTES: usize = 256;
/// The forwarded packets that are kept for a mass drop, and the bytes of each.
const RECENT_PACKETS: usize = 256;
const RECENT_BYTES: usize = 64;
/// Clients that drop within `MASS_DROP_WINDOW` milliseconds count as a mass drop.
const MASS_DROP_CLIENTS: usize = 2;
const MASS_DROP_WINDOW: u64 = 1000;
/// Connections from one address within a minute, and packets from one client within a second, that count as a flood.
const CONNECTION_FLOOD: usize = 20;
const PACKET_FLOOD: u32 = 500;
/// The shortest time between two flood or mass drop reports about the same thing.
const REPORT_INTERVAL: u64 = 60_000;

/// Logs a packet of client `client` from `origin` that breaks the protocol.
pub fn log_malformed(what: &str, client: i32, origin: &str, frame: i32, data: &[u8]) {
    log::warn!(target: CHANNEL,
        "kind=malformed what=\"{what}\" client={client} origin={origin} frame={frame} length={} data={}",
        data.len(),
        hex_dump(data, DUMP_BYTES)
    );
}

struct RecentPacket {
    time: u64,
    from: i32,
    frame: i32,
    data: String,
    length: usize,
}

/// The state of the records: the last forwarded packets, the last drops, and the counters of the flood checks.
#[derive(Default)]
pub struct Forensics {
    recent: VecDeque<RecentPacket>,
    drops: VecDeque<(u64, i32)>,
    last_mass_drop: Option<u64>,
    connections: HashMap<IpAddr, (VecDeque<u64>, Option<u64>)>,
    packets: HashMap<i32, PacketCount>,
}

#[derive(Default)]
struct PacketCount {
    second: u64,
    count: u32,
    reported: Option<u64>,
}

fn due(reported: Option<u64>, now: u64) -> bool {
    reported.is_none_or(|t| now.saturating_sub(t) >= REPORT_INTERVAL)
}

impl Forensics {
    /// Keeps a packet that the server forwards from client `from` at `now`.
    pub fn forwarded(&mut self, now: u64, from: i32, frame: i32, data: &[u8]) {
        if self.recent.len() == RECENT_PACKETS {
            self.recent.pop_front();
        }
        self.recent.push_back(RecentPacket {
            time: now,
            from,
            frame,
            data: hex_dump(data, RECENT_BYTES),
            length: data.len(),
        });
    }

    /// Counts a dropped client that had joined; logs a mass drop with the recent packets.
    pub fn dropped(&mut self, now: u64, client: i32, reason: &str) {
        self.drops.push_back((now, client));
        while self.drops.front().is_some_and(|&(t, _)| now.saturating_sub(t) > MASS_DROP_WINDOW) {
            self.drops.pop_front();
        }
        if self.drops.len() < MASS_DROP_CLIENTS || !due(self.last_mass_drop, now) {
            return;
        }
        self.last_mass_drop = Some(now);
        let clients: Vec<String> = self.drops.iter().map(|(_, c)| c.to_string()).collect();
        log::warn!(target: CHANNEL,
            "kind=mass_drop clients={} window_ms={MASS_DROP_WINDOW} last_reason=\"{reason}\" recent_packets={}",
            clients.join(","),
            self.recent.len()
        );
        for p in &self.recent {
            log::warn!(target: CHANNEL,
                "kind=recent time_ms={} from={} frame={} length={} data={}",
                p.time, p.from, p.frame, p.length, p.data
            );
        }
    }

    /// Counts a connection from `ip` at `now`; logs a flood of connections from it.
    pub fn connected(&mut self, now: u64, ip: IpAddr) {
        let (times, reported) = self.connections.entry(ip).or_default();
        times.push_back(now);
        while times.front().is_some_and(|&t| now.saturating_sub(t) > 60_000) {
            times.pop_front();
        }
        if times.len() >= CONNECTION_FLOOD && due(*reported, now) {
            *reported = Some(now);
            log::warn!(target: CHANNEL, "kind=connection_flood origin={ip} connections_per_minute={}", times.len());
        }
        self.connections
            .retain(|_, (t, _)| t.back().is_some_and(|&t| now.saturating_sub(t) <= 60_000));
    }

    /// Counts a packet of client `client` at `now`; logs a flood of packets from it.
    pub fn received(&mut self, now: u64, client: i32, origin: &str) {
        let count = self.packets.entry(client).or_default();
        let second = now / 1000;
        if count.second != second {
            count.second = second;
            count.count = 0;
        }
        count.count += 1;
        if count.count == PACKET_FLOOD && due(count.reported, now) {
            count.reported = Some(now);
            log::warn!(target: CHANNEL, "kind=packet_flood client={client} origin={origin} packets_per_second={PACKET_FLOOD}");
        }
    }

    /// Forgets the packet counter of a client that left.
    pub fn forget(&mut self, client: i32) {
        self.packets.remove(&client);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_within_a_second_are_a_mass_drop() {
        let mut f = Forensics::default();
        f.dropped(1000, 1, "x");
        assert!(f.last_mass_drop.is_none());
        f.dropped(3000, 2, "x");
        assert!(f.last_mass_drop.is_none());
        f.dropped(3500, 3, "x");
        assert_eq!(f.last_mass_drop, Some(3500));
    }

    #[test]
    fn recent_packets_are_limited() {
        let mut f = Forensics::default();
        for i in 0..300 {
            f.forwarded(i, 1, 1, &[1, 2, 3]);
        }
        assert_eq!(f.recent.len(), RECENT_PACKETS);
        assert_eq!(f.recent.front().unwrap().time, 44);
    }

    #[test]
    fn floods_are_reported_once_a_minute() {
        let mut f = Forensics::default();
        let ip: IpAddr = "203.0.113.5".parse().unwrap();
        for i in 0..25 {
            f.connected(i, ip);
        }
        assert_eq!(f.connections[&ip].1, Some(19));
        for _ in 0..600 {
            f.received(5000, 7, "o");
        }
        assert_eq!(f.packets[&7].reported, Some(5000));
    }
}
