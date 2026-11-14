//! Feature `testkit`: what the web portal does for the remote source, on
//! loopback, for tests: accept, answer the WebSocket upgrade at the capture
//! stream path, and hand the connection over. Every wait is bounded. Also a
//! TLS-terminating proxy with a test certificate, for `wss://`.
use crate::{RemoteSender, RemoteSource, StreamRequest};
use prysm_ports::{Accepted, CAPTURE_PROTOCOL, CAPTURE_STREAM_PATH, Handoff, Result};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

fn tls_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls")
        .join(name)
}

/// The certificate authority that signed [`tls_proxy`]'s certificate for
/// `localhost`. A sender trusts it only with `SSL_CERT_FILE` pointing here,
/// set when the process starts: see [`in_test_ca_process`].
#[must_use]
pub fn test_ca() -> PathBuf {
    tls_fixture("ca.pem")
}

/// Set, to the authority's name, in the processes [`in_test_ca_process`]
/// and [`in_other_ca_process`] start.
const CA_PROCESS: &str = "PRYSM_TEST_CA_PROCESS";

/// Whether this is the process that trusts only [`test_ca`]. In the
/// test's first process, runs the test named `test` again in such a process
/// (this test binary with `SSL_CERT_FILE` set to the test CA and
/// `SSL_CERT_DIR` removed), asserts it passed and returns `false`; in a
/// process [`in_other_ca_process`] started, returns `false`.
///
/// A test cannot change its own environment safely, and the presence of
/// `SSL_CERT_FILE` says nothing: Cargo on Linux sets it (and `SSL_CERT_DIR`)
/// to the system's bundle for every process it starts, tests included.
///
/// # Panics
/// The run in the test CA's process failed.
#[must_use]
pub fn in_test_ca_process(test: &str) -> bool {
    in_process_trusting(test, "test", &test_ca())
}

/// As [`in_test_ca_process`], for a process that trusts only an authority
/// that signed nothing here, so [`tls_proxy`]'s certificate is refused.
/// Neither process reads the system's authorities, which on macOS takes
/// seconds.
///
/// # Panics
/// The run in that process failed.
#[must_use]
pub fn in_other_ca_process(test: &str) -> bool {
    in_process_trusting(test, "other", &tls_fixture("other-ca.pem"))
}

fn in_process_trusting(test: &str, name: &str, authority: &Path) -> bool {
    if let Some(running) = std::env::var_os(CA_PROCESS) {
        return running == name;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(CA_PROCESS, name)
        .env("SSL_CERT_FILE", authority)
        .env_remove("SSL_CERT_DIR")
        .status()
        .unwrap();
    assert!(status.success(), "{test} failed trusting the {name} CA");
    false
}

/// A TLS terminator for `localhost` in front of `upstream`, on an ephemeral
/// loopback port, for the test's life: one thread per connection pumps both
/// directions until either side ends.
///
/// # Panics
/// The test certificate cannot be loaded or loopback cannot be bound.
#[must_use]
pub fn tls_proxy(upstream: SocketAddr) -> SocketAddr {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certificates = CertificateDer::pem_file_iter(tls_fixture("server.pem"))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(tls_fixture("server.key")).unwrap();
    let config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certificates, key)
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let config = config.clone();
            thread::spawn(move || pump(client, upstream, config));
        }
    });
    addr
}

/// Complete the TLS handshake (within [`BOUND`]), then connect to
/// `upstream` and copy decrypted bytes both ways until either side ends.
/// Upstream is not involved before the handshake, so a slow client or one
/// that refuses the certificate never reaches the portal.
fn pump(mut client: TcpStream, upstream: SocketAddr, config: Arc<rustls::ServerConfig>) {
    let Ok(mut connection) = rustls::ServerConnection::new(config) else {
        return;
    };
    if client.set_read_timeout(Some(BOUND)).is_err() {
        return;
    }
    while connection.is_handshaking() {
        if connection.complete_io(&mut client).is_err() {
            return;
        }
    }
    let tick = Some(Duration::from_millis(2));
    if client.set_read_timeout(tick).is_err() {
        return;
    }
    let mut tls = rustls::StreamOwned::new(connection, client);
    let Ok(mut plain) = TcpStream::connect(upstream) else {
        return;
    };
    if plain.set_read_timeout(tick).is_err() {
        return;
    }
    let mut buffer = [0; 64 * 1024];
    let pending = |error: &std::io::Error| {
        matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
    };
    loop {
        match tls.read(&mut buffer) {
            Ok(0) => return,
            Ok(n) => {
                if plain.write_all(&buffer[..n]).is_err() {
                    return;
                }
            }
            Err(error) if pending(&error) => {}
            Err(_) => return,
        }
        match plain.read(&mut buffer) {
            Ok(0) => return,
            Ok(n) => {
                if tls.write_all(&buffer[..n]).is_err() {
                    return;
                }
            }
            Err(error) if pending(&error) => {}
            Err(_) => return,
        }
    }
}
