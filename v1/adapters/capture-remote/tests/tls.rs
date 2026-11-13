//! `wss://` through a TLS-terminating reverse proxy, in-process: a rustls
//! proxy with a test certificate forwards plain bytes to the portal
//! stand-in. The sender verifies certificates against the system's
//! authorities; the test points it at a test CA with `SSL_CERT_FILE` by
//! running itself again with that variable set (a test cannot change its
//! own environment safely).
use prysm_capture_remote::testkit::{BOUND, Portal};
use prysm_capture_remote::{RemoteSender, StreamEncoding, StreamRequest};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::{FrameSink, FrameSource};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const RAW: StreamRequest = StreamRequest {
    width: 320,
    height: 180,
    fps: 30,
    include_ui: true,
    encoding: StreamEncoding::Raw,
};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls")
        .join(name)
}

/// A TLS terminator for `localhost` in front of `upstream`, on an ephemeral
/// port: one thread per connection pumps both directions.
fn proxy(upstream: SocketAddr) -> SocketAddr {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certificates = CertificateDer::pem_file_iter(fixture("server.pem"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(fixture("server.key")).unwrap();
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

/// Copy decrypted bytes to `upstream` and back until either side ends.
fn pump(client: TcpStream, upstream: SocketAddr, config: Arc<rustls::ServerConfig>) {
    let tick = Some(Duration::from_millis(2));
    client.set_read_timeout(tick).unwrap();
    let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(config).unwrap(), client);
    let mut plain = TcpStream::connect(upstream).unwrap();
    plain.set_read_timeout(tick).unwrap();
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

#[test]
fn wss_through_a_tls_terminating_proxy() {
    let portal = Portal::start();
    let proxy = proxy(portal.addr);
    let url = format!("wss://localhost:{}/api/v1/capture/stream", proxy.port())
        .parse()
        .unwrap();

    if std::env::var_os("SSL_CERT_FILE").is_none() {
        // The test CA is not a system authority: verification fails.
        let source = portal.source(RAW);
        let error = RemoteSender::connect(&url, proxy).unwrap_err();
        let text = prysm_ports::chain(&error);
        assert!(text.to_lowercase().contains("certificate"), "{text}");
        Box::new(source).close().unwrap();

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "wss_through_a_tls_terminating_proxy",
                "--nocapture",
            ])
            .env("SSL_CERT_FILE", fixture("ca.pem"))
            .status()
            .unwrap();
        assert!(status.success(), "the run with the test CA failed");
        return;
    }

    let mut source = portal.source(RAW);
    let mut sender = RemoteSender::connect(&url, proxy).unwrap();
    assert_eq!(sender.request(), RAW);
    let data: Vec<u8> = (0..320 * 180 * 3).map(|i| i as u8).collect();
    let frame = Frame::new(data, 320, 180, PixelFormat::RGB24).unwrap();
    sender.send(&frame).unwrap();
    let deadline = Instant::now() + BOUND;
    let got = loop {
        assert!(Instant::now() < deadline, "no frame through the proxy");
        if let Some(got) = source
            .next_frame(Duration::from_millis(50))
            .unwrap()
            .into_frame()
        {
            break got;
        }
    };
    assert_eq!(got.as_slice(), frame.as_slice());

    // Lighting off: the pause and the close arrive through TLS as well.
    Box::new(source).close().unwrap();
    while sender.tick().is_ok() {
        assert!(
            Instant::now() < deadline,
            "the sender never noticed the close"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(sender.paused());
}
