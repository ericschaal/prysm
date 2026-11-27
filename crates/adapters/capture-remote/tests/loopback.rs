//! Both ends on loopback over WebSocket, through a stand-in for the portal's
//! upgrade and hand-over. Every wait is bounded.
use prysm_capture_remote::testkit::{BOUND, Portal, failure, receive};
use prysm_capture_remote::{
    Close, Offer, RemoteReceiver, RemoteSender, RemoteSettings, RemoteSource, SendError, WireError,
};
use prysm_core::{Frame, PixelFormat, YuvRange, YuvStandardMatrix};
use prysm_ports::{FrameSink, FrameSource, Mode, Modes, Pull, SourceFactory, Span, StreamEncoding};
use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{CallLog, FakeSource, FakeSourceSettings};
use prysm_testkit::eventually;
use std::error::Error as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Uncompressed frames of any size and format.
const RAW: RemoteSettings = RemoteSettings {
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
    error
        .cause::<WireError>()
        .cloned()
        .unwrap_or_else(|| panic!("no wire cause: {error}"))
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
fn next_frame_times_out_quiet() {
    let (_portal, mut source) = open();
    let started = Instant::now();
    assert!(matches!(
        source.next_frame(Duration::from_millis(30)).unwrap(),
        Pull::Quiet
    ));
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
    eventually("the receiver to hold the last frame", BOUND, || {
        source
            .pending_frame()
            .filter(|frame| frame.as_slice()[0] == 20)
    });
    assert_eq!(receive(&mut source).as_slice()[0], 20, "frames were queued");
    assert!(matches!(
        source.next_frame(Duration::from_millis(100)).unwrap(),
        Pull::Quiet
    ));
    Box::new(source).close().unwrap();
}

#[test]
fn receiver_survives_disconnect_and_accepts_a_new_sender() {
    let (portal, mut source) = open();
    let mut first = portal.connect().unwrap();
    first.send(&frame(1, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 1);
    drop(first);

    assert!(matches!(
        source.next_frame(Duration::from_millis(200)).unwrap(),
        Pull::Quiet
    ));

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
    // Upgraded, then silence: a crashed TV whose socket never closed. The
    // receiver sends its request once it has adopted the connection.
    let stalled = greeted(&portal);

    let mut sender = portal.connect().unwrap();
    sender.send(&frame(3, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 3);
    assert_dropped(stalled);

    let mut second = portal.connect().unwrap();
    second.send(&frame(4, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 4);
    let error = eventually("the replaced sender to stop", BOUND, || sender.tick().err());
    assert_eq!(
        wire_cause(&error),
        WireError::Closed(Some(Close::Replaced)),
        "{error}"
    );
    Box::new(source).close().unwrap();
}

/// A source that closes while it waits for a replaced sender to answer the
/// close tells the newer sender it stopped instead of asking it for frames.
#[test]
fn a_close_while_replacing_tells_the_newer_sender_it_stopped() {
    let (portal, source) = open();
    let replaced = greeted(&portal);
    let mut newer = portal.client();
    // The replaced sender's close arrives and stays unanswered: the receive
    // thread waits for the answer.
    let MaybeTlsStream::Plain(socket) = replaced.get_ref() else {
        panic!("not plain TCP");
    };
    assert!(socket.peek(&mut [0; 1]).unwrap() > 0, "no close arrived");
    Box::new(source).close().unwrap();
    match newer.read() {
        Ok(Message::Close(Some(frame))) => assert_eq!(u16::from(frame.code), 4000),
        other => panic!("{other:?}"),
    }
    drop(replaced);
}

/// Each connection is checked against its own sender's offer: a sender that
/// sends none is not checked against an earlier sender's.
#[test]
fn a_sender_without_an_offer_is_not_checked_against_another_senders() {
    let portal = Portal::start();
    let h264_only = Offer::new([(
        StreamEncoding::H264Software,
        vec![Mode::new(
            Span::exactly(320),
            Span::exactly(180),
            Span::new(1, 30, 1).unwrap(),
        )],
    )]);
    let error = portal.connect_offering(&h264_only).unwrap_err();
    assert_eq!(wire_cause(&error), WireError::Unavailable(503));
    let mut source = portal.source(RAW);
    let mut client = greeted(&portal);
    client
        .send(Message::binary(raw_frame([1, 0, 0, 0], 2, 2, &[9; 12])))
        .unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 9);
    Box::new(source).close().unwrap();
}

/// A message this end does not take fails the source, and the sender hears
/// close 4002: the same sender would send it again.
#[test]
fn malformed_and_oversized_messages_fail_the_source() {
    let padded = format!(r#"{{"type":"stats","pad":"{}"}}"#, "x".repeat(4096));
    for (message, cause) in [
        (Message::text("GET / HTTP/1.1"), "invalid message"),
        (
            Message::binary(raw_frame([1, 0, 0, 0], 1921, 1080, &[])),
            "outside",
        ),
        (
            Message::binary(raw_frame([1, 0, 0, 0], 4, 4, &[0; 47])),
            "does not match",
        ),
        (Message::text(padded), "exceeds"),
        (
            Message::text(r#"{"refused":"request","reason":"x"}"#),
            "invalid message",
        ),
    ] {
        let (portal, mut source) = open();
        let mut client = greeted(&portal);
        client.send(message).unwrap();
        match client.read() {
            Ok(Message::Close(Some(frame))) => assert_eq!(u16::from(frame.code), 4002),
            other => panic!("{cause}: {other:?}"),
        }
        let error = failure(&mut source);
        assert!(wire_cause(&error).to_string().contains(cause), "{error}");
        Box::new(source).close().unwrap();
    }
}

/// What this end does not know is ignored: a sender message of another
/// type, unknown fields of a refusal's, and frame flag bits.
#[test]
fn unknown_messages_fields_and_flags_are_ignored() {
    let (portal, mut source) = open();
    let mut client = greeted(&portal);
    client
        .send(Message::text(r#"{"type":"stats","sent":3}"#))
        .unwrap();
    client
        .send(Message::binary(raw_frame([1, 0, 0, 0xff], 2, 2, &[9; 12])))
        .unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 9);
    // A refusal with a cause and a field this end does not know: a refusal
    // of the request.
    client
        .send(Message::text(
            r#"{"type":"refused","cause":"thermal","reason":"too hot","celsius":90}"#,
        ))
        .unwrap();
    let error = failure(&mut source);
    assert_eq!(wire_cause(&error), WireError::Refused("too hot".into()));
    Box::new(source).close().unwrap();
}

/// The version is the WebSocket subprotocol: a sender of another version is
/// refused at the upgrade, and either end says which versions differ.
#[test]
fn version_mismatches_are_typed_errors_on_both_ends() {
    let (portal, source) = open();
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

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let newer = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = [0; 1024];
        let _ = stream.read(&mut head).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 426 Upgrade Required\r\nsec-websocket-protocol: prysm-capture.v9, prysm-capture.v7\r\ncontent-length: 0\r\n\r\n",
            )
            .unwrap();
    });
    let url = format!("ws://{addr}/api/v1/capture/stream")
        .parse()
        .unwrap();
    let error =
        RemoteSender::connect(&url, addr, &prysm_capture_remote::testkit::offer()).unwrap_err();
    assert_eq!(
        wire_cause(&error),
        WireError::Version {
            ours: prysm_ports::CAPTURE_VERSION,
            theirs: Some(9)
        },
        "{error}"
    );
    newer.join().unwrap();
}

/// A receiver that sends `messages` after the upgrade, then reports what
/// the sender answered before closing.
fn receiver_sending(
    messages: &'static [&'static str],
) -> (std::net::SocketAddr, thread::JoinHandle<Vec<Message>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(BOUND)).unwrap();
        let mut socket = prysm_capture_remote::testkit::accept(stream);
        if messages.is_empty() {
            socket.close(None).unwrap();
        }
        for message in messages {
            socket.send(Message::text(*message)).unwrap();
        }
        let mut answers = Vec::new();
        while let Ok(message) = socket.read() {
            if !matches!(message, Message::Close(_)) {
                answers.push(message);
            }
        }
        answers
    });
    (addr, peer)
}

