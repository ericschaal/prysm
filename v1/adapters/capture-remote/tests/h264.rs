//! H.264 streams on loopback with the software encoder: round trip, recovery,
//! pause/resume and refusal. Every wait is bounded.
use prysm_capture_remote::{RemoteSender, RemoteSource, StreamEncoding, StreamRequest, WireError};
use prysm_core::Frame;
use prysm_h264::{Backend, Encoder, EncoderSettings, FORMAT, H264Error};
use prysm_ports::{FrameSink, FrameSource};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(5);
/// Software H.264; frames carry their own size, so the tests use several.
const SOFTWARE: StreamRequest = StreamRequest {
    width: 640,
    height: 280,
    fps: 30,
    include_ui: true,
    encoding: StreamEncoding::H264Software,
};
/// The reply a fake receiver sends for [`SOFTWARE`].
const SOFTWARE_REPLY: &[u8; 16] = b"PRSM\x03\x00\x00\x00\x02\x01\x02\x80\x01\x18\x1e\x00";

fn bind(request: StreamRequest) -> RemoteSource {
    RemoteSource::bind(any_port(), request).unwrap()
}

fn any_port() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
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
    let mut source = bind(SOFTWARE);
    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
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
fn wire_bytes(frames: impl Iterator<Item = Frame>) -> usize {
    let listener = TcpListener::bind(any_port()).unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut hello = [0; 8];
        stream.read_exact(&mut hello).unwrap();
        stream.write_all(SOFTWARE_REPLY).unwrap();
        let mut total = Vec::new();
        stream.read_to_end(&mut total).unwrap();
        total.len()
    });
    let mut sender = RemoteSender::connect(addr).unwrap();
    for frame in frames {
        sender.send(&frame).unwrap();
    }
    drop(sender);
    counter.join().unwrap()
}

#[test]
fn h264_needs_a_small_fraction_of_raw() {
    let raw = 30 * (16 + 640 * 280 * 3 / 2);
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
    stream: TcpStream,
    encoder: Encoder,
}

impl Client {
    fn connect(addr: SocketAddr) -> Self {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(BOUND)).unwrap();
        stream.write_all(b"PRSM\x03\x00\x00\x00").unwrap();
        let mut reply = [0; 16];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, SOFTWARE_REPLY);
        let encoder = Encoder::new(EncoderSettings::new(Backend::Software, 30)).unwrap();
        Self { stream, encoder }
    }

    /// Encode `frame`; send it unless `drop` (a lost picture). Returns the keyframe flag.
    fn encode(&mut self, frame: &Frame, drop: bool) -> bool {
        let picture = self.encoder.encode(frame).unwrap().unwrap();
        if !drop {
            let mut header = vec![4, 1, 2, u8::from(picture.keyframe)];
            header.extend_from_slice(&frame.width().to_be_bytes());
            header.extend_from_slice(&frame.height().to_be_bytes());
            header.extend_from_slice(&(picture.data.len() as u32).to_be_bytes());
            header.extend_from_slice(picture.data);
            self.stream.write_all(&header).unwrap();
        }
        picture.keyframe
    }

    fn control(&mut self) -> u8 {
        let mut byte = [0];
        self.stream.read_exact(&mut byte).unwrap();
        byte[0]
    }
}

#[test]
fn a_lost_picture_requests_a_keyframe_and_the_stream_recovers() {
    let mut source = bind(SOFTWARE);
    let mut client = Client::connect(source.local_addr());
    let frames: Vec<Frame> = (0..6).map(|i| picture(64, 32, i * 9)).collect();

    assert!(client.encode(&frames[0], false));
    assert!(mean_error(&receive(&mut source), &frames[0]) < 4.0);
    client.encode(&frames[1], false);
    receive(&mut source);
    // Picture 2 is lost; picture 3 references it.
    client.encode(&frames[2], true);
    client.encode(&frames[3], false);
    assert_eq!(client.control(), 3, "a keyframe request");
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
    let mut source = bind(SOFTWARE);
    let mut client = Client::connect(source.local_addr());
    client.encode(&picture(64, 32, 0), true);
    client.encode(&picture(64, 32, 5), false);
    assert_eq!(client.control(), 3);
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
    let mut source = bind(SOFTWARE);
    let mut client = Client::connect(source.local_addr());
    let mut header = vec![4, 1, 2, 1];
    header.extend_from_slice(&64u32.to_be_bytes());
    header.extend_from_slice(&32u32.to_be_bytes());
    header.extend_from_slice(&5u32.to_be_bytes());
    header.extend_from_slice(b"\x00\x00\x01\x65\x00");
    client.stream.write_all(&header).unwrap();
    let mut rest = Vec::new();
    assert!(
        client.stream.read_to_end(&mut rest).is_ok(),
        "dropped, not stalled"
    );
    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
    round_trip(&mut sender, &mut source, &picture(64, 32, 0));
    Box::new(source).close().unwrap();
}

