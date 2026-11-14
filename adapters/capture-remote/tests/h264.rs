//! H.264 streams on loopback over WebSocket with the software encoder: round
//! trip, recovery, pause/resume and refusal. Every wait is bounded.
use prysm_capture_remote::testkit::{BOUND, Portal};
use prysm_capture_remote::{RemoteSender, RemoteSource, StreamEncoding, StreamRequest, WireError};
use prysm_core::Frame;
use prysm_h264::{Backend, Encoder, EncoderSettings, FORMAT, H264Error};
use prysm_ports::{FrameSink, FrameSource};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Software H.264; frames carry their own size, so the tests use several.
const SOFTWARE: StreamRequest = StreamRequest {
    width: 640,
    height: 280,
    fps: 30,
    include_ui: true,
    encoding: StreamEncoding::H264Software,
};
/// The request a receiver sends for [`SOFTWARE`].
const SOFTWARE_REQUEST: &str = r#"{"type":"request","width":640,"height":280,"fps":30,"include_ui":true,"encoding":"h264_software"}"#;

/// A frame message by hand: NV12 (limited, BT.709), the keyframe flag,
/// width, height, then `payload`.
fn nv12_message(keyframe: bool, width: u16, height: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![4, 1, 2, u8::from(keyframe)];
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

/// A portal with a source receiving `request`.
fn open(request: StreamRequest) -> (Portal, RemoteSource) {
    let portal = Portal::start();
    let source = portal.source(request);
    (portal, source)
}

/// A fake receiver: one upgraded connection that sent the [`SOFTWARE`]
/// request.
fn fake_receiver() -> (SocketAddr, thread::JoinHandle<WebSocket<TcpStream>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let receiver = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(BOUND)).unwrap();
        let mut socket = prysm_capture_remote::testkit::accept(stream);
        socket.send(Message::text(SOFTWARE_REQUEST)).unwrap();
        socket
    });
    (addr, receiver)
}

fn connect(addr: SocketAddr) -> RemoteSender {
    let url = format!("ws://{addr}/api/v1/capture/stream")
        .parse()
        .unwrap();
    RemoteSender::connect(&url, addr).unwrap()
}

/// Limited-range NV12: a horizontal luma ramp moved by `shift`, plus four
/// coloured chroma quadrants, like a picture with distinct screen edges.
fn picture(width: u32, height: u32, shift: u32) -> Frame {
    let (w, h) = (width as usize, height as usize);
    let mut data: Vec<u8> = (0..w * h)
        .map(|i| 16 + ((i % w + shift as usize) * 200 / w % 200) as u8)
        .collect();
    for row in 0..h / 2 {
        for col in 0..w / 2 {
            let (u, v) = match (row * 2 / (h / 2), col * 2 / (w / 2)) {
                (0, 0) => (90, 240),
                (0, _) => (54, 34),
                (_, 0) => (240, 110),
                _ => (128, 128),
            };
            data.extend([u, v]);
        }
    }
    Frame::new(data, width, height, FORMAT).unwrap()
}

fn mean_error(a: &Frame, b: &Frame) -> f64 {
    let sum: u64 = a
        .as_slice()
        .iter()
        .zip(b.as_slice())
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    sum as f64 / a.len() as f64
}

fn receive(source: &mut RemoteSource) -> Frame {
    try_receive(source, BOUND).unwrap_or_else(|| panic!("no frame within {BOUND:?}"))
}

fn try_receive(source: &mut RemoteSource, within: Duration) -> Option<Frame> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Some(frame) = source
            .next_frame(Duration::from_millis(20))
            .unwrap()
            .into_frame()
        {
            return Some(frame);
        }
    }
    None
}

/// Send, then wait until the receiver produced that frame (frames are not
/// queued, so this keeps the comparison one-to-one).
fn round_trip(sender: &mut RemoteSender, source: &mut RemoteSource, frame: &Frame) -> Frame {
    sender.send(frame).unwrap();
    receive(source)
}

