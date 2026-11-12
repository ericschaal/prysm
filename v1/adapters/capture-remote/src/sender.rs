//! The capture-host end: one TCP connection, one frame per `send`.
use crate::wire::{self, Encoding, HEADER_LEN, HELLO_LEN, Header, WireError};
use prysm_core::Frame;
use prysm_ports::{Error, FrameSink, Result};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// How long `connect` waits for the receiver to accept, and then for its
/// reply to the hello.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long one `send` may block on a receiver that stopped reading.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Sends frames to a [`RemoteSource`](crate::RemoteSource). Any failure is
/// final: the caller drops the sender and connects a new one.
#[derive(Debug)]
pub struct RemoteSender {
    stream: TcpStream,
    peer: SocketAddr,
    encoding: Encoding,
    /// Header and payload, reused so each frame is a single write.
    buffer: Vec<u8>,
}

impl RemoteSender {
    /// Connect and agree on the protocol version and a raw stream.
    ///
    /// # Errors
    /// The receiver is unreachable, refused the connection, or rejected the
    /// hello; a protocol mismatch carries a [`WireError::Version`] cause.
    pub fn connect(peer: SocketAddr) -> Result<Self> {
        let encoding = Encoding::Raw;
        let mut stream = TcpStream::connect_timeout(&peer, CONNECT_TIMEOUT)
            .map_err(|error| Error::adapter(format!("cannot connect to receiver {peer}"), error))?;
        stream
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .and_then(|()| stream.set_read_timeout(Some(CONNECT_TIMEOUT)))
            .and_then(|()| stream.set_nodelay(true))
            .map_err(|error| {
                Error::adapter(format!("cannot configure connection to {peer}"), error)
            })?;
        let mut reply = [0; HELLO_LEN];
        stream
            .write_all(&wire::hello(encoding))
            .and_then(|()| stream.read_exact(&mut reply))
            .map_err(|error| match error.kind() {
                // Version-1 receivers wait for a frame header and never reply;
                // others close on a hello they cannot parse.
                ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::UnexpectedEof => {
                    Error::adapter(
                        format!("receiver {peer} refused the stream"),
                        WireError::NoReply,
                    )
                }
                _ => Error::adapter(format!("cannot greet receiver {peer}"), error),
            })?;
        wire::accepted(&reply, encoding).map_err(|error| {
            Error::adapter(format!("receiver {peer} refused the stream"), error)
        })?;
        Ok(Self {
            stream,
            peer,
            encoding,
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
        .encode(self.encoding)
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
