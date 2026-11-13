//! Both ends on loopback over WebSocket, through a stand-in for the portal's
//! upgrade and hand-over. Every wait is bounded.
use prysm_capture_remote::testkit::{BOUND, Portal};
use prysm_capture_remote::{
    RemoteReceiver, RemoteSender, RemoteSource, StreamEncoding, StreamRequest, WireError,
};
use prysm_core::{Frame, PixelFormat, YuvRange, YuvStandardMatrix};
use prysm_ports::{FrameSink, FrameSource, SourceFactory};
use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{CallLog, FakeSource, FakeSourceSettings};
use std::error::Error as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Uncompressed frames of any size and format.
const RAW: StreamRequest = StreamRequest {
    width: 320,
    height: 180,
    fps: 30,
    include_ui: true,
    encoding: StreamEncoding::Raw,
};

/// A portal with a raw source receiving.
fn open() -> (Portal, RemoteSource) {
    let portal = Portal::start();
    let source = portal.source(RAW);
    (portal, source)
}

/// An RGB frame whose first byte is `seed` and whose pixels are all distinct-ish.
fn frame(seed: u8, width: u32, height: u32) -> Frame {
    let len = (width * height * 3) as usize;
    let data = (0..len).map(|i| seed.wrapping_add(i as u8)).collect();
    Frame::new(data, width, height, PixelFormat::RGB24).unwrap()
}

fn receive(source: &mut RemoteSource) -> Frame {
    let deadline = Instant::now() + BOUND;
    while Instant::now() < deadline {
        if let Some(frame) = source
            .next_frame(Duration::from_millis(50))
            .unwrap()
            .into_frame()
        {
            return frame;
        }
    }
    panic!("no frame within {BOUND:?}");
}

type Client = WebSocket<MaybeTlsStream<TcpStream>>;

/// Wait for the receiver to drop a raw client: a close or reset, never a
/// timeout. Returns the messages it sent first.
fn assert_dropped(mut client: Client) -> Vec<Message> {
    let mut answers = Vec::new();
    loop {
        match client.read() {
            Ok(Message::Close(_)) => {}
            Ok(message) => answers.push(message),
            Err(tungstenite::Error::Io(error)) => {
                assert!(
                    !matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ),
                    "connection was not dropped: {error}"
                );
                break;
            }
            Err(_) => break,
        }
    }
    answers
}

/// The request a raw client reads first: raw, with menus, 320x180 at 30 fps.
const REQUEST: &str =
    r#"{"type":"request","width":320,"height":180,"fps":30,"include_ui":true,"encoding":"raw"}"#;

/// A raw client that has read the request.
fn greeted(portal: &Portal) -> Client {
    let mut client = portal.client();
    let Message::Text(request) = client.read().unwrap() else {
        panic!("no request");
    };
    assert_eq!(request.as_str(), REQUEST);
    client
}

