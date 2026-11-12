//! Both ends on loopback with ephemeral ports. Every wait is bounded.
use prysm_capture_remote::{
    RemoteReceiver, RemoteReceiverSettings, RemoteSender, RemoteSource, VERSION, WireError,
};
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

/// Wait for the receiver to drop a raw client: EOF or reset, never a timeout.
/// Returns what it answered first (at most the 8-byte hello reply).
fn assert_dropped(mut client: TcpStream) -> Vec<u8> {
    client.set_read_timeout(Some(BOUND)).unwrap();
    let mut answer = Vec::new();
    let mut buf = [0; 16];
    loop {
        match client.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => answer.extend_from_slice(&buf[..n]),
            Err(error) => {
                assert!(
                    !matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ),
                    "connection was not dropped: {error}"
                );
                break;
            }
        }
    }
    assert!(answer.len() <= 8, "receiver answered {answer:?}");
    answer
}

/// A raw client that has completed the version-2 hello for a raw stream.
fn greeted(addr: SocketAddr) -> TcpStream {
    let mut client = TcpStream::connect(addr).unwrap();
    client.set_read_timeout(Some(BOUND)).unwrap();
    client.write_all(b"PRSM\x02\x00\x00\x00").unwrap();
    let mut reply = [0; 8];
    client.read_exact(&mut reply).unwrap();
    assert_eq!(&reply, b"PRSM\x02\x00\x00\x00");
    client
}

/// The typed wire cause of a sender error.
fn wire_cause(error: &prysm_ports::Error) -> WireError {
    let prysm_ports::Error::Adapter {
        source: Some(cause),
        ..
    } = error
    else {
        panic!("no cause: {error}");
    };
    cause
        .downcast_ref::<WireError>()
        .unwrap_or_else(|| panic!("no wire cause: {error}"))
        .clone()
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

    // NV12: 1.5 bytes per pixel, both planes intact.
    let format = PixelFormat::NV12 {
        range: YuvRange::Limited,
        matrix: YuvStandardMatrix::Bt709,
    };
    let nv12 = Frame::new((0..36).collect(), 6, 4, format).unwrap();
    sender.send(&nv12).unwrap();
    let got = receive(&mut source);
    assert_eq!((got.width(), got.height(), got.format()), (6, 4, format));
    assert_eq!(got.as_slice(), nv12.as_slice());

    Box::new(source).close().unwrap();
}

#[test]
fn next_frame_times_out_idle() {
    let mut source = RemoteSource::bind(any_port()).unwrap();
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
            .is_idle()
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
            .is_idle()
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
    stalled.write_all(b"PRSM\x02").unwrap();
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

    // A valid hello, then 4096x4096 RGB and a 4 GiB payload: rejected from
    // the header alone, before anything is allocated.
    let mut header = vec![1, 0, 0, 0];
    header.extend_from_slice(&4096u32.to_be_bytes());
    header.extend_from_slice(&4096u32.to_be_bytes());
    header.extend_from_slice(&u32::MAX.to_be_bytes());
    let mut oversized = greeted(source.local_addr());
    oversized.write_all(&header).unwrap();
    assert_eq!(assert_dropped(oversized), b"");
    assert!(
        source
            .next_frame(Duration::from_millis(50))
            .unwrap()
            .is_idle()
    );

    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
    sender.send(&frame(4, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 4);
    Box::new(source).close().unwrap();
}

#[test]
fn version_mismatches_are_typed_errors_on_both_ends() {
    // A version-1 sender opens with a frame header: refused with this
    // receiver's version, then dropped.
    let mut source = RemoteSource::bind(any_port()).unwrap();
    let mut old = TcpStream::connect(source.local_addr()).unwrap();
    old.write_all(b"PRSM\x01\x01\x00\x00\x00\x00\x00\x04")
        .unwrap();
    assert_eq!(
        assert_dropped(old),
        [b'P', b'R', b'S', b'M', VERSION, 1, 0, 0]
    );
    // A reserved encoding (1: H.264) is refused the same way.
    let mut h264 = TcpStream::connect(source.local_addr()).unwrap();
    h264.write_all(b"PRSM\x02\x01\x00\x00").unwrap();
    assert_eq!(
        assert_dropped(h264),
        [b'P', b'R', b'S', b'M', VERSION, 2, 0, 0]
    );
    // The receiver keeps serving current senders.
    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
    sender.send(&frame(9, 4, 4)).unwrap();
    assert_eq!(receive(&mut source).as_slice()[0], 9);
    Box::new(source).close().unwrap();

    // Receivers that answer with another version, or (version 1) not at all.
    for (answer, expected) in [
        (
            Some(*b"PRSM\x03\x01\x00\x00"),
            WireError::Version {
                ours: VERSION,
                theirs: 3,
            },
        ),
        (None, WireError::NoReply),
    ] {
        let listener = TcpListener::bind(any_port()).unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut hello = [0; 8];
            stream.read_exact(&mut hello).unwrap();
            match answer {
                Some(answer) => stream.write_all(&answer).unwrap(),
                None => drop(stream),
            }
        });
        let error = RemoteSender::connect(addr).unwrap_err();
        assert_eq!(wire_cause(&error), expected, "{error}");
        assert!(error.to_string().contains(&addr.to_string()), "{error}");
        peer.join().unwrap();
    }
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
            .is_idle()
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
