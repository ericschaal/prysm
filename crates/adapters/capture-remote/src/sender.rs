//! The capture-host end: one WebSocket to Prysm's portal, one frame per
//! `send`, encoded as the receiver requested.
use crate::link::{self, Link, LinkError, Message};
use crate::offer::Offer;
use crate::request::RemoteSettings;
use crate::wire::{
    self, Encoding, FromReceiver, HEADER_LEN, Header, MAX_TEXT_BYTES, Refusal, RefusalCause,
    ToReceiver, WireError,
};
use bytes::{Bytes, BytesMut};
use prysm_core::Frame;
use prysm_h264::{Encoder, EncoderSettings, H264Error};
use prysm_ports::{CAPTURE_OFFER_HEADER, CAPTURE_PROTOCOL, Error, FrameSink, Result, chain};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::{HeaderValue, StatusCode, Uri, header};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Connector, HandshakeError};

/// How long `connect` waits for the connection, the upgrade (and TLS), and
/// then for the receiver's request.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long one `send` may block on a receiver that stopped reading.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

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
    link: Link<MaybeTlsStream<TcpStream>>,
    /// A second handle on the link's socket, TLS or not, to switch it
    /// between non-blocking and blocking.
    socket: TcpStream,
    /// The capture stream's URL, for messages.
    url: Uri,
    request: RemoteSettings,
    stream: Stream,
    message: Outgoing,
}

/// How frames go out, as the request says.
enum Stream {
    Raw,
    /// The encoder opens at the first frame.
    H264 {
        settings: EncoderSettings,
        encoder: Option<Encoder>,
    },
}

/// What encoding one frame gave.
enum Encoded {
    /// Rate control skipped it; references are unaffected.
    Skipped,
    Appended {
        keyframe: bool,
    },
}

/// The frame message being built: header space, then the payload. One
/// allocation serves every frame: each finished message is split off, and
/// its space is reclaimed by the next `start` once the WebSocket has copied
/// and dropped it.
#[derive(Default)]
struct Outgoing(BytesMut);

impl Outgoing {
    /// Start a message: header space only; the payload is appended after it.
    fn start(&mut self) -> &mut BytesMut {
        self.0.clear();
        self.0.resize(HEADER_LEN, 0);
        &mut self.0
    }

    /// Fill in `header` for the payload appended and split the message off,
    /// leaving the buffer empty.
    fn finish(&mut self, header: &Header, encoding: Encoding) -> Result<Bytes, WireError> {
        let payload_len = self.0.len() - HEADER_LEN;
        let bytes = header.encode(encoding, payload_len)?;
        self.0[..HEADER_LEN].copy_from_slice(&bytes);
        Ok(self.0.split().freeze())
    }
}