/// The first cause of type `T` in an adapter error's chain.
fn cause<T: std::error::Error + 'static>(error: &prysm_ports::Error) -> Option<&T> {
    let prysm_ports::Error::Adapter {
        source: Some(cause),
        ..
    } = error
    else {
        return None;
    };
    let mut cause: &(dyn std::error::Error + 'static) = &**cause;
    loop {
        if let Some(found) = cause.downcast_ref::<T>() {
            return Some(found);
        }
        cause = cause.source()?;
    }
}

#[test]
fn software_h264_round_trips_within_tolerance_and_reports_its_bitrate() {
    let (portal, mut source) = open(SOFTWARE);
    let mut sender = portal.connect().unwrap();
    let (width, height) = (640, 280);
    let mut worst: f64 = 0.0;
    for i in 0..30 {
        let sent = picture(width, height, i * 7);
        let got = round_trip(&mut sender, &mut source, &sent);
        assert_eq!(
            (got.width(), got.height(), got.format()),
            (width, height, FORMAT)
        );
        worst = worst.max(mean_error(&sent, &got));
    }
    assert!(worst < 4.0, "mean absolute NV12 error {worst}");
    Box::new(source).close().unwrap();
}

/// Bytes a receiver sees for one second of 640x280 at 30 fps, the
/// qualified size and target bitrate (printed; `--nocapture` shows them).
/// (Message bytes; WebSocket framing adds 6 to 14 more per message.)
fn wire_bytes(frames: impl Iterator<Item = Frame>) -> usize {
    let (addr, receiver) = fake_receiver();
    let mut sender = connect(addr);
    let mut socket = receiver.join().unwrap();
    let counter = thread::spawn(move || {
        let mut total = 0;
        while let Ok(message) = socket.read() {
            total += message.len();
        }
        total
    });
    for frame in frames {
        sender.send(&frame).unwrap();
    }
    drop(sender);
    counter.join().unwrap()
}

#[test]
fn h264_needs_a_small_fraction_of_raw() {
    let raw = 30 * (8 + 640 * 280 * 3 / 2);
    let moving = wire_bytes((0..30).map(|i| picture(640, 280, i * 7)));
    // Worst case: new noise every frame, held to the target by rate control.
    let mut seed = 1_u32;
    let noise = wire_bytes((0..30).map(|_| {
        let data = (0..640 * 280 * 3 / 2)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                16 + (seed >> 24) as u8 % 220
            })
            .collect();
        Frame::new(data, 640, 280, FORMAT).unwrap()
    }));
    let mbit = |bytes: usize| bytes as f64 * 8.0 / 1e6;
    eprintln!(
        "640x280 at 30 fps, 2 Mbit/s target: moving ramp {:.2} Mbit/s, noise {:.2} Mbit/s, raw NV12 {:.1} Mbit/s",
        mbit(moving),
        mbit(noise),
        mbit(raw)
    );
    assert!(moving > 0 && moving < raw / 10, "{moving} of {raw}");
    assert!(noise < raw / 4, "{noise} of {raw}");
}

/// A raw client speaking H.264 with its own encoder, to drop pictures.
struct Client {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    encoder: Encoder,
}

impl Client {
    fn connect(portal: &Portal) -> Self {
        let mut socket = portal.client();
        let Message::Text(request) = socket.read().unwrap() else {
            panic!("no request");
        };
        assert_eq!(request.as_str(), SOFTWARE_REQUEST);
        let encoder = Encoder::new(EncoderSettings::new(Backend::Software, 30)).unwrap();
        Self { socket, encoder }
    }

    /// Encode `frame`; send it unless `drop` (a lost picture). Returns the keyframe flag.
    fn encode(&mut self, frame: &Frame, drop: bool) -> bool {
        let picture = self.encoder.encode(frame).unwrap().unwrap();
        if !drop {
            let message = nv12_message(
                picture.keyframe,
                frame.width() as u16,
                frame.height() as u16,
                picture.data,
            );
            self.socket.send(Message::binary(message)).unwrap();
        }
        picture.keyframe
    }

    /// The next command the receiver sent.
    fn command(&mut self) -> String {
        let Message::Text(command) = self.socket.read().unwrap() else {
            panic!("no command");
        };
        command.as_str().to_owned()
    }
}

