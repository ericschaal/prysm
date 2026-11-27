//! The capture-host end: one WebSocket to Prysm's portal, one frame per
//! `send`, encoded as the receiver requested.
use crate::link::{self, Link, LinkError, Message};
use crate::offer::Offer;
use crate::receive;
use crate::request::RemoteSettings;
use crate::wire::{
    self, Encoding, FromReceiver, Header, MAX_TEXT_BYTES, Refusal, RefusalCause, WireError,
};
use bytes::{Bytes, BytesMut};
use prysm_core::Frame;
use prysm_h264::Encoder;
use prysm_ports::{
    CAPTURE_OFFER_HEADER, CAPTURE_PROTOCOL, Error, FrameSink, Hangup, Registration, Result,
    Stopping, chain,
};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::{HeaderValue, StatusCode, Uri, header};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Connector, HandshakeError};

/// How long [`RemoteSender::connect`] waits on the network in all: for the
/// connection, the upgrade (with TLS) and the receiver's request, however
/// slowly the receiver answers. The request can wait for the receiver to
/// close a connection this one replaces (its close write, its linger and one
/// read); 200 ms more leave room for the connection and the upgrade.
pub const CONNECT_DEADLINE: Duration = receive::WRITE_TIMEOUT
    .saturating_add(receive::CLOSE_LINGER)
    .saturating_add(receive::POLL_INTERVAL)
    .saturating_add(Duration::from_millis(200));
/// How long one `send` may block on a receiver that reads slowly or not at
/// all: one deadline for the whole message, however many writes it takes.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// The link's socket. While a deadline is set (the connect, or one blocking
/// send), every read and write waits at most until then, so a peer that
/// answers or reads a byte at a time cannot stretch it.
struct Socket {
    stream: TcpStream,
    deadline: Option<Instant>,
    /// The sender's [`Hangup`], which a cut shuts this socket down through.
    hangup: Hangup,
}

impl Socket {
    /// Bound the next blocking call by the deadline, if one is set, through
    /// `set` (the read or the write timeout).
    fn bound(
        &self,
        set: fn(&TcpStream, Option<Duration>) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        let Some(deadline) = self.deadline else {
            return Ok(());
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        set(&self.stream, Some(left))
    }
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.bound(TcpStream::set_read_timeout)?;
        self.stream.read(buf)
    }
}

impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bound(TcpStream::set_write_timeout)?;
        if self.deadline.is_some() {
            self.hangup.waiting();
        }
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

/// Why [`RemoteSender::deliver`] failed. Either way the sender is finished:
/// the caller drops it and connects a new one.
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// This end cannot produce the request: the requested encoder could not
    /// start or failed, or a frame does not fit the stream. The receiver
    /// was told (see [`RemoteSender::refuse_encoding`]).
    #[error(transparent)]
    Refused(Error),
    /// The connection to the receiver failed or was closed; a
    /// [`WireError::Closed`] cause says why the receiver closed it.
    #[error(transparent)]
    Connection(#[from] Error),
}

impl From<SendError> for Error {
    fn from(error: SendError) -> Self {
        match error {
            SendError::Refused(error) | SendError::Connection(error) => error,
        }
    }
}

/// Sends frames to a [`RemoteSource`](crate::RemoteSource) through Prysm's
/// portal, as its [`RemoteSettings`] say. Any failure is final: the caller
/// drops the sender and connects a new one. [`RemoteSender::deliver`] tells
/// a refusal from a connection failure; [`FrameSink::send`] does not.
///
/// The receiver can ask for a keyframe, which starts a new H.264 encoder
/// session; each `send` and `tick` first reads the receiver's messages,
/// without waiting for any, and so learns when the receiver closes.
pub struct RemoteSender {
    /// Reads never wait (the socket is non-blocking once connected);
    /// writes wait out a full send buffer within the write timeout.
    link: Link<MaybeTlsStream<Socket>>,
    /// The [`Hangup`]'s handle on the socket, dropped with the sender.
    registration: Registration,
    /// The capture stream's URL, for messages.
    url: Uri,
    outgoing: Outgoing,
}

