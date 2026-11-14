//! `wss://` through a TLS-terminating reverse proxy, in-process: the
//! testkit's rustls proxy with a test certificate forwards plain bytes to the
//! portal stand-in. The sender verifies certificates against the system's
//! authorities; the test runs itself again twice with `SSL_CERT_FILE`
//! pointing at one authority: one that did not sign the proxy's certificate
//! (`in_other_ca_process`), then the test CA that did (`in_test_ca_process`).
use prysm_capture_remote::testkit::{
    BOUND, Portal, in_other_ca_process, in_test_ca_process, tls_proxy,
};
use prysm_capture_remote::{RemoteSender, StreamEncoding, StreamRequest};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::{FrameSink, FrameSource};
use std::thread;
use std::time::{Duration, Instant};

const RAW: StreamRequest = StreamRequest {
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
        let error = RemoteSender::connect(&url, proxy).unwrap_err();
        let text = prysm_ports::chain(&error);
        // A certificate refusal naming the authority, not a lost connection.
        assert!(
            text.contains("invalid peer certificate: UnknownIssuer"),
            "{text}"
        );
        Box::new(source).close().unwrap();
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