#[test]
fn a_lost_picture_requests_a_keyframe_and_the_stream_recovers() {
    let (portal, mut source) = open(SOFTWARE);
    let mut client = Client::connect(&portal);
    let frames: Vec<Frame> = (0..6).map(|i| picture(64, 32, i * 9)).collect();

    assert!(client.encode(&frames[0], false));
    assert!(mean_error(&receive(&mut source), &frames[0]) < 4.0);
    client.encode(&frames[1], false);
    receive(&mut source);
    // Picture 2 is lost; picture 3 references it.
    client.encode(&frames[2], true);
    client.encode(&frames[3], false);
    assert_eq!(
        client.command(),
        r#"{"type":"keyframe"}"#,
        "a keyframe request"
    );
    assert!(try_receive(&mut source, Duration::from_millis(200)).is_none());
    // Later pictures of the broken session are skipped, not misdecoded.
    client.encode(&frames[4], false);
    assert!(try_receive(&mut source, Duration::from_millis(200)).is_none());
    // A new session's keyframe decodes as the current picture.
    client.encoder.restart();
    assert!(client.encode(&frames[5], false));
    assert!(mean_error(&receive(&mut source), &frames[5]) < 4.0);
    Box::new(source).close().unwrap();
}

#[test]
fn a_stream_must_start_with_a_keyframe() {
    let (portal, mut source) = open(SOFTWARE);
    let mut client = Client::connect(&portal);
    client.encode(&picture(64, 32, 0), true);
    client.encode(&picture(64, 32, 5), false);
    assert_eq!(client.command(), r#"{"type":"keyframe"}"#);
    assert!(try_receive(&mut source, Duration::from_millis(200)).is_none());
    client.encoder.restart();
    client.encode(&picture(64, 32, 10), false);
    receive(&mut source);
    Box::new(source).close().unwrap();
}

/// A keyframe that does not decode ends the connection; the receiver keeps
/// listening for the sender's next one.
#[test]
fn a_broken_keyframe_drops_the_connection() {
    let (portal, mut source) = open(SOFTWARE);
    let mut client = Client::connect(&portal);
    let broken = nv12_message(true, 64, 32, b"\x00\x00\x01\x65\x00");
    client.socket.send(Message::binary(broken)).unwrap();
    loop {
        match client.socket.read() {
            Ok(_) => {}
            Err(tungstenite::Error::Io(error)) => {
                assert!(
                    error.kind() != std::io::ErrorKind::WouldBlock
                        && error.kind() != std::io::ErrorKind::TimedOut,
                    "dropped, not stalled"
                );
                break;
            }
            Err(_) => break,
        }
    }
    let mut sender = portal.connect().unwrap();
    round_trip(&mut sender, &mut source, &picture(64, 32, 0));
    Box::new(source).close().unwrap();
}

/// A fake receiver that records frame headers' keyframe flags and sends
/// control bytes on request.
struct Recorder {
    socket: WebSocket<TcpStream>,
}

impl Recorder {
    /// The next frame's keyframe flag; its payload is as long as it says.
    fn frame(&mut self) -> bool {
        let Message::Binary(frame) = self.socket.read().unwrap() else {
            panic!("no frame");
        };
        assert!(frame.len() > 8, "a header and a picture");
        frame[3] == 1
    }

    fn nothing_for(&mut self, wait: Duration) -> bool {
        self.socket.get_ref().set_read_timeout(Some(wait)).unwrap();
        let quiet = self.socket.read().is_err();
        self.socket.get_ref().set_read_timeout(Some(BOUND)).unwrap();
        quiet
    }

    fn command(&mut self, command: &str) {
        self.socket.send(Message::text(command)).unwrap();
        // Let it arrive before the sender's next poll.
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn pause_stops_sending_and_resume_starts_with_a_keyframe() {
    let (addr, receiver) = fake_receiver();
    let mut sender = connect(addr);
    let mut recorder = Recorder {
        socket: receiver.join().unwrap(),
    };
    let mut next = (0..).map(|i| picture(64, 32, i * 11));

    sender.send(&next.next().unwrap()).unwrap();
    assert!(recorder.frame(), "a session starts with a keyframe");
    sender.send(&next.next().unwrap()).unwrap();
    assert!(!recorder.frame());

    recorder.command(r#"{"type":"pause"}"#);
    for _ in 0..3 {
        sender.send(&next.next().unwrap()).unwrap();
        assert!(sender.paused());
    }
    assert!(recorder.nothing_for(Duration::from_millis(150)));

    recorder.command(r#"{"type":"resume"}"#);
    sender.send(&next.next().unwrap()).unwrap();
    assert!(!sender.paused());
    assert!(recorder.frame(), "resume starts a new session");
    sender.send(&next.next().unwrap()).unwrap();
    assert!(!recorder.frame());

    recorder.command(r#"{"type":"keyframe"}"#);
    sender.send(&next.next().unwrap()).unwrap();
    assert!(recorder.frame());

    // An unknown control byte fails the connection.
    recorder.command(r#"{"type":"jump"}"#);
    let error = sender.send(&next.next().unwrap()).unwrap_err();
    assert!(
        matches!(cause::<WireError>(&error), Some(WireError::Json(_))),
        "{error}"
    );
}

/// Closing the source (lighting off) pauses the sender, then ends the stream.
#[test]
fn close_pauses_the_sender_before_ending_the_stream() {
    let (portal, mut source) = open(SOFTWARE);
    let mut sender = portal.connect().unwrap();
    round_trip(&mut sender, &mut source, &picture(64, 32, 0));
    Box::new(source).close().unwrap();
    let deadline = Instant::now() + BOUND;
    let error = loop {
        assert!(Instant::now() < deadline, "the sender never noticed");
        if let Err(error) = sender.send(&picture(64, 32, 1)) {
            break error;
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(sender.paused(), "{error}");
    assert_eq!(cause::<WireError>(&error), Some(&WireError::Closed));
}

/// The requested hardware encoder is not available (any desktop): the send
/// fails with an encoder cause, nothing is sent in software instead, and the
/// receiver's source fails with the sender's reason.
#[test]
fn an_unavailable_hardware_encoder_is_refused_without_switching() {
    let (portal, mut source) = open(StreamRequest {
        encoding: StreamEncoding::H264Hardware,
        ..SOFTWARE
    });
    let mut sender = portal.connect().unwrap();
    assert_eq!(sender.request().encoding, StreamEncoding::H264Hardware);
    let error = sender.send(&picture(64, 32, 0)).unwrap_err();
    assert!(cause::<H264Error>(&error).is_some(), "{error}");
    let deadline = Instant::now() + BOUND;
    let refused = loop {
        assert!(Instant::now() < deadline, "no refusal");
        match source.next_frame(Duration::from_millis(20)) {
            Err(error) => break error,
            Ok(pull) => assert!(pull.is_idle(), "a frame was sent"),
        }
    };
    // The encoding is named as the setting the sender cannot produce.
    let prysm_ports::Error::Invalid(invalid) = &refused else {
        panic!("{refused:?}");
    };
    assert_eq!(invalid.field, "capture.encoding");
    assert!(
        invalid
            .reason
            .starts_with("the sender cannot stream 640x280 at 30 fps with menus as h264_hardware: cannot encode frame: ")
            && invalid.reason.contains("hardware"),
        "{}",
        invalid.reason
    );
    Box::new(source).close().unwrap();
}

/// H.264 needs the capture's NV12: other pixels are an encoder failure.
#[test]
fn h264_of_other_pixels_is_refused() {
    let (portal, mut source) = open(SOFTWARE);
    let mut sender = portal.connect().unwrap();
    let error = sender.send(&Frame::dummy(16, 16).unwrap()).unwrap_err();
    assert!(cause::<H264Error>(&error).is_some(), "{error}");
    let deadline = Instant::now() + BOUND;
    while source.next_frame(Duration::from_millis(20)).is_ok() {
        assert!(Instant::now() < deadline, "no refusal");
    }
    Box::new(source).close().unwrap();
}
