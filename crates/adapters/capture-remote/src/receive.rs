//! The receive thread: takes the connections the portal hands over, greets
//! each sender with the request and keeps only the newest frame.
use crate::link::{self, Inbound, Link, Outbound};
use crate::offer::Offer;
use crate::request::RemoteSettings;
use crate::wire::{
    self, Close, Encoding, FrameMessage, FromSender, HEADER_LEN, Header, MAX_PAYLOAD_BYTES,
    Refusal, RefusalCause, ToSender, WireError,
};
use prysm_core::FramePool;
use prysm_h264::{Decoder, H264Error};
use prysm_ports::{Accepted, Error, Receiving, Section, ValidationError, Worker, chain};
use std::io;
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::WebSocket;
use tungstenite::protocol::Role;

/// How often the receive thread checks for `close` and for a newer sender.
/// The thread leaves a connection within about one tick of `close`.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long one write to the sender (the request, a keyframe request, a
/// close) may block.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// How long, after closing a connection as stopped or replaced, the receive
/// thread waits for the sender to answer the close, discarding its frames.
pub(crate) const CLOSE_LINGER: Duration = Duration::from_secs(1);

pub(crate) struct Receiver {
    receiving: Receiving,
    worker: Worker,
    request: RemoteSettings,
    pool: FramePool,
}

/// Where the receive thread goes when it leaves a connection.
enum Next {
    /// The source closes.
    Stop,
    /// Wait for the sender to connect again.
    WaitForSender,
    /// A newer sender connected; it replaces this one.
    Switch(Accepted),
}

impl Next {
    /// What the connection being left is told: stopped, or replaced.
    fn close(&self) -> Option<Close> {
        match self {
            Self::Stop => Some(Close::Stopped),
            Self::Switch(_) => Some(Close::Replaced),
            Self::WaitForSender => None,
        }
    }
}

/// What reading a connection gave: a message, or a reason to leave it.
enum Read {
    Message(Inbound),
    Leave(Next),
}

/// What became of one H.264 picture.
enum Decoded {
    /// The frame holds it.
    Picture,
    /// Skipped while waiting for the keyframe already asked for.
    Skipped,
    /// The stream did not start with a keyframe: ask the sender for one.
    Unstarted,
    /// A picture was lost: ask the sender for a keyframe.
    Lost(H264Error),
    /// A keyframe did not decode, or no decoder starts: the stream itself
    /// is broken.
    Broken(H264Error),
}

impl Receiver {
    pub(crate) fn new(receiving: Receiving, worker: Worker, request: RemoteSettings) -> Self {
        Self {
            receiving,
            worker,
            request,
            pool: FramePool::default(),
        }
    }

    /// Serve one connection after another until stopped.
    pub(crate) fn run(mut self) {
        let mut next = Next::WaitForSender;
        loop {
            let connection = match next {
                Next::Stop => return,
                Next::Switch(connection) => connection,
                Next::WaitForSender => match self.wait_for_sender() {
                    Some(connection) => connection,
                    None => return,
                },
            };
            let mut link = match link_on(connection) {
                Ok(link) => link,
                Err(error) => {
                    next = drop_connection(&error);
                    continue;
                }
            };
            // The portal agreed the version at the upgrade and announced the
            // sender's offer; the request comes first, if offered.
            if let Some(Err(excluded)) = self.offer().map(|offer| offer.check(&self.request)) {
                next = self.fail_with_offer(&excluded);
                link.close(None);
                continue;
            }
            send(&mut link, &ToSender::Request(self.request.into()));
            next = self.serve(&mut link);
            if let Some(close) = next.close() {
                link.close(Some(close));
                link.drain(CLOSE_LINGER);
            }
        }
    }

    /// The newest connection handed over, waiting for one; `None` once stopped.
    fn wait_for_sender(&self) -> Option<Accepted> {
        loop {
            if self.worker.stopped() {
                return None;
            }
            if let Some(connection) = self.receiving.take() {
                return Some(connection);
            }
            self.worker.sleep(POLL_INTERVAL);
        }
    }

