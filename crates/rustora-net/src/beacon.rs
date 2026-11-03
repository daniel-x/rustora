// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! LAN discovery with the UDP protocol of BeaconLib, which OpenRA uses to find games on the local network.
//! A probe broadcasts `<type>`; a beacon of that type answers `<type> <port> <data>`. Strings are UTF-8 with a
//! 16 bit big endian length, the port is 16 bit big endian.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The UDP port beacons listen on.
pub const DISCOVERY_PORT: u16 = 35224;
/// The beacon type of OpenRA games.
pub const LAN_GAME_BEACON: &str = "OpenRALANGame";

const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const BEACON_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(200);

fn encode(text: &str) -> Vec<u8> {
    let mut out = (text.len() as u16).to_be_bytes().to_vec();
    out.extend_from_slice(text.as_bytes());
    out
}

fn decode(data: &[u8]) -> Option<(String, &[u8])> {
    let length = u16::from_be_bytes(data.get(..2)?.try_into().ok()?) as usize;
    let text = data.get(2..2 + length)?;
    Some((String::from_utf8_lossy(text).into_owned(), &data[2 + length..]))
}

/// The answer of a beacon of `beacon_type` to a probe.
fn encode_response(beacon_type: &str, port: u16, data: &str) -> Vec<u8> {
    let mut out = encode(beacon_type);
    out.extend_from_slice(&port.to_be_bytes());
    out.extend(encode(data));
    out
}

/// The port and data of a beacon's answer, if it has the type.
fn decode_response(beacon_type: &str, packet: &[u8]) -> Option<(u16, String)> {
    let (ty, rest) = decode(packet)?;
    if ty != beacon_type {
        return None;
    }
    let port = u16::from_be_bytes(rest.get(..2)?.try_into().ok()?);
    let (data, _) = decode(&rest[2..])?;
    Some((port, data))
}

fn beacon_socket(port: u16) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).into())?;
    Ok(socket.into())
}

/// `Beacon`: answers probes until it is stopped or dropped.
pub struct Beacon {
    data: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
}

impl Beacon {
    /// Starts answering probes on `listen_port`, normally [`DISCOVERY_PORT`], with `advertised_port` in the answers.
    pub fn start(beacon_type: &str, listen_port: u16, advertised_port: u16) -> std::io::Result<Beacon> {
        let socket = beacon_socket(listen_port)?;
        socket.set_read_timeout(Some(POLL_INTERVAL))?;
        let data = Arc::new(Mutex::new(String::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_data, thread_stop, ty) = (data.clone(), stop.clone(), beacon_type.to_string());
        std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while !thread_stop.load(Ordering::Relaxed) {
                let Ok((n, remote)) = socket.recv_from(&mut buf) else {
                    continue;
                };
                if decode(&buf[..n]).is_some_and(|(t, _)| t == ty) {
                    let text = thread_data.lock().expect("beacon data").clone();
                    let _ = socket.send_to(&encode_response(&ty, advertised_port, &text), remote);
                }
            }
        });
        Ok(Beacon { data, stop })
    }

    /// `BeaconData`: what the next probes receive.
    pub fn set_data(&self, data: &str) {
        *self.data.lock().expect("beacon data") = data.to_string();
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Beacon {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `BeaconLocation`: a beacon that answered recently.
#[derive(Clone, Debug)]
pub struct BeaconLocation {
    pub address: SocketAddr,
    pub data: String,
    pub last_advertised: Instant,
}

/// `Probe`: broadcasts periodically and collects the beacons that answer.
pub struct Probe {
    locations: Arc<Mutex<Vec<BeaconLocation>>>,
    stop: Arc<AtomicBool>,
}

impl Probe {
    /// Starts probing for `beacon_type`, sending to `target` (normally the broadcast address on [`DISCOVERY_PORT`]).
    pub fn start_to(beacon_type: &str, target: SocketAddr) -> std::io::Result<Probe> {
        let socket = beacon_socket(0)?;
        socket.set_broadcast(true)?;
        socket.set_read_timeout(Some(POLL_INTERVAL))?;
        let locations: Arc<Mutex<Vec<BeaconLocation>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_locations, thread_stop, ty) = (locations.clone(), stop.clone(), beacon_type.to_string());
        std::thread::spawn(move || {
            let request = encode(&ty);
            let mut next_probe = Instant::now();
            let mut buf = [0u8; 65536];
            while !thread_stop.load(Ordering::Relaxed) {
                if Instant::now() >= next_probe {
                    let _ = socket.send_to(&request, target);
                    next_probe = Instant::now() + PROBE_INTERVAL;
                    let mut l = thread_locations.lock().expect("beacon locations");
                    l.retain(|b| b.last_advertised.elapsed() < BEACON_TIMEOUT);
                }
                let Ok((n, remote)) = socket.recv_from(&mut buf) else {
                    continue;
                };
                if let Some((port, data)) = decode_response(&ty, &buf[..n]) {
                    let address = SocketAddr::new(remote.ip(), port);
                    let mut l = thread_locations.lock().expect("beacon locations");
                    l.retain(|b| b.address != address);
                    l.push(BeaconLocation {
                        address,
                        data,
                        last_advertised: Instant::now(),
                    });
                }
            }
        });
        Ok(Probe { locations, stop })
    }

    /// Starts probing the local network for `beacon_type`.
    pub fn start(beacon_type: &str) -> std::io::Result<Probe> {
        Probe::start_to(beacon_type, SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT)))
    }

    pub fn locations(&self) -> Vec<BeaconLocation> {
        self.locations.lock().expect("beacon locations").clone()
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_layout() {
        let r = encode_response("AB", 0x1234, "x");
        assert_eq!(r, [0, 2, b'A', b'B', 0x12, 0x34, 0, 1, b'x']);
        assert_eq!(decode_response("AB", &r), Some((0x1234, "x".to_string())));
        assert_eq!(decode_response("CD", &r), None);
        assert_eq!(decode_response("AB", &r[..3]), None);
    }

    #[test]
    fn probe_finds_beacon_and_sees_new_data() {
        let free = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let beacon = Beacon::start("rustora-test", free, 4321).unwrap();
        beacon.set_data("first");
        let target = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, free));
        let probe = Probe::start_to("rustora-test", target).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while probe.locations().is_empty() {
            assert!(Instant::now() < deadline, "no beacon found");
            std::thread::sleep(Duration::from_millis(10));
        }
        let found = &probe.locations()[0];
        assert_eq!(found.address.port(), 4321);
        assert_eq!(found.data, "first");
        beacon.set_data("second");
        while probe.locations()[0].data != "second" {
            assert!(Instant::now() < deadline, "data did not update");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