/// A frame message by hand: format, range, matrix, flags, width, height
/// (`u16` each), then `payload`.
fn raw_frame(format: [u8; 4], width: u16, height: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = format.to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

/// The typed wire cause of an adapter error.
fn wire_cause(error: &prysm_ports::Error) -> WireError {
    let prysm_ports::Error::Adapter {
        source: Some(cause),
        ..
    } = error
    else {
        panic!("no cause: {error}");
    };
    let mut cause: &(dyn std::error::Error + 'static) = &**cause;
    loop {
        if let Some(wire) = cause.downcast_ref::<WireError>() {
            return wire.clone();
        }
        cause = cause
            .source()
            .unwrap_or_else(|| panic!("no wire cause: {error}"));
    }
}

#[test]
fn round_trip_preserves_dimensions_format_and_pixels() {
    let (portal, mut source) = open();
    let mut sender = portal.connect().unwrap();

    let rgb = frame(7, 7, 5);
    sender.send(&rgb).unwrap();
    let got = receive(&mut source);
    assert_eq!((got.width(), got.height()), (7, 5));
    assert_eq!(got.format(), PixelFormat::RGB24);
    assert_eq!(got.as_slice(), rgb.as_slice());

    let format = PixelFormat::YUYV {
        range: YuvRange::Limited,
        matrix: YuvStandardMatrix::Bt709,
    };
    let yuyv = Frame::new((0..16).collect(), 4, 2, format).unwrap();
    sender.send(&yuyv).unwrap();
    let got = receive(&mut source);
    assert_eq!((got.width(), got.height(), got.format()), (4, 2, format));
    assert_eq!(got.as_slice(), yuyv.as_slice());

    // NV12: 1.5 bytes per pixel, both planes intact; a full-size frame.
    let format = PixelFormat::NV12 {
        range: YuvRange::Limited,
        matrix: YuvStandardMatrix::Bt709,
    };
    let nv12 = Frame::new(
        (0..1920 * 1080 * 3 / 2).map(|i| i as u8).collect(),
        1920,
        1080,
        format,
    )
    .unwrap();
    sender.send(&nv12).unwrap();
    let got = receive(&mut source);
    assert_eq!(
        (got.width(), got.height(), got.format()),
        (1920, 1080, format)
    );
    assert_eq!(got.as_slice(), nv12.as_slice());

    Box::new(source).close().unwrap();
}

#[test]
fn next_frame_times_out_idle() {
    let (_portal, mut source) = open();
    let started = Instant::now();
    assert!(
        source
            .next_frame(Duration::from_millis(30))
            .unwrap()
            .is_idle()
    );
    assert!(started.elapsed() >= Duration::from_millis(30));
    Box::new(source).close().unwrap();
}

#[test]
fn a_slow_consumer_sees_only_the_newest_frame() {
    let (portal, mut source) = open();
    let mut sender = portal.connect().unwrap();
    for seed in 1..=20 {
        sender.send(&frame(seed, 8, 4)).unwrap();
    }
    thread::sleep(Duration::from_millis(200));

    let mut seen = Vec::new();
    while seen.last() != Some(&20) {
        seen.push(receive(&mut source).as_slice()[0]);
    }
    assert!(seen.len() < 20, "frames were queued: {seen:?}");
    assert!(seen.is_sorted(), "frames out of order: {seen:?}");
    assert!(
        source
            .next_frame(Duration::from_millis(100))
            .unwrap()
            .is_idle()
    );
    Box::new(source).close().unwrap();
}

#[test]
fn receiver_survives_disconnect_and_accepts_a_new_sender() {
    let (portal, mut source) = open();
    let mut first = portal.connect().unwrap();
    first.send(&frame(1, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 1);
    drop(first);

    // A disconnect is not an error.
    assert!(
        source
            .next_frame(Duration::from_millis(200))
            .unwrap()
            .is_idle()
    );

    let mut second = portal.connect().unwrap();
    second.send(&frame(2, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 2);
    Box::new(source).close().unwrap();
}

/// One sender at a time: a second one replaces the first, whose connection
/// ends (a TV that crashed without closing, or a second TV).
#[test]
fn a_second_sender_replaces_the_first() {
    let (portal, mut source) = open();
    // Upgraded, then silence: a crashed TV whose socket never closed.
    let stalled = portal.client();
    thread::sleep(Duration::from_millis(100));

    let mut sender = portal.connect().unwrap();
    sender.send(&frame(3, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 3);
    assert_dropped(stalled);

    // A second working sender takes over; the first one's stream ends.
    let mut second = portal.connect().unwrap();
    second.send(&frame(4, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 4);
    let deadline = Instant::now() + BOUND;
    while sender.tick().is_ok() {
        assert!(Instant::now() < deadline, "the replaced sender kept going");
        thread::sleep(Duration::from_millis(10));
    }
    Box::new(source).close().unwrap();
}

#[test]
fn malformed_and_oversized_messages_drop_only_that_connection() {
    let (portal, mut source) = open();

    // Text that is not a refusal.
    let mut garbage = greeted(&portal);
    garbage.send(Message::text("GET / HTTP/1.1")).unwrap();
    assert!(assert_dropped(garbage).is_empty());

    // 4096x4096 RGB: over the payload limit from the header alone.
    let mut oversized = greeted(&portal);
    oversized
        .send(Message::binary(raw_frame([1, 0, 0, 0], 4096, 4096, &[])))
        .unwrap();
    assert!(assert_dropped(oversized).is_empty());
    // A payload one byte shorter than the header's frame.
    let mut short = greeted(&portal);
    short
        .send(Message::binary(raw_frame([1, 0, 0, 0], 4, 4, &[0; 47])))
        .unwrap();
    assert!(assert_dropped(short).is_empty());
    // A refusal with a field the protocol does not have.
    let mut unknown = greeted(&portal);
    unknown
        .send(Message::text(
            r#"{"refused":"request","reason":"x","why":1}"#,
        ))
        .unwrap();
    assert!(assert_dropped(unknown).is_empty());
    assert!(
        source
            .next_frame(Duration::from_millis(50))
            .unwrap()
            .is_idle()
    );

    let mut sender = portal.connect().unwrap();
    sender.send(&frame(4, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 4);
    Box::new(source).close().unwrap();
}

/// The version is the WebSocket subprotocol: a sender of another version is
/// refused at the upgrade, and either end says which versions differ.
#[test]
fn version_mismatches_are_typed_errors_on_both_ends() {
    let (portal, source) = open();
    // An older sender: the portal answers 426 with the version it speaks.
    for offered in [Some("prysm-capture.v3"), None] {
        let socket = TcpStream::connect(portal.addr).unwrap();
        let mut request = portal.url().into_client_request().unwrap();
        if let Some(offered) = offered {
            request
                .headers_mut()
                .insert("sec-websocket-protocol", offered.parse().unwrap());
        }
        let Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))) =
            tungstenite::client(request, socket)
        else {
            panic!("{offered:?} was upgraded");
        };
        assert_eq!(response.status(), 426);
        assert_eq!(
            response.headers()["sec-websocket-protocol"],
            prysm_ports::CAPTURE_PROTOCOL
        );
    }
    Box::new(source).close().unwrap();

    // A newer Prysm: this sender is told both versions.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let newer = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = [0; 1024];
        let _ = stream.read(&mut head).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 426 Upgrade Required\r\nsec-websocket-protocol: prysm-capture.v9\r\ncontent-length: 0\r\n\r\n",
            )
            .unwrap();
    });
    let url = format!("ws://{addr}/api/v1/capture/stream")
        .parse()
        .unwrap();
    let error = RemoteSender::connect(&url, addr).unwrap_err();
    assert_eq!(
        wire_cause(&error),
        WireError::Version {
            ours: prysm_ports::CAPTURE_VERSION,
            theirs: Some(9)
        },
        "{error}"
    );
    newer.join().unwrap();

    // Receivers that upgrade but send no request, or one this sender cannot
    // understand.
    for (answer, expected) in [
        (None, WireError::NoRequest),
        (
            Some(r#"{"type":"request","include_ui":9}"#),
            WireError::Json("invalid type: integer `9`, expected a boolean".into()),
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = prysm_capture_remote::testkit::accept(stream);
            match answer {
                Some(answer) => socket.send(Message::text(answer)).unwrap(),
                None => socket.close(None).unwrap(),
            }
            let _ = socket.flush();
            // Hold the connection until the sender has read the answer.
            let _ = socket.read();
        });
        let url = format!("ws://{addr}/api/v1/capture/stream")
            .parse()
            .unwrap();
        let error = RemoteSender::connect(&url, addr).unwrap_err();
        let cause = wire_cause(&error);
        match (&cause, &expected) {
            (WireError::Json(got), WireError::Json(want)) => {
                assert!(got.starts_with(want.as_str()), "{got}");
            }
            _ => assert_eq!(cause, expected, "{error}"),
        }
        assert!(error.to_string().contains(&addr.to_string()), "{error}");
        peer.join().unwrap();
    }
}

/// A sender that connects while lighting is off (no source receives) is
/// told so with a typed cause and retries; a source opened later gets it.
#[test]
fn a_sender_before_the_source_waits_for_it() {
    let portal = Portal::start();
    let error = portal.connect().unwrap_err();
    assert_eq!(wire_cause(&error), WireError::Unavailable(503), "{error}");

    let mut source = portal.source(RAW);
    let mut sender = portal.connect().unwrap();
    sender.send(&frame(5, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 5);
    Box::new(source).close().unwrap();
}

/// The sender learns the receiver's request at connect; a new request takes
/// a new source, and the sender's next connection gets it.
#[test]
fn the_sender_receives_the_request_and_a_new_source_changes_it() {
    let (portal, source) = open();
    assert_eq!(source.request(), RAW);
    let sender = portal.connect().unwrap();
    assert_eq!(sender.request(), RAW);
    Box::new(source).close().unwrap();
    drop(sender);

    let changed = StreamRequest {
        width: 640,
        height: 360,
        fps: 60,
        include_ui: false,
        encoding: StreamEncoding::H264Software,
    };
    let source = portal.source(changed);
    assert_eq!(portal.connect().unwrap().request(), changed);
    Box::new(source).close().unwrap();
}

/// A sender that cannot produce the request says why; the source fails with
/// that reason and stops.
#[test]
fn a_refusal_fails_the_source_with_the_senders_reason() {
    let refused = |refuse: fn(&mut RemoteSender)| {
        let (portal, mut source) = open();
        let mut sender = portal.connect().unwrap();
        refuse(&mut sender);
        let deadline = Instant::now() + BOUND;
        let error = loop {
            assert!(Instant::now() < deadline, "the refusal never arrived");
            match source.next_frame(Duration::from_millis(50)) {
                Err(error) => break error,
                Ok(pull) => assert!(pull.is_idle()),
            }
        };
        assert!(
            source.next_frame(Duration::ZERO).is_err(),
            "the failure stays"
        );
        // The receive thread stopped with the failure: closing is prompt.
        let started = Instant::now();
        Box::new(source).close().unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(!portal.handoff.is_receiving());
        error
    };
    let error = refused(|sender| sender.refuse("the TV's capture is busy"));
    assert_eq!(
        wire_cause(&error),
        WireError::Refused("the TV's capture is busy".into())
    );
    assert_eq!(
        prysm_ports::Error::to_string(&error),
        "the sender cannot stream 320x180 at 30 fps with menus as raw"
    );

    // A refused encoding names the setting, with the sender's reason.
    let error = refused(|sender| sender.refuse_encoding("no encoder here"));
    let prysm_ports::Error::Invalid(invalid) = &error else {
        panic!("{error:?}");
    };
    assert_eq!(invalid.field, "capture.encoding");
    assert_eq!(
        invalid.reason,
        "the sender cannot stream 320x180 at 30 fps with menus as raw: no encoder here"
    );
}

#[test]
fn close_is_prompt_stops_receiving_and_fails_the_sender() {
    let (portal, mut source) = open();
    let mut sender = portal.connect().unwrap();
    sender.send(&frame(5, 4, 4)).unwrap();
    receive(&mut source);

    let started = Instant::now();
    Box::new(source).close().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(!portal.handoff.is_receiving());

    let deadline = Instant::now() + BOUND;
    let error = loop {
        assert!(Instant::now() < deadline, "sender never noticed the close");
        if let Err(error) = sender.send(&frame(6, 4, 4)) {
            break error;
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(error.source().is_some(), "{error}");
}

/// A sender with nothing to send (the slim TV stops capturing while paused)
/// still reads control messages through `tick`: a receiver that closes
/// (lighting stops) pauses it before the connection ends.
#[test]
fn idle_reads_the_pause_of_a_closing_receiver() {
    let (portal, mut source) = open();
    let mut sender = portal.connect().unwrap();
    sender.send(&frame(1, 4, 4)).unwrap();
    receive(&mut source);
    sender.tick().unwrap();
    assert!(!sender.paused());

    Box::new(source).close().unwrap();
    let deadline = Instant::now() + BOUND;
    while sender.tick().is_ok() {
        assert!(Instant::now() < deadline, "sender never noticed the close");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(sender.paused(), "paused before the connection ended");
}

#[test]
fn connecting_to_a_dead_peer_fails_with_a_source() {
    let addr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let url = format!("ws://{addr}/api/v1/capture/stream")
        .parse()
        .unwrap();
    let error = RemoteSender::connect(&url, addr).unwrap_err();
    assert!(error.to_string().contains(&addr.to_string()), "{error}");
    assert!(error.source().is_some(), "{error}");
}

#[test]
fn factory_decodes_validates_and_opens() {
    let factory = RemoteReceiver::default();
    let defaults: StreamRequest = serde_json::from_str("{}").unwrap();
    assert_eq!(defaults, StreamRequest::default());
    assert!(factory.validate(&defaults).is_ok());
    let full: StreamRequest = serde_json::from_str(
        r#"{ "width": 640, "height": 280, "fps": 25, "include_ui": false, "encoding": "raw" }"#,
    )
    .unwrap();
    assert_eq!(
        full,
        StreamRequest {
            width: 640,
            height: 280,
            fps: 25,
            include_ui: false,
            encoding: StreamEncoding::Raw,
        }
    );
    for bad in [
        r#"{ "listen": "0.0.0.0:7380" }"#,
        r#"{ "x": 1 }"#,
        r#"{ "encoding": "lz4" }"#,
    ] {
        assert!(serde_json::from_str::<StreamRequest>(bad).is_err(), "{bad}");
    }
    for (bad, field) in [
        (
            StreamRequest {
                width: 321,
                ..defaults
            },
            "width",
        ),
        (StreamRequest { fps: 0, ..defaults }, "fps"),
    ] {
        assert_eq!(factory.validate(&bad).unwrap_err().field, field);
    }

    let mut source = factory.open(&RAW).unwrap();
    assert!(
        source
            .next_frame(Duration::from_millis(10))
            .unwrap()
            .is_idle()
    );
    source.close().unwrap();
}

#[tokio::test]
async fn forwarder_delivers_frames_end_to_end() {
    let (portal, source) = open();
    let sender = portal.connect().unwrap();
    let log = CallLog::new();
    let fake = FakeSource::new(&log, FakeSourceSettings::default());

    let received = tokio::task::spawn_blocking(move || {
        let mut source = source;
        let frame = receive(&mut source);
        (source, frame)
    });
    let mut got = None;
    let forwarded = tokio::time::timeout(
        BOUND * 2,
        Forwarder::new(fake, sender).run(async {
            got = Some(received.await.unwrap());
        }),
    )
    .await
    .expect("forwarder did not stop in time");

    // Closed only after forwarding stopped, so no send can race the close.
    forwarded.unwrap();
    let (source, got) = got.unwrap();
    Box::new(source).close().unwrap();
    assert_eq!(
        (got.width(), got.height(), got.format()),
        (16, 9, PixelFormat::RGB24)
    );
    assert_eq!(got.as_slice(), Frame::dummy(16, 9).unwrap().as_slice());
    assert_eq!(log.count("source.close:"), 1);
}