/// A receiver that sends no request fails the connect; one whose request
/// this sender cannot parse, validate or offer is refused, naming the
/// request as the cause.
#[test]
fn requests_this_end_cannot_take_are_refused() {
    let offer = Offer::new([(
        StreamEncoding::Raw,
        vec![Mode::new(
            Span::exactly(320),
            Span::exactly(180),
            Span::new(1, 30, 1).unwrap(),
        )],
    )]);
    for (messages, expected, refused) in [
        (&[][..], "the receiver did not send its request", false),
        (
            &[r#"{"type":"request","include_ui":9}"#][..],
            "invalid message: invalid type: integer `9`, expected a boolean",
            true,
        ),
        (
            &[
                r#"{"type":"request","width":320,"height":180,"fps":30,"include_ui":true,"encoding":"av1"}"#,
            ][..],
            "invalid stream request: invalid encoding: expected one of h264_hardware, h264_software, raw",
            true,
        ),
        (
            &[
                r#"{"type":"request","width":321,"height":180,"fps":30,"include_ui":true,"encoding":"raw"}"#,
            ][..],
            "invalid stream request: invalid width: must be an even number from 2 to 1920",
            true,
        ),
        (
            &[
                r#"{"type":"request","width":640,"height":360,"fps":30,"include_ui":true,"encoding":"raw"}"#,
            ][..],
            "invalid stream request: invalid width: ",
            true,
        ),
    ] {
        let (addr, peer) = receiver_sending(messages);
        let url = format!("ws://{addr}/api/v1/capture/stream")
            .parse()
            .unwrap();
        let error = RemoteSender::connect(&url, addr, &offer).unwrap_err();
        let cause = prysm_ports::chain(&wire_cause(&error));
        assert!(cause.starts_with(expected), "{cause}");
        assert!(error.to_string().contains(&addr.to_string()), "{error}");
        let answers = peer.join().unwrap();
        if refused {
            let [Message::Text(refusal)] = &answers[..] else {
                panic!("{answers:?}");
            };
            let refusal: serde_json::Value = serde_json::from_str(refusal).unwrap();
            assert_eq!(refusal["type"], "refused");
            assert_eq!(refusal["cause"], "request");
            assert!(
                refusal["reason"].as_str().unwrap().starts_with(expected),
                "{refusal}"
            );
        } else {
            assert!(answers.is_empty(), "{answers:?}");
        }
    }
}

/// A receiver that closes with a code before its request (it stopped) fails
/// the connect with that close, not as a missing request.
#[test]
fn a_close_code_before_the_request_is_the_cause() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(BOUND)).unwrap();
        let mut socket = prysm_capture_remote::testkit::accept(stream);
        let stopped = tungstenite::protocol::CloseFrame {
            code: 4000.into(),
            reason: "".into(),
        };
        socket.close(Some(stopped)).unwrap();
        while socket.read().is_ok() {}
    });
    let url = format!("ws://{addr}/api/v1/capture/stream")
        .parse()
        .unwrap();
    let error =
        RemoteSender::connect(&url, addr, &prysm_capture_remote::testkit::offer()).unwrap_err();
    assert_eq!(
        wire_cause(&error),
        WireError::Closed(Some(Close::Stopped)),
        "{error}"
    );
    peer.join().unwrap();
}

