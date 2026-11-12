//! The capture-host end: one TCP connection, one frame per `send`.
use crate::wire::{HEADER_LEN, Header};
use prysm_core::Frame;
use prysm_ports::{Error, FrameSink, Result};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// How long `connect` waits for the receiver to accept.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long one `send` may block on a receiver that stopped reading.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Sends frames to a [`RemoteSource`](crate::RemoteSource). Any failure is
/// final: the caller drops the sender and connects a new one.
#[derive(Debug)]
pub struct RemoteSender {
    stream: TcpStream,
    peer: SocketAddr,
    /// Header and payload, reused so each frame is a single write.
    buffer: Vec<u8>,
}

impl RemoteSender {
    /// # Errors
    /// The receiver is unreachable or refused the connection.
    pub fn connect(peer: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&peer, CONNECT_TIMEOUT)
            .map_err(|error| Error::adapter(format!("cannot connect to receiver {peer}"), error))?;
        stream
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .and_then(|()| stream.set_nodelay(true))
            .map_err(|error| {
                Error::adapter(format!("cannot configure connection to {peer}"), error)
            })?;
        Ok(Self {
            stream,
            peer,
            buffer: Vec::new(),
        })
    }

    #[must_use]
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }
}

impl FrameSink for RemoteSender {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        let header = Header {
            width: frame.width(),
            height: frame.height(),
            format: frame.format(),
        }
        .encode()
        .map_err(|error| Error::adapter("cannot send frame", error))?;
        self.buffer.clear();
        self.buffer.reserve(HEADER_LEN + frame.len());
        self.buffer.extend_from_slice(&header);
        self.buffer.extend_from_slice(frame.as_slice());
        self.stream.write_all(&self.buffer).map_err(|error| {
            Error::adapter(
                format!("cannot send frame to receiver {}", self.peer),
                error,
            )
        })
    }
}