/// Builds a complete frame message for the negotiated request. The encoder
/// opens on the first H.264 frame. Once the WebSocket releases the previous
/// message, its allocation can be reclaimed for the next one.
struct Outgoing {
    request: RemoteSettings,
    encoder: Option<Encoder>,
    bytes: BytesMut,
}

impl Outgoing {
    fn new(request: RemoteSettings) -> Self {
        Self {
            request,
            encoder: None,
            bytes: BytesMut::new(),
        }
    }

    /// None when rate control skipped a picture.
    fn frame(&mut self, frame: &Frame) -> Result<Option<Bytes>> {
        let (payload, keyframe) = if let Some(settings) = self.request.encoder() {
            let failed = |error| Error::adapter("cannot encode frame", error);
            let encoder = match &mut self.encoder {
                Some(encoder) => encoder,
                slot => slot.insert(Encoder::new(settings).map_err(failed)?),
            };
            let Some(picture) = encoder.encode(frame).map_err(failed)? else {
                return Ok(None);
            };
            (picture.data, picture.keyframe)
        } else {
            (frame.as_slice(), false)
        };
        let header = Header {
            width: frame.width(),
            height: frame.height(),
            format: frame.format(),
            keyframe,
        }
        .encode(self.request.encoding.into(), payload.len())
        .map_err(|error| Error::adapter("cannot send frame", error))?;
        self.bytes.reserve(header.len() + payload.len());
        self.bytes.extend_from_slice(&header);
        self.bytes.extend_from_slice(payload);
        Ok(Some(self.bytes.split().freeze()))
    }

    fn restart(&mut self) {
        if let Some(encoder) = &mut self.encoder {
            encoder.restart();
        }
    }
}

impl std::fmt::Debug for RemoteSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteSender")
            .field("url", &self.url)
            .field("request", &self.request())
            .finish_non_exhaustive()
    }
}

impl RemoteSender {
    /// Open the capture stream at `url` (`ws://`, or `wss://` verified
    /// against the system's certificate authorities) by connecting to
    /// `address`, where the URL's host resolved to (the caller resolves, so
    /// it bounds the lookup), with this end's protocol version as the
    /// subprotocol and `offer` in the [`CAPTURE_OFFER_HEADER`] (Prysm keeps
    /// it even when it refuses the stream); then read the receiver's
    /// request. A request this end cannot parse, that fails validation or
    /// that `offer` excludes is refused to the receiver. The requested
    /// encoder opens at the first frame.
    ///
    /// # Errors
    /// Prysm is unreachable, failed TLS verification, has no receiving
    /// source ([`WireError::Unavailable`] `503`), speaks another version
    /// ([`WireError::Version`]), sent no valid request, or closed the
    /// stream with a code before it ([`WireError::Closed`]), all within
    /// [`CONNECT_DEADLINE`].
    pub fn connect(url: &Uri, address: SocketAddr, offer: &Offer) -> Result<Self> {
        Self::connect_until(url, address, offer, &Hangup::default())
    }

    /// [`Self::connect`], ended at once (while connecting, or later while
    /// the sender is used) when `hangup` is cut.
    ///
    /// # Errors
    /// As [`Self::connect`], or `hangup` was cut.
    pub fn connect_until(
        url: &Uri,
        address: SocketAddr,
        offer: &Offer,
        hangup: &Hangup,
    ) -> Result<Self> {
        let deadline = Instant::now() + CONNECT_DEADLINE;
        Self::connect_by(url, address, offer, hangup, deadline)
    }

    /// [`Self::open`], failing with [`Stopping`] as the cause once `hangup`
    /// is cut, whatever the socket said.
    fn connect_by(
        url: &Uri,
        address: SocketAddr,
        offer: &Offer,
        hangup: &Hangup,
        deadline: Instant,
    ) -> Result<Self> {
        Self::open(url, address, offer, hangup, deadline).map_err(|error| {
            if hangup.is_cut() {
                Error::adapter(format!("cannot connect to {url}"), Stopping)
            } else {
                error
            }
        })
    }