/// Messages of types this end does not know may precede the request.
#[test]
fn unknown_messages_before_the_request_are_skipped() {
    let (addr, peer) = receiver_sending(&[
        r#"{"type":"hello","prysm":"9.0.0"}"#,
        r#"{"type":"request","width":320,"height":180,"fps":30,"include_ui":true,"encoding":"raw","hdr":true}"#,
    ]);
    let url = format!("ws://{addr}/api/v1/capture/stream")
        .parse()
        .unwrap();
    let sender =
        RemoteSender::connect(&url, addr, &prysm_capture_remote::testkit::offer()).unwrap();
    assert_eq!(sender.request(), RAW);
    drop(sender);
    peer.join().unwrap();
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
    let sender = portal.connect().unwrap();
    assert_eq!(sender.request(), RAW);
    Box::new(source).close().unwrap();
    drop(sender);

    let changed = RemoteSettings {
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
        let error = failure(&mut source);
        assert!(
            source.next_frame(Duration::ZERO).is_err(),
            "the failure stays"
        );
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

    let error = refused(|sender| {
        let error = sender.deliver(&frame(0, 1922, 2)).unwrap_err();
        assert!(matches!(error, SendError::Refused(_)), "{error}");
    });
    assert_eq!(
        wire_cause(&error),
        WireError::Refused("cannot send frame: frame 1922x2 is outside 1x1 to 1920x1080".into())
    );

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

/// Raw at 320x180 or 640x360, 1 to 30 fps; no H.264.
fn raw_offer() -> Offer {
    let size = |w, h| {
        Mode::new(
            Span::exactly(w),
            Span::exactly(h),
            Span::new(1, 30, 1).unwrap(),
        )
    };
    Offer::new([(StreamEncoding::Raw, vec![size(320, 180), size(640, 360)])])
}

#[test]
fn a_refused_attempt_still_tells_the_receiver_what_the_sender_offers() {
    let factory = RemoteReceiver::default();
    let portal = Portal::serving(factory.incoming());
    assert_eq!(factory.modes(&RAW), Modes::Unknown);
    let error = portal.connect_offering(&raw_offer()).unwrap_err();
    assert_eq!(wire_cause(&error), WireError::Unavailable(503));
    assert_eq!(factory.modes(&RAW), raw_offer().modes(StreamEncoding::Raw));
    let software = RemoteSettings {
        encoding: StreamEncoding::H264Software,
        ..RAW
    };
    assert_eq!(factory.modes(&software), Modes::Known(Vec::new()));

    let invalid = |request: &RemoteSettings| match factory.open(request) {
        Err(prysm_ports::Error::Invalid(invalid)) => invalid.field,
        Err(error) => panic!("{error:?}"),
        Ok(_) => panic!("{request} opened"),
    };
    assert_eq!(invalid(&software), "encoding");
    assert_eq!(invalid(&RemoteSettings { fps: 60, ..RAW }), "fps");
    assert_eq!(invalid(&RemoteSettings { height: 360, ..RAW }), "height");
    factory.open(&RAW).unwrap().close().unwrap();
}

#[test]
fn a_sender_that_does_not_offer_the_request_fails_the_source_unasked() {
    let portal = Portal::start();
    let mut source = portal.source(RemoteSettings {
        width: 640,
        height: 360,
        fps: 60,
        ..RAW
    });
    let error = portal.connect_offering(&raw_offer()).unwrap_err();
    assert_eq!(wire_cause(&error), WireError::Closed(Some(Close::Failed)));
    let error = failure(&mut source);
    let prysm_ports::Error::Invalid(invalid) = &error else {
        panic!("{error:?}");
    };
    assert_eq!(invalid.field, "capture.fps");
    assert_eq!(
        invalid.reason,
        "the sender cannot stream 640x360 at 60 fps with menus as raw: \
         at 640x360 this source delivers a frame rate of 1 to 30"
    );
    Box::new(source).close().unwrap();
}

#[test]
fn close_is_prompt_stops_receiving_and_fails_the_sender() {
    let (portal, mut source) = open();
    let mut sender = portal.connect().unwrap();
    sender.send(&frame(5, 4, 4)).unwrap();
    receive(&mut source);
    // A sender that reads, as the TV does at every frame or tick.
    let reading = thread::spawn(move || {
        eventually("the sender to notice the close", BOUND, || {
            sender.tick().err()
        })
    });

    let started = Instant::now();
    Box::new(source).close().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(!portal.handoff.is_receiving());
    let error = reading.join().unwrap();
    assert_eq!(
        wire_cause(&error),
        WireError::Closed(Some(Close::Stopped)),
        "{error}"
    );
}

/// Stream full-size raw frames from `sender` on a thread until a send
/// fails, and return the failure.
fn stream_until_closed(mut sender: RemoteSender) -> thread::JoinHandle<prysm_ports::Error> {
    let big = frame(7, 1920, 1080);
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "the sender never saw the close");
            if let Err(error) = sender.send(&big) {
                return error;
            }
        }
    })
}

