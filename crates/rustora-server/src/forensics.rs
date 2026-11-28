// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Records for the investigation of attacks, a rustora addition. Everything goes to the `malformed_packets` log
//! channel as `key=value` lines:
//!
//! - `kind=malformed`: a packet that breaks the protocol, with a hex dump of its first bytes. The whole packet, and for
//!   a connection that had not finished the handshake everything it sent, goes to a file in
//!   `Logs/malformed_packets/` that the line names (`file=`), as the frames the client sent: length, frame and data.
//!   `scripts/replay-packets.py` of the repository sends such a file to a server again.
//! - `kind=mass_drop`: several clients dropped within a second, followed by the packets that the server forwarded
//!   last (`kind=recent`), which hold the packet that crashed the clients if one did.
//! - `kind=connection_flood` and `kind=packet_flood`: many connections from one address or many packets from one
//!   client.

use std::collections::VecDeque;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
const CONNECTION_FLOOD: usize = 30;
/// The flood limit of the master server, which tests the port after each advertisement. A server advertises at most
/// once a second, so the master server stays at about 60 connections a minute.
const MASTER_SERVER_CONNECTION_FLOOD: usize = 120;
/// Milliseconds between two lookups of the addresses of the master server.
const MASTER_SERVER_LOOKUP_INTERVAL: u64 = 3_600_000;
const PACKET_FLOOD: u32 = 500;
/// The most bytes that are kept of a connection before its handshake, and the most bytes of all capture files.
pub const MAX_CAPTURE_BYTES: usize = 1024 * 1024;
const MAX_CAPTURES_BYTES: u64 = 100 * 1024 * 1024;
/// The shortest time between two flood or mass drop reports about the same thing.
const REPORT_INTERVAL: u64 = 60_000;

/// Logs a packet of client `client` from `origin` that breaks the protocol; `capture` names the file with its bytes.
pub fn log_malformed(what: &str, client: i32, origin: &str, frame: i32, data: &[u8], capture: Option<&Path>) {
    let file = capture
        .and_then(|p| p.file_name())
        .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
    log::warn!(target: CHANNEL,
        "kind=malformed what=\"{what}\" client={client} origin={origin} frame={frame} length={} data={} file={file}",
        data.len(),
        hex_dump(data, DUMP_BYTES)
    );
}

/// A frame as the client sent it: the length of frame and data, the frame, and the data.
pub fn frame_bytes(frame: i32, data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as i32 + 4).to_le_bytes().to_vec();
    out.extend_from_slice(&frame.to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// Writes `bytes` of client `client` from `origin` to a new file in `dir`, after deleting the oldest files so that all
/// stay within `MAX_CAPTURES_BYTES`. The names start with the time, so they sort by age.
pub fn save_capture(dir: &Path, client: i32, origin: &str, bytes: &[u8]) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let mut files: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.len())))
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|(_, size)| size).sum::<u64>() + bytes.len() as u64;
    for (path, size) in &files {
        if total <= MAX_CAPTURES_BYTES {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total -= size;
        }
    }
    let origin: String = origin
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' })
        .collect();
    let name = format!("{}-client{client}-{origin}.bin", chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ"));
    let path = dir.join(name);
    std::fs::write(&path, bytes).ok()?;
    Some(path)
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
    /// The addresses of the master server, filled by a thread.
    master_server: Arc<Mutex<Vec<IpAddr>>>,
    master_server_lookup: Option<u64>,
}

/// The host of `url`, like `master.openra.net` of `https://master.openra.net/ping`.
fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => host.split(':').next()?,
    };
    (!host.is_empty()).then_some(host)
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

    /// Looks up the addresses of the host of the master server URL `url` on a thread, at most every
    /// `MASTER_SERVER_LOOKUP_INTERVAL`, so that the flood check knows the master server.
    pub fn look_up_master_server(&mut self, now: u64, url: &str) {
        if self
            .master_server_lookup
            .is_some_and(|t| now.saturating_sub(t) < MASTER_SERVER_LOOKUP_INTERVAL)
        {
            return;
        }
        self.master_server_lookup = Some(now);
        let Some(host) = url_host(url).map(str::to_string) else {
            return;
        };
        let addresses = self.master_server.clone();
        std::thread::spawn(move || {
            if let Ok(found) = (host.as_str(), 443).to_socket_addrs() {
                let found: Vec<IpAddr> = found.map(|a| a.ip().to_canonical()).collect();
                if !found.is_empty()
                    && let Ok(mut a) = addresses.lock()
                {
                    *a = found;
                }
            }
        });
    }

    fn is_master_server(&self, ip: IpAddr) -> bool {
        self.master_server.lock().is_ok_and(|a| a.contains(&ip))
    }

    /// Counts a connection from `ip` at `now`; logs a flood of connections from it.
    pub fn connected(&mut self, now: u64, ip: IpAddr) {
        let limit = if self.is_master_server(ip) {
            MASTER_SERVER_CONNECTION_FLOOD
        } else {
            CONNECTION_FLOOD
        };
        let (times, reported) = self.connections.entry(ip).or_default();
        times.push_back(now);
        while times.front().is_some_and(|&t| now.saturating_sub(t) > 60_000) {
            times.pop_front();
        }
        if times.len() >= limit && due(*reported, now) {
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
        for i in 0..40 {
            f.connected(i, ip);
        }
        assert_eq!(f.connections[&ip].1, Some(29));
        let master: IpAddr = "195.16.73.100".parse().unwrap();
        f.master_server.lock().unwrap().push(master);
        for i in 0..130 {
            f.connected(i, master);
        }
        assert_eq!(f.connections[&master].1, Some(119));
        for _ in 0..600 {
            f.received(5000, 7, "o");
        }
        assert_eq!(f.packets[&7].reported, Some(5000));
    }

    #[test]
    fn captures_stay_within_the_limit() {
        let dir = std::env::temp_dir().join(format!("rustora-capture-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let chunk = vec![0u8; MAX_CAPTURE_BYTES];
        for i in 0..105 {
            save_capture(&dir, i, "[2001:db8::1]:5", &chunk).unwrap();
        }
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
        let total: u64 = files.iter().map(|p| std::fs::metadata(p).unwrap().len()).sum();
        assert!(total <= MAX_CAPTURES_BYTES);
        assert!(
            files
                .iter()
                .any(|p| p.to_string_lossy().ends_with("-client104-_2001_db8__1__5.bin"))
        );
        assert!(!files.iter().any(|p| p.to_string_lossy().contains("-client0-")));
        assert_eq!(frame_bytes(7, &[1, 2]), [6, 0, 0, 0, 7, 0, 0, 0, 1, 2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_hosts() {
        assert_eq!(url_host("https://master.openra.net/ping"), Some("master.openra.net"));
        assert_eq!(url_host("http://127.0.0.1:8080/ping?x=1"), Some("127.0.0.1"));
        assert_eq!(url_host("http://[::1]:80/"), Some("::1"));
        assert_eq!(url_host("https:///"), None);
    }
}