    /// Why to leave the connection now, if at all: stopped, or a newer
    /// sender connected.
    fn interruption(&self) -> Option<Next> {
        if self.worker.stopped() {
            return Some(Next::Stop);
        }
        self.receiving.take().map(Next::Switch)
    }

    /// The next message, waiting for it unless interrupted.
    fn read(&self, link: &mut Link<TcpStream>) -> Read {
        loop {
            match link.receive() {
                Ok(Some(message)) => return Read::Message(message),
                Ok(None) => {
                    if let Some(next) = self.interruption() {
                        return Read::Leave(next);
                    }
                }
                Err(error) => return Read::Leave(drop_connection(&error)),
            }
        }
    }

    /// Receive frames until the connection ends. Malformed input drops the
    /// connection (the cause is logged at debug level); the sender is
    /// expected to reconnect. A refusal or a broken H.264 stream fails the
    /// source.
    fn serve(&mut self, link: &mut Link<TcpStream>) -> Next {
        let encoding = self.request.encoding.into();
        let mut h264 = H264::default();
        loop {
            if let Some(next) = self.interruption() {
                return next;
            }
            let message = match self.read(link) {
                Read::Message(Inbound::Binary(message)) => message,
                Read::Message(Inbound::Text(text)) => match wire::decode_from_sender(&text) {
                    Ok(FromSender::Refused(refusal)) => return self.fail_with(refusal),
                    Ok(FromSender::Unknown) => continue,
                    Err(error) => return drop_connection(&error),
                },
                Read::Leave(next) => return next,
            };
            let FrameMessage { header, payload } = match wire::decode_frame(&message, encoding) {
                Ok(frame) => frame,
                Err(error) => return drop_connection(&error),
            };
            let mut writer = match self
                .pool
                .acquire(header.width, header.height, header.format)
            {
                Ok(writer) => writer,
                Err(error) => return drop_connection(&error),
            };
            let pixels = writer.as_mut_slice();
            match encoding {
                Encoding::Raw => {
                    // `decode_frame` checked the length against the header,
                    // which the pool allocated for; checked again so network
                    // input cannot panic here.
                    if pixels.len() != payload.len() {
                        return drop_connection(&WireError::Length {
                            expected: pixels.len(),
                            actual: payload.len(),
                        });
                    }
                    pixels.copy_from_slice(payload);
                }
                Encoding::H264 => match h264.decode(&header, payload, pixels) {
                    Decoded::Picture => {}
                    Decoded::Skipped => continue,
                    Decoded::Unstarted => {
                        send(link, &ToSender::Keyframe);
                        continue;
                    }
                    Decoded::Lost(error) => {
                        tracing::debug!(error = %chain(&error), "remote H.264 picture lost; requesting a keyframe");
                        send(link, &ToSender::Keyframe);
                        continue;
                    }
                    Decoded::Broken(error) => return self.fail_with_stream(error),
                },
            }
            self.worker.offer(writer.finish());
        }
    }

    /// The newest offer, if one was announced.
    fn offer(&self) -> Option<Offer> {
        Offer::parse(&self.receiving.announcement()?)
    }

    /// The sender's Prysm release, from its newest offer, for logs.
    fn sender_release(&self) -> String {
        self.offer()
            .and_then(|offer| offer.prysm().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned())
    }

    /// The sender's offer excludes the request: fail the source with the
    /// field it excludes, as a sender's refusal does. The source then stops.
    fn fail_with_offer(&self, excluded: &ValidationError) -> Next {
        tracing::warn!(
            request = %self.request,
            field = excluded.field,
            reason = excluded.reason,
            sender = self.sender_release(),
            "the remote sender does not offer the stream"
        );
        self.worker.fail(Error::Invalid(ValidationError::new(
            format!("{}.{}", Section::Capture.as_str(), excluded.field),
            format!(
                "the sender cannot stream {}: {}",
                self.request, excluded.reason
            ),
        )));
        Next::WaitForSender
    }

