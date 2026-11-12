//! The capture-host end: one TCP connection, one frame per `send`, encoded
//! as the stream says.
use crate::wire::{self, Control, Encoding, HEADER_LEN, HELLO_LEN, Header, WireError};
use prysm_core::Frame;
use prysm_h264::{Encoder, EncoderSettings};
use prysm_ports::{Error, FrameSink, Result};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// How long `connect` waits for the receiver to accept, and then for its
/// reply to the hello.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long one `send` may block on a receiver that stopped reading.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// What a connection carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// Uncompressed frames.
    Raw,
    /// H.264 from exactly this encoder; it never switches backend.
    H264(EncoderSettings),
}

impl Stream {
    #[must_use]
    pub fn encoding(self) -> Encoding {
        match self {
            Self::Raw => Encoding::Raw,
            Self::H264(_) => Encoding::H264,
        }
    }
}

/// Sends frames to a [`RemoteSource`](crate::RemoteSource). Any failure is
/// final: the caller drops the sender and connects a new one. A failure
/// whose cause is a [`prysm_h264::H264Error`] is the encoder's; anything
/// else is the connection's.
///
/// The receiver can pause the stream (frames are dropped unencoded until it
/// resumes) and ask for a keyframe; each `send` reads those requests
/// without blocking. A paused or resumed H.264 stream restarts its encoder
/// session, so the first picture after it is a keyframe, and a paused
/// hardware encoder is released.
#[derive(Debug)]
pub struct RemoteSender {
    stream: TcpStream,
    peer: SocketAddr,
    encoder: Option<Encoder>,
    paused: bool,
    /// Header and payload, reused so each frame is a single write.
    buffer: Vec<u8>,
}

impl RemoteSender {
    /// Connect and agree on the protocol version and the stream's encoding.
    /// An H.264 encoder is checked first but opens only at the first frame.
    ///
    /// # Errors
    /// Invalid encoder settings (an [`H264Error`](prysm_h264::H264Error)
    /// cause), or the receiver is unreachable, refused the connection, or
    /// rejected the hello: a [`WireError::Version`] or
    /// [`WireError::Encoding`] cause.
    pub fn connect(peer: SocketAddr, kind: Stream) -> Result<Self> {
        let encoder = match kind {
            Stream::Raw => None,
            Stream::H264(settings) => Some(
                Encoder::new(settings)
                    .map_err(|error| Error::adapter("cannot set up the H.264 encoder", error))?,
            ),
        };
        let encoding = kind.encoding();
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
            encoder,
            paused: false,
            buffer: Vec::new(),
        })
    }

    #[must_use]
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    #[must_use]
    pub fn encoding(&self) -> Encoding {
        if self.encoder.is_some() {
            Encoding::H264
        } else {
            Encoding::Raw
        }
    }

    /// The receiver does not want frames; `send` drops them.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Apply the receiver's pending control messages without blocking.
    fn poll_control(&mut self) -> Result<()> {
        let peer = self.peer;
        let configure = |error| Error::adapter(format!("cannot poll receiver {peer}"), error);
        self.stream.set_nonblocking(true).map_err(configure)?;
        let mut bytes = [0; 16];
        let read = loop {
            match self.stream.read(&mut bytes) {
                Ok(0) => {
                    break Err(Error::adapter(
                        format!("receiver {peer} ended the stream"),
                        WireError::Closed,
                    ));
                }
                Ok(n) => {
                    if let Err(error) = bytes[..n]
                        .iter()
                        .try_for_each(|&byte| Control::from_code(byte).map(|c| self.apply(c)))
                    {
                        break Err(Error::adapter(format!("receiver {peer} misbehaved"), error));
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break Ok(()),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => {
                    break Err(Error::adapter(
                        format!("receiver {peer} ended the stream"),
                        error,
                    ));
                }
            }
        };
        self.stream.set_nonblocking(false).map_err(configure)?;
        read
    }

    fn apply(&mut self, control: Control) {
        self.paused = match control {
            Control::Pause => true,
            Control::Resume => false,
            Control::Keyframe => self.paused,
        };
        // Pause releases the encoder; resume and keyframe start a new session.
        if let Some(encoder) = &mut self.encoder {
            encoder.restart();
        }
    }

    /// One header and payload in a single write.
    fn write(&mut self, header: &Header, payload_len: usize) -> Result<()> {
        let bytes = header
            .encode(self.encoding(), payload_len)
            .map_err(|error| Error::adapter("cannot send frame", error))?;
        self.buffer[..HEADER_LEN].copy_from_slice(&bytes);
        self.stream.write_all(&self.buffer).map_err(|error| {
            Error::adapter(
                format!("cannot send frame to receiver {}", self.peer),
                error,
            )
        })
    }
}

impl FrameSink for RemoteSender {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.poll_control()?;
        if self.paused {
            return Ok(());
        }
        let header = Header {
            width: frame.width(),
            height: frame.height(),
            format: frame.format(),
            keyframe: false,
        };
        // The buffer is header space followed by the payload.
        self.buffer.clear();
        self.buffer.resize(HEADER_LEN, 0);
        let Some(encoder) = &mut self.encoder else {
            self.buffer.extend_from_slice(frame.as_slice());
            return self.write(&header, 0);
        };
        let picture = encoder
            .encode(frame)
            .map_err(|error| Error::adapter("cannot encode frame", error))?;
        let Some(picture) = picture else {
            // Rate control skipped it; references are unaffected.
            return Ok(());
        };
        let header = Header {
            keyframe: picture.keyframe,
            ..header
        };
        self.buffer.extend_from_slice(picture.data);
        let len = picture.data.len();
        self.write(&header, len)
    }
}
