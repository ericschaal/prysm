//! Feature `testkit`: what the web portal does for the remote source, on
//! loopback, for tests: accept, answer the WebSocket upgrade at the capture
//! stream path, and hand the connection over. Every wait is bounded.
use crate::{RemoteSender, RemoteSource, StreamRequest};
use prysm_ports::{Accepted, CAPTURE_PROTOCOL, CAPTURE_STREAM_PATH, Handoff, Result};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;
use tungstenite::WebSocket;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::{HeaderValue, Uri};
use tungstenite::stream::MaybeTlsStream;

/// The longest any wait here takes.
pub const BOUND: Duration = Duration::from_secs(5);

/// A stand-in for the portal on an ephemeral loopback port.
pub struct Portal {
    pub addr: SocketAddr,
    pub handoff: Handoff,
}

impl Portal {
    /// Serve upgrades on an ephemeral loopback port, for the test's life.
    ///
    /// # Panics
    /// Loopback cannot be bound.
    #[must_use]
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handoff = Handoff::new();
        let serving = handoff.clone();
        thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                upgrade(socket, &serving);
            }
        });
        Self { addr, handoff }
    }

    /// The capture stream URL.
    #[must_use]
    pub fn url(&self) -> Uri {
        format!("ws://{}{CAPTURE_STREAM_PATH}", self.addr)
            .parse()
            .unwrap()
    }

    /// Connect a sender.
    ///
    /// # Errors
    /// As [`RemoteSender::connect`].
    pub fn connect(&self) -> Result<RemoteSender> {
        RemoteSender::connect(&self.url(), self.addr)
    }

    /// Open a source receiving from this portal.
    ///
    /// # Panics
    /// The receive thread cannot start.
    #[must_use]
    pub fn source(&self, request: StreamRequest) -> RemoteSource {
        RemoteSource::open(&self.handoff, request).unwrap()
    }

    /// A WebSocket client that upgraded and sent nothing yet.
    ///
    /// # Panics
    /// The upgrade fails.
    #[must_use]
    pub fn client(&self) -> WebSocket<MaybeTlsStream<TcpStream>> {
        let socket = TcpStream::connect(self.addr).unwrap();
        socket.set_read_timeout(Some(BOUND)).unwrap();
        let mut request = self.url().into_client_request().unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static(CAPTURE_PROTOCOL),
        );
        tungstenite::client(request, MaybeTlsStream::Plain(socket))
            .unwrap()
            .0
    }
}

/// Read the request head, answer `101` with the subprotocol (`426` for
/// another version, `503` while no source receives) and hand the socket over
/// with what was read past the head.
fn upgrade(mut socket: TcpStream, handoff: &Handoff) {
    socket.set_read_timeout(Some(BOUND)).unwrap();
    let mut read = Vec::new();
    let end = loop {
        if let Some(at) = read.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let mut chunk = [0; 1024];
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => read.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&read[..end]).into_owned();
    let field = |wanted: &str| {
        head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(wanted)
                .then(|| value.trim().to_owned())
        })
    };
    if !field("sec-websocket-protocol")
        .is_some_and(|offered| offered.split(',').any(|p| p.trim() == CAPTURE_PROTOCOL))
    {
        let refused = format!(
            "HTTP/1.1 426 Upgrade Required\r\nsec-websocket-protocol: {CAPTURE_PROTOCOL}\r\ncontent-length: 0\r\n\r\n"
        );
        let _ = socket.write_all(refused.as_bytes());
        return;
    }
    let (Some(key), true) = (field("sec-websocket-key"), handoff.is_receiving()) else {
        let _ = socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n");
        return;
    };
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-accept: {accept}\r\nsec-websocket-protocol: {CAPTURE_PROTOCOL}\r\n\r\n"
    );
    if socket.write_all(response.as_bytes()).is_err() {
        return;
    }
    let _ = socket.set_read_timeout(None);
    let _ = handoff.offer(Accepted {
        socket,
        buffered: read[end..].to_vec(),
    });
}

/// Accept an upgrade on `stream` speaking this version: a peer other than the
/// portal, for tests of the sender.
///
/// # Panics
/// The upgrade fails.
#[must_use]
pub fn accept(stream: TcpStream) -> WebSocket<TcpStream> {
    #[allow(clippy::result_large_err, reason = "tungstenite's callback signature")]
    fn agree(
        _: &tungstenite::handshake::server::Request,
        mut response: tungstenite::handshake::server::Response,
    ) -> std::result::Result<
        tungstenite::handshake::server::Response,
        tungstenite::handshake::server::ErrorResponse,
    > {
        response.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static(CAPTURE_PROTOCOL),
        );
        Ok(response)
    }
    tungstenite::accept_hdr(stream, agree).unwrap()
}