/// A fake receiver that records frame headers' keyframe flags and sends
/// control bytes on request.
struct Recorder {
    stream: TcpStream,
}

impl Recorder {
    fn accept(listener: &TcpListener) -> Self {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(BOUND)).unwrap();
        let mut hello = [0; 8];
        stream.read_exact(&mut hello).unwrap();
        stream.write_all(SOFTWARE_REPLY).unwrap();
        Self { stream }
    }

    /// The next frame's keyframe flag.
    fn frame(&mut self) -> bool {
        let mut header = [0; 16];
        self.stream.read_exact(&mut header).unwrap();
        let len = u32::from_be_bytes(header[12..16].try_into().unwrap());
        let mut payload = vec![0; len as usize];
        self.stream.read_exact(&mut payload).unwrap();
        header[3] == 1
    }

    fn nothing_for(&mut self, wait: Duration) -> bool {
        self.stream.set_read_timeout(Some(wait)).unwrap();
        let quiet = self.stream.read(&mut [0; 1]).is_err();
        self.stream.set_read_timeout(Some(BOUND)).unwrap();
        quiet
    }

    fn control(&mut self, code: u8) {
        self.stream.write_all(&[code]).unwrap();
        // Let it arrive before the sender's next poll.
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn pause_stops_sending_and_resume_starts_with_a_keyframe() {
    let listener = TcpListener::bind(any_port()).unwrap();
    let addr = listener.local_addr().unwrap();
    let receiver = thread::spawn(move || Recorder::accept(&listener));
    let mut sender = RemoteSender::connect(addr).unwrap();
    let mut recorder = receiver.join().unwrap();
    let mut next = (0..).map(|i| picture(64, 32, i * 11));

    sender.send(&next.next().unwrap()).unwrap();
    assert!(recorder.frame(), "a session starts with a keyframe");
    sender.send(&next.next().unwrap()).unwrap();
    assert!(!recorder.frame());

    recorder.control(1); // pause
    for _ in 0..3 {
        sender.send(&next.next().unwrap()).unwrap();
        assert!(sender.paused());
    }
    assert!(recorder.nothing_for(Duration::from_millis(150)));

    recorder.control(2); // resume
    sender.send(&next.next().unwrap()).unwrap();
    assert!(!sender.paused());
    assert!(recorder.frame(), "resume starts a new session");
    sender.send(&next.next().unwrap()).unwrap();
    assert!(!recorder.frame());

    recorder.control(3); // keyframe request
    sender.send(&next.next().unwrap()).unwrap();
    assert!(recorder.frame());

    // An unknown control byte fails the connection.
    recorder.control(9);
    let error = sender.send(&next.next().unwrap()).unwrap_err();
    assert_eq!(cause::<WireError>(&error), Some(&WireError::Control(9)));
}

#[test]
fn a_real_receiver_pauses_and_resumes_its_sender() {
    let mut source = bind(SOFTWARE);
    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
    round_trip(&mut sender, &mut source, &picture(64, 32, 0));

    source.set_wanted(false);
    let deadline = Instant::now() + BOUND;
    while !sender.paused() {
        assert!(Instant::now() < deadline, "never paused");
        sender.send(&picture(64, 32, 1)).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    // Nothing more arrives while paused (a frame in flight may still land).
    let _ = try_receive(&mut source, Duration::from_millis(100));
    sender.send(&picture(64, 32, 2)).unwrap();
    assert!(try_receive(&mut source, Duration::from_millis(150)).is_none());

    source.set_wanted(true);
    let deadline = Instant::now() + BOUND;
    while sender.paused() {
        assert!(Instant::now() < deadline, "never resumed");
        sender.send(&picture(64, 32, 3)).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    let sent = picture(64, 32, 4);
    let got = round_trip(&mut sender, &mut source, &sent);
    assert!(mean_error(&got, &sent) < 8.0);
    Box::new(source).close().unwrap();
}

/// Closing the source (lighting off) pauses the sender, then ends the stream.
#[test]
fn close_pauses_the_sender_before_ending_the_stream() {
    let mut source = bind(SOFTWARE);
    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
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
    let mut source = bind(StreamRequest {
        encoding: StreamEncoding::H264Hardware,
        ..SOFTWARE
    });
    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
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
    let mut source = bind(SOFTWARE);
    let mut sender = RemoteSender::connect(source.local_addr()).unwrap();
    let error = sender.send(&Frame::dummy(16, 16).unwrap()).unwrap_err();
    assert!(cause::<H264Error>(&error).is_some(), "{error}");
    let deadline = Instant::now() + BOUND;
    while source.next_frame(Duration::from_millis(20)).is_ok() {
        assert!(Instant::now() < deadline, "no refusal");
    }
    Box::new(source).close().unwrap();
}
