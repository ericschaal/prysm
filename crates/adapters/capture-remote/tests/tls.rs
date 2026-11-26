//! `wss://` through a TLS-terminating reverse proxy, in-process: the
//! testkit's rustls proxy with a test certificate forwards plain bytes to the
//! portal stand-in. The sender verifies certificates against the system's
//! authorities; the test runs itself again twice with `SSL_CERT_FILE`
//! pointing at one authority: one that did not sign the proxy's certificate
//! (`in_other_ca_process`), then the test CA that did (`in_test_ca_process`).
use prysm_capture_remote::testkit::{
    BOUND, Portal, in_other_ca_process, in_test_ca_process, receive, tls_proxy,
};
use prysm_capture_remote::{Close, RemoteSender, RemoteSettings, WireError};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::StreamEncoding;
use prysm_ports::{FrameSink, FrameSource};
use prysm_testkit::eventually;

const RAW: RemoteSettings = RemoteSettings {
    width: 320,
    height: 180,
    fps: 30,
    include_ui: true,
    encoding: StreamEncoding::Raw,
};

#[test]
fn wss_through_a_tls_terminating_proxy() {
    const TEST: &str = "wss_through_a_tls_terminating_proxy";
    let refusing = in_other_ca_process(TEST);
    if !refusing && !in_test_ca_process(TEST) {
        return;
    }
    let portal = Portal::start();
    let proxy = tls_proxy(portal.addr);
    let url = format!("wss://localhost:{}/api/v1/capture/stream", proxy.port())
        .parse()
        .unwrap();

    if refusing {
        // The test CA is not an authority here: verification fails.
        let source = portal.source(RAW);
        let error = RemoteSender::connect(&url, proxy, &prysm_capture_remote::testkit::offer())
            .unwrap_err();
        // A certificate refusal naming the authority, not a lost connection.
        assert_eq!(
            error.cause::<WireError>(),
            Some(&WireError::Untrusted(
                "invalid peer certificate: UnknownIssuer".into()
            )),
            "{error}"
        );
        Box::new(source).close().unwrap();
        return;
    }

    // A peer that answers TLS in plain HTTP fails the handshake in rustls,
    // but not as an untrusted certificate.
    let plain = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = plain.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut peer, _) = plain.accept().unwrap();
        // Read the ClientHello first, so closing does not reset the answer.
        let _ = std::io::Read::read(&mut peer, &mut [0; 1024]);
        let _ = std::io::Write::write_all(&mut peer, b"HTTP/1.1 400 Bad Request\r\n\r\n");
    });
    let plain = format!("wss://localhost:{}/", address.port());
    let error = RemoteSender::connect(
        &plain.parse().unwrap(),
        address,
        &prysm_capture_remote::testkit::offer(),
    )
    .unwrap_err();
    assert_eq!(error.cause::<WireError>(), None, "{error}");
    // rustls's error sits inside the I/O error, out of the source chain.
    assert!(format!("{error:?}").contains("InvalidMessage"), "{error:?}");

    let mut source = portal.source(RAW);
    let mut sender =
        RemoteSender::connect(&url, proxy, &prysm_capture_remote::testkit::offer()).unwrap();
    assert_eq!(sender.request(), RAW);
    let data: Vec<u8> = (0..320 * 180 * 3).map(|i| i as u8).collect();
    let frame = Frame::new(data, 320, 180, PixelFormat::RGB24).unwrap();
    sender.send(&frame).unwrap();
    let got = receive(&mut source);
    assert_eq!(got.as_slice(), frame.as_slice());

    // Lighting off: the stopped close arrives through TLS as well.
    Box::new(source).close().unwrap();
    let error = eventually("the sender to notice the close", BOUND, || {
        sender.tick().err()
    });
    assert_eq!(
        error.cause::<WireError>(),
        Some(&WireError::Closed(Some(Close::Stopped))),
        "{error}"
    );
}