    fn open(
        url: &Uri,
        address: SocketAddr,
        offer: &Offer,
        hangup: &Hangup,
        deadline: Instant,
    ) -> Result<Self> {
        let failed =
            |error: std::io::Error| Error::adapter(format!("cannot connect to {url}"), error);
        let (stream, registration) = hangup
            .connect(address, deadline)
            .map_err(|error| Error::adapter(format!("cannot connect to {url}"), error))?;
        stream.set_nodelay(true).map_err(failed)?;
        let socket = Socket {
            stream,
            deadline: Some(deadline),
            hangup: hangup.clone(),
        };
        let mut link = upgrade(url, socket, offer)?;
        let request = read_request(&mut link, deadline)
            .and_then(|request| {
                offer.check(&request).map_err(WireError::Request)?;
                Ok(request)
            })
            .map_err(|error| {
                if let LinkError::Wire(WireError::Json(_) | WireError::Request(_)) = &error {
                    refuse(&mut link, RefusalCause::Request, &chain(&error));
                }
                error.context(format!("{url} sent no valid request"))
            })?;
        // From here the link's reads never wait (the receiver's messages are
        // read before each frame), and a send waits out a full send buffer
        // for at most the write timeout.
        let socket = inner(&mut link);
        socket.deadline = None;
        socket
            .stream
            .set_nonblocking(true)
            .map_err(|error| Error::adapter(format!("cannot configure {url}"), error))?;
        Ok(Self {
            link,
            registration,
            url: url.clone(),
            outgoing: Outgoing::new(request),
        })
    }

    /// What the receiver asked for; frames should match its size.
    #[must_use]
    pub fn request(&self) -> RemoteSettings {
        self.outgoing.request
    }

    /// Read the receiver's pending messages, then send one frame, encoded
    /// as requested.
    ///
    /// # Errors
    /// [`SendError::Refused`] when the requested encoder cannot start or
    /// failed, or the frame does not fit the stream (the receiver is told),
    /// else [`SendError::Connection`].
    pub fn deliver(&mut self, frame: &Frame) -> Result<(), SendError> {
        self.read_pending()?;
        if let Some(message) = self
            .outgoing
            .frame(frame)
            .map_err(|error| self.refused(error))?
        {
            self.send_message(Message::Binary(message))
                .map_err(|error| self.failed_send(error))?;
        }
        Ok(())
    }

    /// Tell the receiver that this end cannot produce the requested stream
    /// (`reason`, shown to its user), and close the connection. Best effort:
    /// a receiver that is gone learns nothing.
    pub fn refuse(&mut self, reason: &str) {
        self.refuse_with(reason, RefusalCause::Request);
    }

    /// Like [`Self::refuse`], naming the requested encoding as the cause:
    /// its encoder cannot start or failed. `send` does this on an encoder
    /// failure.
    pub fn refuse_encoding(&mut self, reason: &str) {
        self.refuse_with(reason, RefusalCause::Encoding);
    }

    fn refuse_with(&mut self, reason: &str, cause: RefusalCause) {
        // Within the write timeout, rather than giving up on a full buffer.
        let _ = self.set_blocking(true);
        refuse(&mut self.link, cause, reason);
        let _ = self.set_blocking(false);
    }

    /// Refuse the request for `error`, naming the encoding of an H.264
    /// stream (its encoder produced the frame) and the request otherwise.
    fn refused(&mut self, error: Error) -> SendError {
        let cause = match Encoding::from(self.request().encoding) {
            Encoding::H264 => RefusalCause::Encoding,
            Encoding::Raw => RefusalCause::Request,
        };
        self.refuse_with(&chain(&error), cause);
        SendError::Refused(error)
    }

    /// Drain pending receiver messages. Repeated keyframe requests are valid.
    /// A second stream request is a protocol error.
    fn read_pending(&mut self) -> Result<()> {
        while let Some(message) = self.link.receive().map_err(|error| self.failed(error))? {
            let command = match message {
                Message::Text(text) => wire::decode_from_receiver(&text),
                Message::Binary(_) => Err(WireError::Unexpected),
            }
            .map_err(|error| self.failed(error.into()))?;
            match command {
                FromReceiver::Keyframe => self.outgoing.restart(),
                FromReceiver::Unknown => {}
                FromReceiver::Request(_) => return Err(self.failed(WireError::Unexpected.into())),
            }
        }
        Ok(())
    }

