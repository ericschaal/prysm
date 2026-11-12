//! Both ends on loopback with ephemeral ports. Every wait is bounded.
use prysm_capture_remote::{RemoteReceiver, RemoteReceiverSettings, RemoteSender, RemoteSource};
use prysm_core::{Frame, PixelFormat, YuvRange, YuvStandardMatrix};
use prysm_ports::{FrameSink, FrameSource, SourceFactory};
use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{CallLog, FakeSource, FakeSourceSettings};
use std::error::Error as _;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(5);

fn any_port() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
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
        if let Some(frame) = source.next_frame(Duration::from_millis(50)).unwrap() {
            return frame;
        }
    }
    panic!("no frame within {BOUND:?}");
}

/// Wait for the receiver to drop a raw client: EOF or reset, never a timeout.
fn assert_dropped(mut client: TcpStream) {
    client.set_read_timeout(Some(BOUND)).unwrap();
    let mut buf = [0; 16];
    match client.read(&mut buf) {
        Ok(0) => {}
        Ok(n) => panic!("receiver answered {n} bytes"),
        Err(error) => assert!(
            !matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "connection was not dropped: {error}"
        ),
    }
}

#[test]
fn round_trip_preserves_dimensions_format_and_pixels() {
    let mut source = RemoteSource::bind(any_port()).unwrap();
    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();

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

    Box::new(source).close().unwrap();
}

#[test]
fn next_frame_times_out_with_none() {
    let mut source = RemoteSource::bind(any_port()).unwrap();
    let started = Instant::now();
    assert!(
        source
            .next_frame(Duration::from_millis(30))
            .unwrap()
            .is_none()
    );
    assert!(started.elapsed() >= Duration::from_millis(30));
    Box::new(source).close().unwrap();
}

#[test]
fn a_slow_consumer_sees_only_the_newest_frame() {
    let mut source = RemoteSource::bind(any_port()).unwrap();
    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
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
            .is_none()
    );
    Box::new(source).close().unwrap();
}

#[test]
fn receiver_survives_disconnect_and_accepts_a_new_sender() {
    let mut source = RemoteSource::bind(any_port()).unwrap();
    let mut first = RemoteSender::connect(source.local_addr()).unwrap();
    first.send(&frame(1, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 1);
    drop(first);

    // A disconnect is not an error.
    assert!(
        source
            .next_frame(Duration::from_millis(200))
            .unwrap()
            .is_none()
    );

    let mut second = RemoteSender::connect(source.local_addr()).unwrap();
    second.send(&frame(2, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 2);
    Box::new(source).close().unwrap();
}

#[test]
fn a_new_sender_replaces_a_stalled_one() {
    let mut source = RemoteSource::bind(any_port()).unwrap();
    // Half a header, then silence: a crashed TV whose socket never closed.
    let mut stalled = TcpStream::connect(source.local_addr()).unwrap();
    stalled.write_all(b"PRSM\x01").unwrap();
    thread::sleep(Duration::from_millis(100));

    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
    sender.send(&frame(3, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 3);
    assert_dropped(stalled);
    Box::new(source).close().unwrap();
}

#[test]
fn malformed_and_oversized_headers_drop_only_that_connection() {
    let mut source = RemoteSource::bind(any_port()).unwrap();

    let mut garbage = TcpStream::connect(source.local_addr()).unwrap();
    garbage
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    assert_dropped(garbage);

    // Valid magic and version, 4096x4096 RGB and a 4 GiB payload: rejected
    // from the header alone, before anything is allocated.
    let mut header = b"PRSM\x01\x01\x00\x00".to_vec();
    header.extend_from_slice(&4096u32.to_be_bytes());
    header.extend_from_slice(&4096u32.to_be_bytes());
    header.extend_from_slice(&u32::MAX.to_be_bytes());
    let mut oversized = TcpStream::connect(source.local_addr()).unwrap();
    oversized.write_all(&header).unwrap();
    assert_dropped(oversized);
    assert!(
        source
            .next_frame(Duration::from_millis(50))
            .unwrap()
            .is_none()
    );

    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
    sender.send(&frame(4, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 4);
    Box::new(source).close().unwrap();
}

#[test]
fn close_is_prompt_frees_the_port_and_fails_the_sender() {
    let mut source = RemoteSource::bind(any_port()).unwrap();
    let addr = source.local_addr();
    let mut sender = RemoteSender::connect(addr).unwrap();
    sender.send(&frame(5, 4, 4)).unwrap();
    receive(&mut source);

    let started = Instant::now();
    Box::new(source).close().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );

    let deadline = Instant::now() + BOUND;
    let error = loop {
        assert!(Instant::now() < deadline, "sender never noticed the close");
        if let Err(error) = sender.send(&frame(6, 4, 4)) {
            break error;
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(error.source().is_some(), "{error}");

    let rebound = RemoteSource::bind(addr).unwrap();
    Box::new(rebound).close().unwrap();
}

#[test]
fn connecting_to_a_dead_peer_fails_with_a_source() {
    let addr = TcpListener::bind(any_port()).unwrap().local_addr().unwrap();
    let error = RemoteSender::connect(addr).unwrap_err();
    assert!(error.to_string().contains(&addr.to_string()), "{error}");
    assert!(error.source().is_some(), "{error}");
}

#[test]
fn factory_decodes_validates_and_opens() {
    let settings: RemoteReceiverSettings =
        serde_json::from_str(r#"{ "listen": "0.0.0.0:7380" }"#).unwrap();
    assert_eq!(settings.listen, "0.0.0.0:7380".parse().unwrap());
    assert!(RemoteReceiver.validate(&settings).is_ok());
    assert!(serde_json::from_str::<RemoteReceiverSettings>(r#"{ "listen": "nope" }"#).is_err());
    assert!(
        serde_json::from_str::<RemoteReceiverSettings>(r#"{ "listen": "0.0.0.0:1", "x": 1 }"#)
            .is_err()
    );

    let ephemeral = RemoteReceiverSettings { listen: any_port() };
    assert_eq!(
        RemoteReceiver.validate(&ephemeral).unwrap_err().field,
        "capture.listen"
    );
    // `open` itself does not re-validate; port 0 keeps the test hermetic.
    let mut source = RemoteReceiver.open(&ephemeral).unwrap();
    assert!(
        source
            .next_frame(Duration::from_millis(10))
            .unwrap()
            .is_none()
    );
    source.close().unwrap();
}

#[tokio::test]
async fn forwarder_delivers_frames_end_to_end() {
    let source = RemoteSource::bind(any_port()).unwrap();
    let sender = RemoteSender::connect(source.local_addr()).unwrap();
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
