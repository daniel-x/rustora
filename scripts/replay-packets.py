#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) The rustora authors.
"""Sends a capture of malformed packets (rustora/Logs/malformed_packets/*.bin of a server) to a server again.

A capture holds the frames that a client sent after the server's greeting: each frame is the length of frame and data
(int32), the frame number (int32) and the data. This script connects, reads the greeting (protocol version and client
index), sends the capture as it is, and prints the frames that come back until the server closes the connection or
--wait seconds pass.

Usage: scripts/replay-packets.py CAPTURE HOST:PORT [--wait SECONDS]

Only use it against your own servers.
"""

import argparse
import socket
import struct
import time


def read_exact(sock, count):
    data = b""
    while len(data) < count:
        chunk = sock.recv(count - len(data))
        if not chunk:
            return None
        data += chunk
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("capture")
    parser.add_argument("address", help="HOST:PORT")
    parser.add_argument("--wait", type=float, default=5.0)
    args = parser.parse_args()

    host, port = args.address.rsplit(":", 1)
    with open(args.capture, "rb") as f:
        capture = f.read()
    sock = socket.create_connection((host.strip("[]"), int(port)), timeout=args.wait)
    greeting = read_exact(sock, 8)
    if greeting is None:
        print("the server closed the connection before its greeting")
        return
    protocol, index = struct.unpack("<ii", greeting)
    print(f"greeting: protocol {protocol}, client {index}")
    sock.sendall(capture)
    print(f"sent {len(capture)} bytes")
    deadline = time.monotonic() + args.wait
    while time.monotonic() < deadline:
        sock.settimeout(max(0.1, deadline - time.monotonic()))
        try:
            header = read_exact(sock, 8)
        except socket.timeout:
            break
        if header is None:
            print("the server closed the connection")
            return
        length, client = struct.unpack("<ii", header)
        body = read_exact(sock, length) or b""
        frame = struct.unpack("<i", body[:4])[0] if len(body) >= 4 else None
        print(f"from client {client}: frame {frame}, {len(body) - 4} bytes: {body[4:36].hex()}")
    print("the connection is still open")


if __name__ == "__main__":
    main()