    /// Send one message. A full send buffer is waited out, blocking for at
    /// most the write timeout in all.
    fn send_message(&mut self, message: Message) -> Result<(), LinkError> {
        match self.link.send(message) {
            Err(error) if error.would_block() => {
                self.set_blocking(true)?;
                let flushed = self.link.flush();
                let restored = self.set_blocking(false);
                flushed.and(restored)
            }
            sent => sent,
        }
    }

    /// Switch the socket to blocking, its waits ending [`WRITE_TIMEOUT`]
    /// from now, or back to non-blocking.
    fn set_blocking(&mut self, blocking: bool) -> Result<(), LinkError> {
        let socket = inner(&mut self.link);
        socket.deadline = blocking.then(|| Instant::now() + WRITE_TIMEOUT);
        socket
            .stream
            .set_nonblocking(!blocking)
            .map_err(|error| LinkError::Socket(error.into()))
    }

    /// A write failed. A receiver that closes the connection races the
    /// write, so a close it sent first, if one arrived, is the cause.
    fn failed_send(&mut self, error: LinkError) -> Error {
        loop {
            match self.link.receive() {
                Ok(Some(_)) => {}
                Err(closed @ LinkError::Wire(WireError::Closed(Some(_)))) => {
                    return self.failed(closed);
                }
                Ok(None) | Err(_) => return self.failed(error),
            }
        }
    }

    /// The stream failed with `error`, or with [`Stopping`] once its
    /// [`Hangup`] is cut, whatever the socket said.
    fn failed(&self, error: LinkError) -> Error {
        let context = format!("the stream to {} failed", self.url);
        if self.registration.hangup().is_cut() {
            return Error::adapter(context, Stopping);
        }
        error.context(context)
    }
}

impl FrameSink for RemoteSender {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        Ok(self.deliver(frame)?)
    }

    /// No frame to send: read the receiver's pending messages, so a sender
    /// whose capture is stopped still learns that the receiver closed.
    fn tick(&mut self) -> Result<()> {
        self.read_pending()
    }
}

/// The link's own socket, under TLS or not.
fn inner(link: &mut Link<MaybeTlsStream<Socket>>) -> &mut Socket {
    match link.socket_mut() {
        MaybeTlsStream::Plain(socket) => socket,
        MaybeTlsStream::Rustls(tls) => &mut tls.sock,
        _ => unreachable!("`upgrade` opens plain or rustls streams only"),
    }
}

/// Upgrade `socket` to a WebSocket at `url` with this end's subprotocol and
/// `offer`, with TLS first for `wss://`, by `socket`'s deadline.
fn upgrade(url: &Uri, socket: Socket, offer: &Offer) -> Result<Link<MaybeTlsStream<Socket>>> {
    let mut request = url
        .into_client_request()
        .map_err(|error| Error::adapter(format!("invalid capture stream URL {url}"), error))?;
    let offer = HeaderValue::from_str(&offer.header())
        .map_err(|error| Error::adapter("cannot send the capture offer", error))?;
    let headers = request.headers_mut();
    headers.insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(CAPTURE_PROTOCOL),
    );
    headers.insert(CAPTURE_OFFER_HEADER, offer);
    let connector = match url.scheme_str() {
        Some("wss") => Some(Connector::Rustls(authorities().map_err(|untrusted| {
            Error::adapter(format!("cannot open {url}"), untrusted)
        })?)),
        _ => None,
    };
    // A request is the longest message this end reads.
    let config = Some(link::config(MAX_TEXT_BYTES));
    let mut handshake = tungstenite::client_tls_with_config(request, socket, config, connector);
    loop {
        return match handshake {
            Ok((socket, _)) => Ok(Link::new(socket)),
            Err(HandshakeError::Interrupted(waiting)) => {
                handshake = waiting.handshake();
                continue;
            }
            Err(HandshakeError::Failure(tungstenite::Error::Http(response))) => {
                let status = response.status();
                // The portal lists the versions it speaks with its refusal.
                let error = if status == StatusCode::UPGRADE_REQUIRED {
                    let theirs = response.headers().get(header::SEC_WEBSOCKET_PROTOCOL);
                    wire::version_error(theirs.and_then(|value| value.to_str().ok()))
                } else {
                    WireError::Unavailable(status.as_u16())
                };
                Err(Error::adapter(format!("{url} did not open"), error))
            }
            Err(HandshakeError::Failure(error)) => match untrusted(&error) {
                Some(untrusted) => Err(Error::adapter(format!("cannot open {url}"), untrusted)),
                None => Err(Error::adapter(format!("cannot open {url}"), error)),
            },
        };
    }
}