    /// The sender cannot produce the request: log it and fail the source with
    /// its reason, as an invalid `capture.encoding` when the encoder is the
    /// cause. The source then stops.
    fn fail_with(&self, refusal: Refusal) -> Next {
        tracing::warn!(
            request = %self.request,
            reason = refusal.reason,
            cause = ?refusal.cause,
            sender = self.sender_release(),
            "the remote sender refused the stream"
        );
        let context = format!("the sender cannot stream {}", self.request);
        self.worker.fail(match refusal.cause {
            RefusalCause::Encoding => Error::Invalid(ValidationError::new(
                format!("{}.encoding", Section::Capture.as_str()),
                format!("{context}: {}", refusal.reason),
            )),
            RefusalCause::Request => Error::adapter(context, WireError::Refused(refusal.reason)),
        });
        Next::WaitForSender
    }

    /// The sender's H.264 stream does not decode from a keyframe: it would
    /// send the same on every connection, so fail the source with the
    /// decoder's error. The source then stops.
    fn fail_with_stream(&self, error: H264Error) -> Next {
        let context = format!("the sender's stream ({}) does not decode", self.request);
        tracing::warn!(
            error = %chain(&error),
            sender = self.sender_release(),
            "{context}"
        );
        self.worker.fail(Error::adapter(context, error));
        Next::WaitForSender
    }
}

/// Leave a connection that failed or misbehaved: its cause at debug level,
/// for diagnosing a sender, and wait for it to connect again.
fn drop_connection(cause: &(dyn std::error::Error + 'static)) -> Next {
    tracing::debug!(cause = %chain(cause), "dropped the remote sender's connection");
    Next::WaitForSender
}

/// The link on a handed-over connection: its reads wake every
/// [`POLL_INTERVAL`].
fn link_on(connection: Accepted) -> io::Result<Link<TcpStream>> {
    let Accepted { socket, buffered } = connection;
    socket.set_read_timeout(Some(POLL_INTERVAL))?;
    socket.set_write_timeout(Some(WRITE_TIMEOUT))?;
    socket.set_nodelay(true)?;
    // A frame message is the longest this end reads.
    let config = link::config(HEADER_LEN + MAX_PAYLOAD_BYTES);
    Ok(Link::new(WebSocket::from_partially_read(
        socket,
        buffered,
        Role::Server,
        Some(config),
    )))
}

/// One message to the sender. Best effort, bounded by the write timeout; if
/// the sender is gone, the next read ends the connection.
fn send(link: &mut Link<TcpStream>, message: &ToSender) {
    let _ = link.send(Outbound::Text(wire::encode_to_sender(message)));
}

/// Decoding state of one H.264 connection.
#[derive(Default)]
struct H264 {
    decoder: Option<Decoder>,
    /// Pictures are skipped until a keyframe; the sender was asked for one.
    waiting: bool,
}

impl H264 {
    /// Decode one picture into `nv12`. A keyframe starts a new decoder
    /// session; a lost picture drops the decoder and skips the rest of its
    /// session until the next keyframe.
    fn decode(&mut self, header: &Header, payload: &[u8], nv12: &mut [u8]) -> Decoded {
        if header.keyframe {
            // A new encoder session: never carry references across sessions.
            self.decoder = None;
            self.waiting = false;
        } else if self.waiting {
            return Decoded::Skipped;
        } else if self.decoder.is_none() {
            self.waiting = true;
            return Decoded::Unstarted;
        }
        let decoder = match &mut self.decoder {
            Some(decoder) => decoder,
            slot => match Decoder::new() {
                Ok(decoder) => slot.insert(decoder),
                Err(error) => return Decoded::Broken(error),
            },
        };
        match decoder.decode(payload, header.width, header.height, nv12) {
            Ok(()) => Decoded::Picture,
            Err(error) if header.keyframe => Decoded::Broken(error),
            Err(error) => self.lost(error),
        }
    }

    fn lost(&mut self, error: H264Error) -> Decoded {
        self.decoder = None;
        self.waiting = true;
        Decoded::Lost(error)
    }
}
