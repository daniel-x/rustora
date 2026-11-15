// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! A client answers the pings of the server even while its main thread does not read the connection.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use rustora_launch::net_client::NetworkConnection;
use rustora_net::handshake::HANDSHAKE_PROTOCOL;
use rustora_net::order::order_type;

fn ping_frame(timestamp: i64) -> Vec<u8> {
    let mut out = 13i32.to_le_bytes().to_vec();
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.push(order_type::PING);
    out.extend_from_slice(&timestamp.to_le_bytes());
    out
}

#[test]
fn a_stalled_client_answers_pings() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.write_all(&HANDSHAKE_PROTOCOL.to_le_bytes()).unwrap();
        socket.write_all(&1i32.to_le_bytes()).unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(1000))).unwrap();
        let start = Instant::now();
        let mut timestamp = 0;
        while start.elapsed() < Duration::from_secs(15) {
            timestamp += 1;
            socket.write_all(&ping_frame(timestamp)).unwrap();
            let mut header = [0u8; 9];
            if socket.read_exact(&mut header).is_ok() {
                return header[8] == order_type::PING;
            }
        }
        false
    });
    let _connection = NetworkConnection::connect(&address).unwrap();
    assert!(server.join().unwrap(), "no ping answer");
}