/// The certificate refusal in a failed handshake: rustls reports it
/// through the stream's I/O error.
fn untrusted(error: &tungstenite::Error) -> Option<WireError> {
    let tungstenite::Error::Io(io) = error else {
        return None;
    };
    let tls = io.get_ref()?.downcast_ref::<rustls::Error>()?;
    matches!(tls, rustls::Error::InvalidCertificate(_))
        .then(|| WireError::Untrusted(tls.to_string()))
}

/// TLS verification against the system's certificate authorities, loaded
/// once per process: the answer, including that there are none, holds until
/// a restart. Loading is the slow part of a `wss://` connect: on macOS it
/// asks the trust daemon about every keychain certificate, which takes
/// seconds; elsewhere it parses the system bundle. `SSL_CERT_FILE` and
/// `SSL_CERT_DIR` replace the system's set, read here.
fn authorities() -> Result<Arc<rustls::ClientConfig>, WireError> {
    static CONFIG: OnceLock<Result<Arc<rustls::ClientConfig>, WireError>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let loaded = rustls_native_certs::load_native_certs();
            if !loaded.errors.is_empty() {
                tracing::warn!(errors = ?loaded.errors, "some certificate authorities did not load");
            }
            let mut roots = rustls::RootCertStore::empty();
            let (added, ignored) = roots.add_parsable_certificates(loaded.certs);
            tracing::debug!(added, ignored, "certificate authorities loaded");
            if added == 0 {
                // The first load failure, if any, says why (a missing
                // `SSL_CERT_FILE`, say); the rest were logged above.
                let none = "the system has no certificate authorities";
                return Err(WireError::Untrusted(match loaded.errors.first() {
                    Some(cause) => format!("{none}: {cause}"),
                    None => none.to_owned(),
                }));
            }
            Ok(Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            ))
        })
        .clone()
}

/// Tell the receiver that this end cannot produce its request, and close
/// the connection. Best effort, within the socket's write timeout.
fn refuse<S: Read + Write>(link: &mut Link<S>, cause: RefusalCause, reason: &str) {
    let refusal = Refusal::new(cause, reason);
    let _ = link.send(Message::Text(wire::encode_refusal(refusal).into()));
    let _ = link.close(None);
}

