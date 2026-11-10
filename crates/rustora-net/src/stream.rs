// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The byte streams that carry the game protocol between client and server: a TCP socket, or an in-memory pipe
//! for a server that runs inside the client process.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Condvar, Mutex};

/// A bidirectional byte stream. Clones share the stream, so one thread can read while another writes.
pub trait ByteStream: Read + Write + Send {
    fn try_clone_stream(&self) -> std::io::Result<Box<dyn ByteStream>>;
    /// Closes both directions. A blocked read on either end returns end of file.
    fn shutdown_stream(&self);
}

impl ByteStream for TcpStream {
    fn try_clone_stream(&self) -> std::io::Result<Box<dyn ByteStream>> {
        Ok(Box::new(self.try_clone()?))
    }

    fn shutdown_stream(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

#[derive(Default)]
struct Buffer {
    data: VecDeque<u8>,
    closed: bool,
}

/// One direction of a pipe.
#[derive(Default)]
struct Channel {
    buffer: Mutex<Buffer>,
    ready: Condvar,
}

impl Channel {
    fn close(&self) {
        self.buffer.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.ready.notify_all();
    }
}

/// One end of an in-memory pipe.
pub struct PipeEnd {
    incoming: Arc<Channel>,
    outgoing: Arc<Channel>,
}

/// Two connected ends of an in-memory pipe: what one end writes, the other reads.
pub fn pipe() -> (PipeEnd, PipeEnd) {
    let (a, b) = (Arc::new(Channel::default()), Arc::new(Channel::default()));
    (
        PipeEnd {
            incoming: a.clone(),
            outgoing: b.clone(),
        },
        PipeEnd { incoming: b, outgoing: a },
    )
}

impl Read for PipeEnd {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut buffer = self.incoming.buffer.lock().unwrap_or_else(|e| e.into_inner());
        while buffer.data.is_empty() && !buffer.closed {
            buffer = self.incoming.ready.wait(buffer).unwrap_or_else(|e| e.into_inner());
        }
        let n = buf.len().min(buffer.data.len());
        for (target, byte) in buf.iter_mut().zip(buffer.data.drain(..n)) {
            *target = byte;
        }
        Ok(n)
    }
}

impl Write for PipeEnd {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut buffer = self.outgoing.buffer.lock().unwrap_or_else(|e| e.into_inner());
        if buffer.closed {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        buffer.data.extend(buf);
        self.outgoing.ready.notify_all();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl ByteStream for PipeEnd {
    fn try_clone_stream(&self) -> std::io::Result<Box<dyn ByteStream>> {
        Ok(Box::new(PipeEnd {
            incoming: self.incoming.clone(),
            outgoing: self.outgoing.clone(),
        }))
    }

    fn shutdown_stream(&self) {
        self.incoming.close();
        self.outgoing.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_carries_bytes_both_ways_and_ends_on_shutdown() {
        let (mut a, mut b) = pipe();
        a.write_all(b"ping").unwrap();
        let mut buf = [0u8; 4];
        b.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 4];
            a.read_exact(&mut buf).map(|()| buf)
        });
        b.write_all(b"pong").unwrap();
        assert_eq!(&reader.join().unwrap().unwrap(), b"pong");
        let c = b.try_clone_stream().unwrap();
        c.shutdown_stream();
        assert_eq!(b.read(&mut buf).unwrap(), 0);
        assert!(b.write(b"x").is_err());
    }
}
