//! The capture-host end: one WebSocket to Prysm's portal, one frame per
//! `send`, encoded as the receiver requested.
use crate::link::{self, Inbound, Link, LinkError, Outbound};
use crate::request::StreamRequest;
use crate::wire::{
    self, Command, Encoding, HEADER_LEN, Header, MAX_TEXT_BYTES, Refusal, Refused, WireError,
};
use bytes::{Bytes, BytesMut};
use prysm_core::Frame;
use prysm_h264::{Encoder, EncoderSettings, H264Error};
use prysm_ports::{CAPTURE_PROTOCOL, Error, FrameSink, Result, chain};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
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
    /// The requested encoder could not start or failed. The receiver was
    /// told (see [`RemoteSender::refuse_encoding`]).
    #[error(transparent)]
    Encoder(Error),
    /// The connection to the receiver failed.
    #[error(transparent)]
    Connection(#[from] Error),
}

impl From<SendError> for Error {
    fn from(error: SendError) -> Self {
        match error {
            SendError::Encoder(error) | SendError::Connection(error) => error,
        }
    }
}

/// Sends frames to a [`RemoteSource`](crate::RemoteSource) through Prysm's
/// portal, as its [`StreamRequest`] says. Any failure is final: the caller
/// drops the sender and connects a new one. [`RemoteSender::deliver`] tells
/// an encoder failure from a connection failure; [`FrameSink::send`] does not.
///
/// The receiver can pause the stream (frames are dropped unencoded until it
/// resumes) and ask for a keyframe; each `send` and `tick` first applies
/// those requests, without waiting for any. A pause, resume or keyframe
/// request starts a new H.264 encoder session, so the next picture is a
/// keyframe, and a pause releases a hardware encoder.
pub struct RemoteSender {
    /// Reads never wait (the socket is non-blocking once connected);
    /// writes wait out a full send buffer within the write timeout.
    link: Link<MaybeTlsStream<TcpStream>>,
    /// A second handle on the link's socket, TLS or not, to switch it
    /// between non-blocking and blocking.
    socket: TcpStream,
    /// The capture stream's URL, for messages.
    url: Uri,
    request: StreamRequest,
    stream: Stream,
    paused: bool,
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
enum Picture {
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
            .field("paused", &self.paused)
            .finish_non_exhaustive()
    }
}