/// The receiver's first message, its request, by `deadline` (which the
/// link's socket also bounds each read by). Messages of types this end does
/// not know are skipped. A peer that closes without a
/// [`Close`](crate::Close) code, or stays silent, sent none; one that
/// closes with one fails with it (a receiver stopped before it asked).
fn read_request<S: Read + Write>(
    link: &mut Link<S>,
    deadline: Instant,
) -> Result<RemoteSettings, LinkError> {
    loop {
        if Instant::now() >= deadline {
            return Err(WireError::NoRequest.into());
        }
        let text = match link.receive() {
            Ok(Some(Message::Text(text))) => text,
            Ok(Some(Message::Binary(_))) => return Err(WireError::Unexpected.into()),
            Ok(None) => continue,
            Err(LinkError::Wire(WireError::Closed(None))) => {
                return Err(WireError::NoRequest.into());
            }
            Err(error) => return Err(error),
        };
        match wire::decode_from_receiver(&text)? {
            FromReceiver::Request(request) => {
                return RemoteSettings::try_from(request)
                    .map_err(|error| WireError::Request(error).into());
            }
            FromReceiver::Unknown => {}
            FromReceiver::Keyframe => return Err(WireError::Unexpected.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::PixelFormat;
    use std::io;
    use tungstenite::WebSocket;
    use tungstenite::protocol::Role;

    /// A peer that takes everything and has nothing to say.
    struct Sink;

    impl Read for Sink {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::WouldBlock.into())
        }
    }

    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A receiver that answers forever, a little at a time: `upgraded`
    /// completes the upgrade first and then sends pings instead of the
    /// request; otherwise it drips the upgrade's answer byte by byte.
    #[allow(
        clippy::result_large_err,
        reason = "tungstenite's upgrade callback returns its error response; small enough on 32-bit targets"
    )]
    fn dripping(upgraded: bool) -> SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let drip = Duration::from_millis(20);
            if upgraded {
                let answer =
                    |_: &tungstenite::handshake::server::Request,
                     mut response: tungstenite::handshake::server::Response| {
                        response.headers_mut().insert(
                            header::SEC_WEBSOCKET_PROTOCOL,
                            HeaderValue::from_static(CAPTURE_PROTOCOL),
                        );
                        Ok(response)
                    };
                let mut socket = tungstenite::accept_hdr(socket, answer).unwrap();
                while socket
                    .send(tungstenite::Message::Ping(Bytes::new()))
                    .is_ok()
                {
                    std::thread::sleep(drip);
                }
            } else {
                let mut request = [0; 2048];
                let _ = socket.read(&mut request);
                while socket.write_all(b"X").is_ok() {
                    std::thread::sleep(drip);
                }
            }
        });
        address
    }

    /// A receiver that accepts and then holds its socket until the returned
    /// sender drops: with `request` it upgrades, sends the raw request and
    /// reads nothing more; without, it reads the upgrade request and does not
    /// answer. It says when it holds.
    fn holding(
        request: bool,
    ) -> (
        SocketAddr,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (holds, held) = std::sync::mpsc::channel();
        let (done, release) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _held: Box<dyn std::any::Any> = if request {
                let mut socket = crate::testkit::accept(socket);
                socket
                    .send(tungstenite::Message::text(
                        r#"{"type":"request","width":320,"height":180,"fps":30,"include_ui":true,"encoding":"raw"}"#,
                    ))
                    .unwrap();
                Box::new(socket)
            } else {
                let mut upgrade = Vec::new();
                let mut byte = [0; 1];
                while !upgrade.ends_with(b"\r\n\r\n") && socket.read(&mut byte).unwrap() == 1 {
                    upgrade.push(byte[0]);
                }
                Box::new(socket)
            };
            let _ = holds.send(());
            let _ = release.recv();
        });
        (address, held, done)
    }

    fn url(address: SocketAddr) -> Uri {
        format!("ws://{address}/api/v1/capture/stream")
            .parse()
            .unwrap()
    }

    /// Whether the call ended for the cut.
    fn stopped(error: &Error) -> bool {
        error.cause::<Stopping>().is_some()
    }

    /// Wait (yielding) until `hangup` counts a blocking wait after the first
    /// `seen`; a call that ended without waiting fails the test here.
    fn blocked(hangup: &Hangup, seen: usize) {
        let hangup = hangup.clone();
        prysm_testkit::within(Duration::from_secs(10), move || {
            while hangup.waits() == seen {
                std::thread::yield_now();
            }
        });
    }

    /// Long enough that a call the cut did not wake would still be waiting
    /// at the test's bound.
    const LONG: Duration = Duration::from_secs(60);

    /// A cut ends a connect waiting for the upgrade's answer at once, and
    /// every connect after it fails with [`Stopping`] without connecting.
    #[test]
    fn a_cut_ends_a_connect_waiting_for_the_upgrade() {
        let (address, held, _done) = holding(false);
        let hangup = Hangup::default();
        let connecting = {
            let hangup = hangup.clone();
            std::thread::spawn(move || {
                let offer = crate::testkit::offer();
                let deadline = Instant::now() + LONG;
                RemoteSender::connect_by(&url(address), address, &offer, &hangup, deadline)
                    .map(|_| ())
            })
        };
        held.recv().unwrap();
        hangup.cut();
        let error = prysm_testkit::within(Duration::from_secs(5), move || {
            connecting.join().unwrap().unwrap_err()
        });
        assert!(stopped(&error), "{}", chain(&error));
        let again =
            RemoteSender::connect_until(&url(address), address, &crate::testkit::offer(), &hangup)
                .unwrap_err();
        assert!(stopped(&again), "{}", chain(&again));
    }

    /// A cut ends a connect whose SYN is never answered (the listener's
    /// accept queue is full, so Linux and macOS drop the SYN) at once: its
    /// socket is connecting before any cut can reach it.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_cut_ends_a_connect_whose_syn_is_not_answered() {
        let listener =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        listener
            .bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())
            .unwrap();
        listener.listen(0).unwrap();
        let address = listener.local_addr().unwrap().as_socket().unwrap();
        // Fills the accept queue, never accepted.
        let _filler = TcpStream::connect(address).unwrap();
        let hangup = Hangup::default();
        let connecting = {
            let hangup = hangup.clone();
            std::thread::spawn(move || {
                let offer = crate::testkit::offer();
                let deadline = Instant::now() + LONG;
                RemoteSender::connect_by(&url(address), address, &offer, &hangup, deadline)
                    .map(|_| ())
            })
        };
        blocked(&hangup, 0);
        hangup.cut();
        let error = prysm_testkit::within(Duration::from_secs(5), move || {
            connecting.join().unwrap().unwrap_err()
        });
        assert!(stopped(&error), "{}", chain(&error));
        if cfg!(target_os = "linux") {
            assert_eq!(hangup.waits(), 1, "the connect never completed");
        }
    }

    /// A cut ends a send blocked on a full send buffer (the receiver reads
    /// nothing) at once, not at its write deadline.
    #[test]
    fn a_cut_ends_a_send_blocked_on_a_full_buffer() {
        let (address, held, _done) = holding(true);
        let hangup = Hangup::default();
        let mut sender =
            RemoteSender::connect_until(&url(address), address, &crate::testkit::offer(), &hangup)
                .unwrap();
        held.recv().unwrap();
        let connected = hangup.waits();
        let sending = std::thread::spawn(move || {
            let frame = Frame::fill(1, 320, 180, prysm_core::PixelFormat::RGB24).unwrap();
            loop {
                if let Err(error) = sender.deliver(&frame) {
                    return Error::from(error);
                }
            }
        });
        blocked(&hangup, connected);
        hangup.cut();
        let error = prysm_testkit::within(WRITE_TIMEOUT * 3 / 4, move || sending.join().unwrap());
        assert!(stopped(&error), "{}", chain(&error));
    }

    /// However slowly the receiver answers, the connect ends by its deadline.
    #[test]
    fn a_receiver_that_drips_cannot_hold_up_the_connect() {
        for upgraded in [false, true] {
            let address = dripping(upgraded);
            let url: Uri = format!("ws://{address}/api/v1/capture/stream")
                .parse()
                .unwrap();
            let deadline = Duration::from_millis(200);
            let started = Instant::now();
            let failed = prysm_testkit::within(Duration::from_secs(10), move || {
                let offer = crate::testkit::offer();
                let hangup = Hangup::default();
                RemoteSender::connect_by(&url, address, &offer, &hangup, started + deadline)
                    .is_err()
            });
            assert!(failed, "upgraded {upgraded}");
            assert!(
                started.elapsed() < deadline + Duration::from_secs(2),
                "upgraded {upgraded}: {:?}",
                started.elapsed()
            );
        }
    }

    /// Frame after frame, the message buffer is the same allocation: the
    /// WebSocket drops each message once it is written.
    #[test]
    fn one_allocation_serves_every_frame() {
        let mut link = Link::new(WebSocket::from_raw_socket(Sink, Role::Client, None));
        let mut outgoing = Outgoing::new(RemoteSettings {
            encoding: prysm_ports::StreamEncoding::Raw,
            ..RemoteSettings::default()
        });
        let mut first = None;
        for value in 0..5_u8 {
            let frame = Frame::fill(value, 320, 180, PixelFormat::RGB24).unwrap();
            let bytes = outgoing.frame(&frame).unwrap().unwrap();
            let at = bytes.as_ptr();
            assert_eq!(*first.get_or_insert(at), at, "frame {value} allocated anew");
            assert_eq!(bytes.len(), wire::HEADER_LEN + frame.len());
            link.send(Message::Binary(bytes)).unwrap();
        }
    }
}