/// A sender that is streaming large frames when its connection ends still
/// receives the close code: the receiver reads off its frames until the
/// sender answers, so unread frames do not reset the connection first.
#[test]
fn a_streaming_sender_receives_the_close_code() {
    let (portal, mut source) = open();
    let streaming = stream_until_closed(portal.connect().unwrap());
    receive(&mut source);
    let second = portal.connect().unwrap();
    let error = streaming.join().unwrap();
    assert_eq!(
        wire_cause(&error),
        WireError::Closed(Some(Close::Replaced)),
        "{error}"
    );
    drop(second);
    Box::new(source).close().unwrap();

    let (portal, mut source) = open();
    let streaming = stream_until_closed(portal.connect().unwrap());
    receive(&mut source);
    Box::new(source).close().unwrap();
    let error = streaming.join().unwrap();
    assert_eq!(
        wire_cause(&error),
        WireError::Closed(Some(Close::Stopped)),
        "{error}"
    );
}

/// A sender with nothing to send (the slim TV stops capturing while
/// stopped) still reads the receiver's messages through `tick`: a source
/// that closes (lighting stops) ends the stream with the stopped code.
#[test]
fn idle_reads_the_close_of_a_stopped_receiver() {
    let (portal, mut source) = open();
    let mut sender = portal.connect().unwrap();
    sender.send(&frame(1, 4, 4)).unwrap();
    receive(&mut source);
    sender.tick().unwrap();

    Box::new(source).close().unwrap();
    let error = eventually("the sender to notice the close", BOUND, || {
        sender.tick().err()
    });
    assert_eq!(
        wire_cause(&error),
        WireError::Closed(Some(Close::Stopped)),
        "{error}"
    );
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
    let error =
        RemoteSender::connect(&url, addr, &prysm_capture_remote::testkit::offer()).unwrap_err();
    assert!(error.to_string().contains(&addr.to_string()), "{error}");
    assert!(error.source().is_some(), "{error}");
}

#[test]
fn factory_decodes_validates_and_opens() {
    let factory = RemoteReceiver::default();
    let defaults: RemoteSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(defaults, RemoteSettings::default());
    assert!(factory.validate(&defaults).is_ok());
    let full: RemoteSettings = serde_json::from_str(
        r#"{ "width": 640, "height": 280, "fps": 25, "include_ui": false, "encoding": "raw" }"#,
    )
    .unwrap();
    assert_eq!(
        full,
        RemoteSettings {
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
        assert!(
            serde_json::from_str::<RemoteSettings>(bad).is_err(),
            "{bad}"
        );
    }
    for (bad, field) in [
        (
            RemoteSettings {
                width: 321,
                ..defaults
            },
            "width",
        ),
        (RemoteSettings { fps: 0, ..defaults }, "fps"),
    ] {
        assert_eq!(factory.validate(&bad).unwrap_err().field, field);
    }

    let mut source = factory.open(&RAW).unwrap();
    assert!(matches!(
        source.next_frame(Duration::from_millis(10)).unwrap(),
        Pull::Quiet
    ));
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
        Forwarder::new(fake, sender, BOUND).run(async {
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
