//! The capture-host end: one TCP connection, one frame per `send`, encoded
//! as the receiver requested.
use crate::request::StreamRequest;
use crate::wire::{self, Control, Encoding, HEADER_LEN, HELLO_LEN, Header, WireError};
use prysm_core::Frame;
use prysm_h264::Encoder;
use prysm_ports::{Error, FrameSink, Result, chain};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// How long `connect` waits for the receiver to accept, and then for its
/// reply to the hello.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long one `send` may block on a receiver that stopped reading.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Sends frames to a [`RemoteSource`](crate::RemoteSource), as its
/// [`StreamRequest`] says. Any failure is final: the caller drops the sender
/// and connects a new one. A failure whose cause is a
/// [`prysm_h264::H264Error`] is the encoder's (the receiver was told, see
/// [`RemoteSender::refuse_encoding`]); anything else is the connection's.
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
    request: StreamRequest,
    /// H.264 only; opened at the first frame.
    encoder: Option<Encoder>,
    paused: bool,
    /// Header and payload, reused so each frame is a single write.
    buffer: Vec<u8>,
}

impl RemoteSender {
    /// Connect, agree on the protocol version and read the receiver's
    /// request. The requested encoder opens at the first frame.
    ///
    /// # Errors
    /// The receiver is unreachable, refused the connection, rejected the
    /// hello (a [`WireError::Version`] cause) or sent an invalid request
    /// ([`WireError::Request`]).
    pub fn connect(peer: SocketAddr) -> Result<Self> {
        let mut stream = TcpStream::connect_timeout(&peer, CONNECT_TIMEOUT)
            .map_err(|error| Error::adapter(format!("cannot connect to receiver {peer}"), error))?;
        stream
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .and_then(|()| stream.set_read_timeout(Some(CONNECT_TIMEOUT)))
            .and_then(|()| stream.set_nodelay(true))
            .map_err(|error| {
                Error::adapter(format!("cannot configure connection to {peer}"), error)
            })?;
        stream
            .write_all(&wire::hello())
            .map_err(|error| Error::adapter(format!("cannot greet receiver {peer}"), error))?;
        let refused = |error| Error::adapter(format!("receiver {peer} refused the stream"), error);
        let mut head = [0; HELLO_LEN];
        let mut rest = [0; HELLO_LEN];
        let read = |buf: &mut [u8]| {
            (&stream)
                .read_exact(buf)
                .map_err(|error| match error.kind() {
                    // A peer that closes on a hello it cannot parse, or stays silent.
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::UnexpectedEof => {
                        refused(WireError::NoReply)
                    }
                    _ => Error::adapter(format!("cannot greet receiver {peer}"), error),
                })
        };
        // A receiver of another version answers only the first half.
        read(&mut head)?;
        wire::accepted(&head).map_err(refused)?;
        read(&mut rest)?;
        let request = wire::request(&rest).map_err(refused)?;
        Ok(Self {
            stream,
            peer,
            request,
            encoder: None,
            paused: false,
            buffer: Vec::new(),
        })
    }

    #[must_use]
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// What the receiver asked for; frames should match its size.
    #[must_use]
    pub fn request(&self) -> StreamRequest {
        self.request
    }

    /// The receiver does not want frames; `send` drops them.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Tell the receiver that this end cannot produce the requested stream
    /// (`reason`, shown to its user), and end the connection. Best effort:
    /// a receiver that is gone learns nothing.
    pub fn refuse(&mut self, reason: &str) {
        self.refuse_with(reason, 0);
    }

    /// Like [`Self::refuse`], naming the requested encoding as the cause:
    /// its encoder cannot start or failed. `send` does this on an encoder
    /// failure.
    pub fn refuse_encoding(&mut self, reason: &str) {
        self.refuse_with(reason, wire::REFUSED_ENCODING);
    }

    fn refuse_with(&mut self, reason: &str, flags: u8) {
        let _ = self.stream.write_all(&wire::refusal(reason, flags));
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
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
            .encode(self.request.encoding.into(), payload_len)
            .map_err(|error| Error::adapter("cannot send frame", error))?;
        self.buffer[..HEADER_LEN].copy_from_slice(&bytes);
        self.stream.write_all(&self.buffer).map_err(|error| {
            Error::adapter(
                format!("cannot send frame to receiver {}", self.peer),
                error,
            )
        })
    }

    /// Encode with the requested encoder, opening it first if needed.
    /// `Ok(None)`: rate control skipped the frame.
    fn encode(
        &mut self,
        frame: &Frame,
    ) -> std::result::Result<Option<bool>, prysm_h264::H264Error> {
        let encoder = match self.encoder.take() {
            Some(encoder) => encoder,
            None => Encoder::new(
                self.request
                    .encoder()
                    .ok_or(prysm_h264::H264Error::Settings("raw has no encoder"))?,
            )?,
        };
        let encoder = self.encoder.insert(encoder);
        let Some(picture) = encoder.encode(frame)? else {
            return Ok(None);
        };
        self.buffer.extend_from_slice(picture.data);
        Ok(Some(picture.keyframe))
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
        if Encoding::from(self.request.encoding) == Encoding::Raw {
            self.buffer.extend_from_slice(frame.as_slice());
            return self.write(&header, 0);
        }
        let keyframe = match self.encode(frame) {
            Ok(Some(keyframe)) => keyframe,
            // Rate control skipped it; references are unaffected.
            Ok(None) => return Ok(()),
            Err(error) => {
                let error = Error::adapter("cannot encode frame", error);
                self.refuse_encoding(&chain(&error));
                return Err(error);
            }
        };
        let len = self.buffer.len() - HEADER_LEN;
        self.write(&Header { keyframe, ..header }, len)
    }
}