impl std::fmt::Debug for RemoteSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteSender")
            .field("url", &self.url)
            .field("request", &self.request)
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
    /// stream with a code before it ([`WireError::Closed`]).
    pub fn connect(url: &Uri, address: SocketAddr, offer: &Offer) -> Result<Self> {
        let failed = |error| Error::adapter(format!("cannot connect to {url}"), error);
        let socket = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(failed)?;
        socket
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .and_then(|()| socket.set_read_timeout(Some(CONNECT_TIMEOUT)))
            .and_then(|()| socket.set_nodelay(true))
            .map_err(failed)?;
        let handle = socket.try_clone().map_err(failed)?;
        let mut link = upgrade(url, socket, offer)?;
        let request = read_request(&mut link, &handle)
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
        // From here the link's reads never wait: the receiver's messages are
        // read before each frame.
        handle
            .set_nonblocking(true)
            .map_err(|error| Error::adapter(format!("cannot configure {url}"), error))?;
        let stream = match request.encoder() {
            None => Stream::Raw,
            Some(settings) => Stream::H264 {
                settings,
                encoder: None,
            },
        };
        Ok(Self {
            link,
            socket: handle,
            url: url.clone(),
            request,
            stream,
            message: Outgoing::default(),
        })
    }

    /// What the receiver asked for; frames should match its size.
    #[must_use]
    pub fn request(&self) -> RemoteSettings {
        self.request
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
        let payload = self.message.start();
        let keyframe = match &mut self.stream {
            Stream::Raw => {
                payload.extend_from_slice(frame.as_slice());
                false
            }
            Stream::H264 { settings, encoder } => {
                match append_picture(payload, encoder, *settings, frame) {
                    Ok(Encoded::Appended { keyframe }) => keyframe,
                    Ok(Encoded::Skipped) => return Ok(()),
                    Err(error) => {
                        return Err(self.refused(Error::adapter("cannot encode frame", error)));
                    }
                }
            }
        };
        let header = Header {
            width: frame.width(),
            height: frame.height(),
            format: frame.format(),
            keyframe,
        };
        let message = match self.message.finish(&header, self.request.encoding.into()) {
            Ok(message) => message,
            Err(error) => return Err(self.refused(Error::adapter("cannot send frame", error))),
        };
        self.send_message(Message::Binary(message))
            .map_err(|error| self.failed_send(error))?;
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
    }

    /// Refuse the request for `error`, naming the encoding of an H.264
    /// stream (its encoder produced the frame) and the request otherwise.
    fn refused(&mut self, error: Error) -> SendError {
        let cause = match self.stream {
            Stream::H264 { .. } => RefusalCause::Encoding,
            Stream::Raw => RefusalCause::Request,
        };
        self.refuse_with(&chain(&error), cause);
        SendError::Refused(error)
    }

    /// Read the receiver's messages that arrived, without waiting. A
    /// keyframe request starts a new encoder session; a second request is
    /// out of turn.
    fn read_pending(&mut self) -> Result<()> {
        while let Some(message) = self.link.receive().map_err(|error| self.failed(error))? {
            let command = match message {
                Message::Text(text) => wire::decode_from_receiver(&text),
                Message::Binary(_) => Err(WireError::Unexpected),
            }
            .map_err(|error| self.failed(error.into()))?;
            match command {
                FromReceiver::Keyframe => {
                    if let Stream::H264 {
                        encoder: Some(encoder),
                        ..
                    } = &mut self.stream
                    {
                        encoder.restart();
                    }
                }
                FromReceiver::Unknown => {}
                FromReceiver::Request(_) => return Err(self.failed(WireError::Unexpected.into())),
            }
        }
        Ok(())
    }

    /// Send one message. A full send buffer is waited out, blocking for at
    /// most the write timeout.
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

    fn set_blocking(&self, blocking: bool) -> Result<(), LinkError> {
        self.socket
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

    fn failed(&self, error: LinkError) -> Error {
        error.context(format!("the stream to {} failed", self.url))
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

/// Encode `frame` with the H.264 encoder (opening it first if needed) and
/// append the picture to `payload`.
fn append_picture(
    payload: &mut BytesMut,
    encoder: &mut Option<Encoder>,
    settings: EncoderSettings,
    frame: &Frame,
) -> Result<Encoded, H264Error> {
    let encoder = match encoder {
        Some(encoder) => encoder,
        slot => slot.insert(Encoder::new(settings)?),
    };
    let Some(picture) = encoder.encode(frame)? else {
        return Ok(Encoded::Skipped);
    };
    payload.extend_from_slice(picture.data);
    Ok(Encoded::Appended {
        keyframe: picture.keyframe,
    })
}

/// Upgrade `socket` to a WebSocket at `url` with this end's subprotocol and
/// `offer`, with TLS first for `wss://`.
fn upgrade(url: &Uri, socket: TcpStream, offer: &Offer) -> Result<Link<MaybeTlsStream<TcpStream>>> {
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
        Some("wss") => {
            Some(Connector::Rustls(authorities().map_err(|none| {
                Error::adapter(format!("cannot open {url}"), none)
            })?))
        }
        _ => None,
    };
    // A request is the longest message this end reads.
    let config = Some(link::config(MAX_TEXT_BYTES));
    match tungstenite::client_tls_with_config(request, socket, config, connector) {
        Ok((socket, _)) => Ok(Link::new(socket)),
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
        Err(error) => Err(Error::adapter(format!("cannot open {url}"), error)),
    }
}

/// TLS verification against the system's certificate authorities, loaded
/// once per process: the answer, including that there are none, holds until
/// a restart. Loading is the slow part of a `wss://` connect: on macOS it
/// asks the trust daemon about every keychain certificate, which takes
/// seconds; elsewhere it parses the system bundle. `SSL_CERT_FILE` and
/// `SSL_CERT_DIR` replace the system's set, read here.
fn authorities() -> Result<Arc<rustls::ClientConfig>, Arc<NoAuthorities>> {
    static CONFIG: OnceLock<Result<Arc<rustls::ClientConfig>, Arc<NoAuthorities>>> =
        OnceLock::new();
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
                let cause = loaded.errors.into_iter().next();
                return Err(Arc::new(NoAuthorities(cause)));
            }
            Ok(Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            ))
        })
        .clone()
}

/// No `wss://` connection can be verified: not one certificate authority
/// loaded.
#[derive(Debug, thiserror::Error)]
#[error("the system has no certificate authorities")]
struct NoAuthorities(#[source] Option<rustls_native_certs::Error>);

/// Tell the receiver that this end cannot produce its request, and close
/// the connection. Best effort, within the socket's write timeout.
fn refuse<S: Read + Write>(link: &mut Link<S>, cause: RefusalCause, reason: &str) {
    let refusal = ToReceiver::Refused(Refusal::new(cause, reason));
    let _ = link.send(Message::Text(wire::encode_to_receiver(&refusal).into()));
    let _ = link.close(None);
}

/// The receiver's first message, its request, within [`CONNECT_TIMEOUT`]
/// (`socket` is the link's, its read timeout set to the time left).
/// Messages of types this end does not know are skipped. A peer that closes
/// without a [`Close`](crate::Close) code, or stays silent, sent none; one
/// that closes with one fails with it (a receiver stopped before it asked).
fn read_request<S: Read + Write>(
    link: &mut Link<S>,
    socket: &TcpStream,
) -> Result<RemoteSettings, LinkError> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(WireError::NoRequest.into());
        }
        socket
            .set_read_timeout(Some(left))
            .map_err(|error| LinkError::Socket(error.into()))?;
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

    /// Frame after frame, the message buffer is the same allocation: the
    /// WebSocket drops each message once it is written.
    #[test]
    fn one_allocation_serves_every_frame() {
        let mut link = Link::new(WebSocket::from_raw_socket(Sink, Role::Client, None));
        let mut message = Outgoing::default();
        let header = Header {
            width: 320,
            height: 180,
            format: PixelFormat::RGB24,
            keyframe: false,
        };
        let mut first = None;
        for frame in 0..5_u8 {
            let payload = message.start();
            payload.extend_from_slice(&vec![frame; 320 * 180 * 3]);
            let at = payload.as_ptr();
            assert_eq!(*first.get_or_insert(at), at, "frame {frame} allocated anew");
            let bytes = message.finish(&header, Encoding::Raw).unwrap();
            assert_eq!(bytes.len(), HEADER_LEN + 320 * 180 * 3);
            link.send(Message::Binary(bytes)).unwrap();
        }
    }
}