impl RemoteSender {
    /// Open the capture stream at `url` (`ws://`, or `wss://` verified
    /// against the system's certificate authorities) by connecting to
    /// `address`, where the URL's host resolved to (the caller resolves, so
    /// it bounds the lookup), with this end's protocol version as the
    /// subprotocol; then read the receiver's request. The requested encoder
    /// opens at the first frame.
    ///
    /// # Errors
    /// Prysm is unreachable, failed TLS verification, has no receiving
    /// source ([`WireError::Unavailable`] `503`), speaks another version
    /// ([`WireError::Version`]) or sent no valid request.
    pub fn connect(url: &Uri, address: SocketAddr) -> Result<Self> {
        let failed = |error| Error::adapter(format!("cannot connect to {url}"), error);
        let socket = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(failed)?;
        socket
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .and_then(|()| socket.set_read_timeout(Some(CONNECT_TIMEOUT)))
            .and_then(|()| socket.set_nodelay(true))
            .map_err(failed)?;
        let handle = socket.try_clone().map_err(failed)?;
        let mut link = upgrade(url, socket)?;
        let request = read_request(&mut link)
            .map_err(|error| error.context(format!("{url} sent no request")))?;
        // From here the link's reads never wait: controls are polled per frame.
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
            paused: false,
            message: Outgoing::default(),
        })
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

    /// Apply the receiver's pending control messages, then send one frame,
    /// encoded as requested; dropped while paused.
    ///
    /// # Errors
    /// [`SendError::Encoder`] when the requested encoder cannot start or
    /// failed (the receiver is told), else [`SendError::Connection`].
    pub fn deliver(&mut self, frame: &Frame) -> Result<(), SendError> {
        self.apply_pending_controls()?;
        if self.paused {
            return Ok(());
        }
        let payload = self.message.start();
        let keyframe = match &mut self.stream {
            Stream::Raw => {
                payload.extend_from_slice(frame.as_slice());
                false
            }
            Stream::H264 { settings, encoder } => {
                match append_picture(payload, encoder, *settings, frame) {
                    Ok(Picture::Appended { keyframe }) => keyframe,
                    Ok(Picture::Skipped) => return Ok(()),
                    Err(error) => {
                        let error = Error::adapter("cannot encode frame", error);
                        self.refuse_encoding(&chain(&error));
                        return Err(SendError::Encoder(error));
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
        let message = self
            .message
            .finish(&header, self.request.encoding.into())
            .map_err(|error| Error::adapter("cannot send frame", error))?;
        self.send_message(Outbound::Binary(message))
            .map_err(|error| self.failed(error))?;
        Ok(())
    }

    /// Tell the receiver that this end cannot produce the requested stream
    /// (`reason`, shown to its user), and close the connection. Best effort:
    /// a receiver that is gone learns nothing.
    pub fn refuse(&mut self, reason: &str) {
        self.refuse_with(reason, Refused::Request);
    }

    /// Like [`Self::refuse`], naming the requested encoding as the cause:
    /// its encoder cannot start or failed. `send` does this on an encoder
    /// failure.
    pub fn refuse_encoding(&mut self, reason: &str) {
        self.refuse_with(reason, Refused::Encoding);
    }

    fn refuse_with(&mut self, reason: &str, refused: Refused) {
        let refusal = wire::encode_refusal(&Refusal::new(refused, reason));
        let _ = self.send_message(Outbound::Text(refusal));
        self.link.close();
    }

    /// Apply the receiver's control messages that arrived, without waiting.
    fn apply_pending_controls(&mut self) -> Result<()> {
        while let Some(message) = self.link.receive().map_err(|error| self.failed(error))? {
            let command = match message {
                Inbound::Text(text) => wire::decode_command(&text),
                Inbound::Binary(_) => Err(WireError::Unexpected),
            }
            .map_err(|error| self.failed(error.into()))?;
            self.apply_control(command)
                .map_err(|error| self.failed(error.into()))?;
        }
        Ok(())
    }

    /// Pause, resume or ask for a keyframe; each starts a new encoder session
    /// (a pause also releases a hardware encoder). A second request is out of
    /// turn.
    fn apply_control(&mut self, command: Command) -> Result<(), WireError> {
        self.paused = match command {
            Command::Pause => true,
            Command::Resume => false,
            Command::Keyframe => self.paused,
            Command::Request(_) => return Err(WireError::Unexpected),
        };
        if let Stream::H264 {
            encoder: Some(encoder),
            ..
        } = &mut self.stream
        {
            encoder.restart();
        }
        Ok(())
    }

    /// Send one message. A full send buffer is waited out, blocking for at
    /// most the write timeout.
    fn send_message(&mut self, message: Outbound) -> Result<(), LinkError> {
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

    fn failed(&self, error: LinkError) -> Error {
        error.context(format!("the stream to {} failed", self.url))
    }
}

impl FrameSink for RemoteSender {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        Ok(self.deliver(frame)?)
    }

    /// No frame to send: apply the receiver's pending control messages, so
    /// a sender whose capture is stopped while paused learns of the resume.
    fn tick(&mut self) -> Result<()> {
        self.apply_pending_controls()
    }
}

/// Encode `frame` with the H.264 encoder (opening it first if needed) and
/// append the picture to `payload`.
fn append_picture(
    payload: &mut BytesMut,
    encoder: &mut Option<Encoder>,
    settings: EncoderSettings,
    frame: &Frame,
) -> Result<Picture, H264Error> {
    let encoder = match encoder {
        Some(encoder) => encoder,
        slot => slot.insert(Encoder::new(settings)?),
    };
    let Some(picture) = encoder.encode(frame)? else {
        return Ok(Picture::Skipped);
    };
    payload.extend_from_slice(picture.data);
    Ok(Picture::Appended {
        keyframe: picture.keyframe,
    })
}

/// Upgrade `socket` to a WebSocket at `url` with this end's subprotocol,
/// with TLS first for `wss://`.
fn upgrade(url: &Uri, socket: TcpStream) -> Result<Link<MaybeTlsStream<TcpStream>>> {
    let mut request = url
        .into_client_request()
        .map_err(|error| Error::adapter(format!("invalid capture stream URL {url}"), error))?;
    request.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(CAPTURE_PROTOCOL),
    );
    let connector = match url.scheme_str() {
        Some("wss") => Some(Connector::Rustls(authorities().ok_or_else(|| {
            Error::adapter(
                format!("cannot open {url}"),
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "the system has no certificate authorities",
                ),
            )
        })?)),
        _ => None,
    };
    // A request or a control is the longest message this end reads.
    let config = Some(link::config(MAX_TEXT_BYTES));
    match tungstenite::client_tls_with_config(request, socket, config, connector) {
        Ok((socket, _)) => Ok(Link::new(socket)),
        Err(HandshakeError::Failure(tungstenite::Error::Http(response))) => {
            let status = response.status();
            // The portal names the version it speaks with its refusal.
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
/// once per process (`None`: there are none). Loading is the slow part of a
/// `wss://` connect: on macOS it asks the trust daemon about every keychain
/// certificate, which takes seconds; elsewhere it parses the system bundle.
/// `SSL_CERT_FILE` and `SSL_CERT_DIR` replace the system's set, read here.
fn authorities() -> Option<Arc<rustls::ClientConfig>> {
    static CONFIG: OnceLock<Option<Arc<rustls::ClientConfig>>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let loaded = rustls_native_certs::load_native_certs();
            if !loaded.errors.is_empty() {
                tracing::warn!(errors = ?loaded.errors, "some certificate authorities did not load");
            }
            let mut roots = rustls::RootCertStore::empty();
            let (added, ignored) = roots.add_parsable_certificates(loaded.certs);
            tracing::debug!(added, ignored, "certificate authorities loaded");
            (added > 0).then(|| {
                Arc::new(
                    rustls::ClientConfig::builder()
                        .with_root_certificates(roots)
                        .with_no_client_auth(),
                )
            })
        })
        .clone()
}

/// The receiver's first message: its request. A peer that closes or stays
/// silent sent none.
fn read_request<S: Read + Write>(link: &mut Link<S>) -> Result<StreamRequest, LinkError> {
    let text = match link.receive() {
        Ok(Some(Inbound::Text(text))) => text,
        Ok(Some(Inbound::Binary(_))) => return Err(WireError::Unexpected.into()),
        Ok(None) | Err(LinkError::Wire(WireError::Closed)) => {
            return Err(WireError::NoRequest.into());
        }
        Err(error) => return Err(error),
    };
    match wire::decode_command(&text)? {
        Command::Request(request) => Ok(request),
        Command::Pause | Command::Resume | Command::Keyframe => Err(WireError::NoRequest.into()),
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
            link.send(Outbound::Binary(bytes)).unwrap();
        }
    }
}
