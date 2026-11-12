use crate::capture::{RequestedCapture, settings};
use crate::link::{ForwarderLink, Uplink, Upstream, slim_encoding};
use crate::video::VideoFeed;
use prysm_capture_remote::{RemoteSender, RemoteSource, StreamEncoding, StreamRequest, WireError};
use prysm_capture_webos::WebosSettings;
use prysm_core::Frame;
use prysm_h264::H264Error;
use prysm_ports::{Error, FrameSink, FrameSource, Pull, Result};
use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{CallLog, FakeSource, FakeSourceSettings};
use prysm_webos_link::worker::{Link, Peer, Stop};
use prysm_webos_link::{Command, Event, LightingTarget, Video, WorkerStatus};
use prysm_webos_link::{FailureKind, SlimEncoding};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

#[derive(Clone, Default)]
struct Feed(Arc<Mutex<Vec<String>>>);

impl VideoFeed for Feed {
    fn set(&self, video: Video) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{video:?}"));
    }
    fn reset(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push("reset".into());
    }
}

/// The host task: its result and the stop reason.
type Running = tokio::task::JoinHandle<(Result<()>, Option<Stop>)>;

const RAW: StreamRequest = StreamRequest {
    width: 320,
    height: 180,
    fps: 30,
    include_ui: true,
    encoding: StreamEncoding::Raw,
};

const SOFTWARE: StreamRequest = StreamRequest {
    width: 64,
    height: 32,
    encoding: StreamEncoding::H264Software,
    ..RAW
};

/// Opens a fake capture for every requested shape, recording the shapes.
#[derive(Clone)]
struct Opened {
    log: CallLog,
    shapes: Arc<Mutex<Vec<WebosSettings>>>,
    fake: FakeSourceSettings,
    /// NV12 pictures instead of the fake's RGB.
    nv12: bool,
}

impl Opened {
    fn fake(log: &CallLog, fake: FakeSourceSettings) -> Self {
        Self {
            log: log.clone(),
            shapes: Arc::default(),
            fake,
            nv12: false,
        }
    }

    fn nv12() -> Self {
        Self {
            nv12: true,
            ..Self::fake(&CallLog::new(), FakeSourceSettings::default())
        }
    }

    fn shapes(&self) -> Vec<WebosSettings> {
        self.shapes.lock().unwrap().clone()
    }

    fn open(&self, shape: WebosSettings) -> Result<Box<dyn FrameSource>> {
        self.shapes.lock().unwrap().push(shape);
        Ok(if self.nv12 {
            Box::new(Nv12Source(0, shape.width, shape.height))
        } else {
            Box::new(FakeSource::new(&self.log, self.fake.clone()))
        })
    }
}

/// The worker, with `connect` opening its uplink (retried every 20 ms) and
/// `opened` the capture.
fn start<S: Upstream + 'static>(
    opened: &Opened,
    connect: impl FnMut() -> Result<S> + Send + 'static,
) -> (Peer, Feed, Running) {
    let (link, peer) = Link::memory();
    let feed = Feed::default();
    let stop = link.stop.clone();
    let (sink, flow) = Uplink::new(connect);
    let sink = sink.retry_every(Duration::from_millis(20));
    let source = RequestedCapture::new(flow.clone(), {
        let opened = opened.clone();
        move |shape| opened.open(shape)
    });
    let surface = ForwarderLink::new(link, feed.clone(), flow)
        .with_timing(Duration::from_millis(30), Duration::from_millis(100));
    let task = tokio::spawn(async move {
        let result = Forwarder::new(source, sink)
            .surface(surface)
            .run(stop.stopped())
            .await;
        (result, stop.reason())
    });
    (peer, feed, task)
}

/// A worker whose receiver always accepts a raw stream.
fn start_fake(fake: FakeSourceSettings) -> (Peer, Feed, Running) {
    let log = CallLog::new();
    let sink = log.clone();
    start(&Opened::fake(&log, fake), move || {
        Ok(Fake::new(&sink, RAW, u32::MAX))
    })
}

fn status(event: &Event) -> Option<&WorkerStatus> {
    match event {
        Event::Status(status) => Some(status),
        Event::Faulted { .. } => None,
    }
}

fn capturing(event: &Event) -> Option<bool> {
    status(event).map(|status| status.capturing)
}

fn waiting(event: &Event) -> Option<&str> {
    status(event)
        .filter(|status| !status.capturing && status.failure_kind == Some(FailureKind::Receiver))
        .and_then(|status| status.failure.as_deref())
}

fn encoder_failure(event: &Event) -> Option<&str> {
    status(event)
        .filter(|status| status.failure_kind == Some(FailureKind::Encoder))
        .and_then(|status| status.failure.as_deref())
}

fn encoding(event: &Event) -> Option<SlimEncoding> {
    status(event).and_then(|status| status.encoding)
}

/// A receiver connection: records `sink.send` and `sink.refuse`, and fails
/// after `left` frames like a receiver whose host stopped lighting (or, with
/// `encoder`, like a failing encoder).
struct Fake {
    log: CallLog,
    request: StreamRequest,
    left: u32,
    encoder: bool,
}

impl Fake {
    fn new(log: &CallLog, request: StreamRequest, left: u32) -> Self {
        Self {
            log: log.clone(),
            request,
            left,
            encoder: false,
        }
    }
}

impl Upstream for Fake {
    fn request(&self) -> StreamRequest {
        self.request
    }
    fn paused(&self) -> bool {
        false
    }
    fn refuse(&mut self, reason: &str) {
        self.log.record(format!("sink.refuse:{reason}"));
    }
}

impl FrameSink for Fake {
    fn send(&mut self, _frame: &Frame) -> Result<()> {
        if self.left == 0 {
            if self.encoder {
                return Err(Error::adapter(
                    "cannot encode frame",
                    H264Error::Settings("no encoder here"),
                ));
            }
            return Err(Error::other("receiver closed the connection"));
        }
        self.left -= 1;
        self.log.record("sink.send");
        Ok(())
    }
}

#[tokio::test]
async fn an_unavailable_receiver_is_a_reported_wait_not_a_failure() {
    let log = CallLog::new();
    let attempts = Arc::new(AtomicU32::new(0));
    let opened = Opened::fake(&log, FakeSourceSettings::default());
    let (mut peer, _feed, task) = start(&opened, {
        let (log, attempts) = (log.clone(), attempts.clone());
        move || {
            // Refused three times, then a receiver that drops after 5 frames,
            // then one that stays.
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            match attempt {
                0..=2 => Err(Error::other("cannot connect to receiver: refused")),
                3 => Ok(Fake::new(&log, RAW, 5)),
                _ => Ok(Fake::new(&log, RAW, u32::MAX)),
            }
        }
    });
    let event = peer.wait_for(|e| waiting(e).is_some()).await.unwrap();
    assert_eq!(
        waiting(&event),
        Some("waiting for the receiver: cannot connect to receiver: refused")
    );
    // Connected: frames flow and no wait is reported.
    peer.wait_for(|e| capturing(e) == Some(true) && status(e).is_some_and(|s| s.failure.is_none()))
        .await
        .unwrap();
    // The first receiver drops after 5 frames; the uplink reconnects.
    assert!(log.wait_for("sink.send", 20).await);
    assert!(attempts.load(Ordering::SeqCst) >= 5);
    assert!(!task.is_finished(), "the worker keeps running throughout");
    // The capture opened once, at the requested shape, and stayed open.
    assert_eq!(opened.shapes(), [settings(&RAW)]);

    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// A real receiver that binds after the sender started, as when lighting is
/// switched on at the headless host while the TV is already forwarding.
#[tokio::test]
async fn frames_reach_a_receiver_that_starts_listening_later() {
    let address = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let log = CallLog::new();
    let (mut peer, _feed, task) = start(
        &Opened::fake(&log, FakeSourceSettings::default()),
        move || RemoteSender::connect(address),
    );
    peer.wait_for(|e| waiting(e).is_some()).await.unwrap();

    let mut receiver = RemoteSource::bind(address, RAW).unwrap();
    let (frame, receiver) = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(std::time::Instant::now() < deadline, "no frame received");
            if let Pull::Frame(frame) = receiver.next_frame(Duration::from_millis(50)).unwrap() {
                return (frame, receiver);
            }
        }
    })
    .await
    .unwrap();
    assert_eq!((frame.width(), frame.height()), (16, 9));
    peer.wait_for(|e| capturing(e) == Some(true) && encoding(e) == Some(SlimEncoding::Raw))
        .await
        .unwrap();
    Box::new(receiver).close().unwrap();

    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// The receiver's settings change: it closes and listens again with a new
/// request. The sender's next connection gets it, and the capture reopens
/// at the new shape.
#[tokio::test]
async fn a_new_request_reaches_the_tv_on_the_next_connection() {
    let mut receiver = RemoteSource::bind("127.0.0.1:0".parse().unwrap(), SOFTWARE).unwrap();
    let address = receiver.local_addr();
    let opened = Opened::nv12();
    let (mut peer, _feed, task) = start(&opened, move || RemoteSender::connect(address));
    peer.wait_for(|e| {
        capturing(e) == Some(true) && encoding(e) == Some(SlimEncoding::H264Software)
    })
    .await
    .unwrap();
    let frame = tokio::task::spawn_blocking(move || {
        let frame = receiver
            .next_frame(Duration::from_secs(5))
            .unwrap()
            .into_frame()
            .expect("a decoded frame");
        Box::new(receiver).close().unwrap();
        frame
    })
    .await
    .unwrap();
    assert_eq!((frame.width(), frame.height()), (64, 32));

    let changed = StreamRequest {
        width: 96,
        height: 48,
        fps: 15,
        include_ui: false,
        encoding: StreamEncoding::Raw,
    };
    let mut receiver = RemoteSource::bind(address, changed).unwrap();
    let frame = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(std::time::Instant::now() < deadline, "no frame received");
            if let Pull::Frame(frame) = receiver.next_frame(Duration::from_millis(50)).unwrap() {
                Box::new(receiver).close().unwrap();
                return frame;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        (frame.width(), frame.height(), frame.format()),
        (96, 48, prysm_h264::FORMAT)
    );
    peer.wait_for(|e| encoding(e).is_none()).await.unwrap();
    assert_eq!(opened.shapes(), [settings(&SOFTWARE), settings(&changed)]);
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// A receiver of another wire version refuses the stream: the worker keeps
/// running and its status names the mismatch.
#[tokio::test]
async fn a_receiver_of_another_wire_version_is_reported_as_the_wait() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut hello = [0; 8];
            if stream.read_exact(&mut hello).is_ok() {
                let _ = stream.write_all(b"PRSM\x02\x01\x00\x00");
            }
        }
    });
    let log = CallLog::new();
    let (mut peer, _feed, task) = start(
        &Opened::fake(&log, FakeSourceSettings::default()),
        move || RemoteSender::connect(address),
    );
    let event = peer
        .wait_for(|e| waiting(e).is_some_and(|reason| reason.contains("version 2")))
        .await
        .unwrap();
    let reason = waiting(&event).unwrap();
    assert!(
        reason.contains("refused the stream") && reason.contains("this end version 3"),
        "{reason}"
    );
    assert!(!task.is_finished(), "the worker keeps running");
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

#[tokio::test]
async fn reports_frame_flow_and_forwards_video_commands() {
    let (mut peer, feed, task) = start_fake(FakeSourceSettings {
        quiet_after: Some(40),
        ..FakeSourceSettings::default()
    });
    assert_eq!(
        peer.event().await.unwrap(),
        Event::Status(WorkerStatus::default())
    );
    peer.wait_for(|e| capturing(e) == Some(true)).await.unwrap();
    // The fake goes quiet after 40 frames.
    peer.wait_for(|e| capturing(e) == Some(false))
        .await
        .unwrap();
    // Heartbeat repeats the unchanged status.
    let again = peer.event().await.unwrap();
    assert_eq!(capturing(&again), Some(false));

    peer.send(Command::Video {
        video: Video::Unavailable,
    });
    peer.send(Command::ResetVideo {});
    peer.close();
    let (result, stop) = task.await.unwrap();
    result.unwrap();
    assert_eq!(stop, Some(Stop::Requested("stdin closed")));
    assert_eq!(
        feed.0.lock().unwrap().clone(),
        ["Unavailable".to_owned(), "reset".to_owned()]
    );
}

#[tokio::test]
async fn lighting_is_protocol_misuse() {
    let (peer, _feed, task) = start_fake(FakeSourceSettings::default());
    peer.send(Command::Lighting {
        target: LightingTarget::Processed,
    });
    let (result, stop) = task.await.unwrap();
    result.unwrap();
    assert!(matches!(stop, Some(Stop::Failed(_))));
}

#[tokio::test]
async fn an_unconfirmed_release_fails_the_forwarder() {
    let (mut peer, _feed, task) = start_fake(FakeSourceSettings {
        fail_close: true,
        ..FakeSourceSettings::default()
    });
    peer.wait_for(|e| capturing(e) == Some(true)).await.unwrap();
    peer.close();
    let (result, _) = task.await.unwrap();
    assert!(result.unwrap_err().to_string().contains("unconfirmed"));
}

/// The TV capture's NV12 pictures at the requested size, every 5 ms.
struct Nv12Source(u32, u32, u32);

impl FrameSource for Nv12Source {
    fn next_frame(&mut self, _timeout: Duration) -> Result<Pull> {
        std::thread::sleep(Duration::from_millis(5));
        self.0 += 1;
        Ok(Pull::Frame(crate::check::picture(self.1, self.2, self.0)))
    }

    fn close(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

/// Software H.264 end to end: frames decode at a real receiver, and the
/// status names the encoder while connected.
#[tokio::test]
async fn software_h264_reaches_a_receiver_and_is_reported() {
    let mut receiver = RemoteSource::bind("127.0.0.1:0".parse().unwrap(), SOFTWARE).unwrap();
    let address = receiver.local_addr();
    let (mut peer, _feed, task) = start(&Opened::nv12(), move || RemoteSender::connect(address));
    peer.wait_for(|e| {
        capturing(e) == Some(true) && encoding(e) == Some(SlimEncoding::H264Software)
    })
    .await
    .unwrap();
    let (frame, receiver) = tokio::task::spawn_blocking(move || {
        let frame = receiver
            .next_frame(Duration::from_secs(5))
            .unwrap()
            .into_frame()
            .expect("a decoded frame");
        (frame, receiver)
    })
    .await
    .unwrap();
    assert_eq!(
        (frame.width(), frame.height(), frame.format()),
        (64, 32, prysm_h264::FORMAT)
    );

    // The receiver pauses the stream: reported as the wait, nothing counted.
    receiver.set_wanted(false);
    peer.wait_for(|e| {
        capturing(e) == Some(false)
            && waiting(e) == Some("the receiver paused the stream")
            && encoding(e) == Some(SlimEncoding::H264Software)
    })
    .await
    .unwrap();
    receiver.set_wanted(true);
    peer.wait_for(|e| capturing(e) == Some(true)).await.unwrap();
    Box::new(receiver).close().unwrap();
    // Lighting off: the receiver is gone, the worker waits and keeps running.
    peer.wait_for(|e| waiting(e).is_some() && encoding(e).is_none())
        .await
        .unwrap();
    assert!(!task.is_finished());
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// The requested hardware encoder is unavailable (any desktop): the worker
/// keeps running and reports it, the receiver's source fails with the TV's
/// reason, and nothing switches to software or raw.
#[tokio::test]
async fn an_unavailable_hardware_encoder_is_reported_on_both_ends() {
    let hardware = StreamRequest {
        encoding: StreamEncoding::H264Hardware,
        ..SOFTWARE
    };
    let mut receiver = RemoteSource::bind("127.0.0.1:0".parse().unwrap(), hardware).unwrap();
    let address = receiver.local_addr();
    let (mut peer, _feed, task) = start(&Opened::nv12(), move || RemoteSender::connect(address));
    let event = peer
        .wait_for(|e| encoder_failure(e).is_some())
        .await
        .unwrap();
    let failure = encoder_failure(&event).unwrap();
    assert!(failure.contains("hardware H.264"), "{failure}");
    assert_eq!(encoding(&event), None);
    assert!(!task.is_finished(), "the worker keeps running");

    let error = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(std::time::Instant::now() < deadline, "no refusal");
            match receiver.next_frame(Duration::from_millis(50)) {
                Err(error) => {
                    Box::new(receiver).close().unwrap();
                    return error;
                }
                Ok(pull) => assert!(pull.is_idle(), "a frame was sent"),
            }
        }
    })
    .await
    .unwrap();
    let text = prysm_webos_link::chain(&error);
    assert!(
        text.starts_with("the sender cannot stream 64x32 at 30 fps with menus as h264_hardware: ")
            && text.contains("hardware H.264"),
        "{text}"
    );
    let Error::Adapter {
        source: Some(cause),
        ..
    } = &error
    else {
        panic!("{error:?}");
    };
    assert!(matches!(
        cause.downcast_ref::<WireError>(),
        Some(WireError::Refused(_))
    ));

    // Still the encoder failure while the receiver is away.
    peer.wait_for(|e| encoder_failure(e).is_some())
        .await
        .unwrap();
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// After an encoder failure the same request is refused without trying the
/// encoder again; a different request is tried and clears the failure.
#[tokio::test]
async fn a_failed_request_is_refused_until_the_request_changes() {
    let log = CallLog::new();
    let attempts = Arc::new(AtomicU32::new(0));
    let opened = Opened::fake(&log, FakeSourceSettings::default());
    let (mut peer, _feed, task) = start(&opened, {
        let (log, attempts) = (log.clone(), attempts.clone());
        move || {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            let hardware = StreamRequest {
                encoding: StreamEncoding::H264Hardware,
                ..RAW
            };
            Ok(match attempt {
                // The encoder fails at the first frame.
                0 => Fake {
                    encoder: true,
                    ..Fake::new(&log, hardware, 0)
                },
                // The same request again: refused before any frame.
                1..=5 => Fake::new(&log, hardware, u32::MAX),
                // Software instead.
                _ => Fake::new(
                    &log,
                    StreamRequest {
                        encoding: StreamEncoding::H264Software,
                        ..RAW
                    },
                    u32::MAX,
                ),
            })
        }
    });
    let event = peer
        .wait_for(|e| encoder_failure(e).is_some())
        .await
        .unwrap();
    assert!(encoder_failure(&event).unwrap().contains("no encoder here"));
    peer.wait_for(|e| {
        capturing(e) == Some(true)
            && encoding(e) == Some(SlimEncoding::H264Software)
            && status(e).is_some_and(|s| s.failure.is_none())
    })
    .await
    .unwrap();
    let refusals: Vec<String> = log
        .events()
        .into_iter()
        .filter(|event| event.starts_with("sink.refuse:"))
        .collect();
    assert_eq!(refusals.len(), 5, "{refusals:?}");
    assert!(refusals.iter().all(|r| r.contains("no encoder here")));
    // The request kept the capture's shape, so it never reopened.
    assert_eq!(opened.shapes(), [settings(&RAW)]);
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

#[test]
fn requests_map_to_capture_settings_and_encodings() {
    let shape = settings(&StreamRequest::default());
    assert_eq!(
        (shape.width, shape.height, shape.fps, shape.include_ui),
        (320, 180, 30, true)
    );
    shape.validate().unwrap();
    for (encoding, slim) in [
        (StreamEncoding::H264Hardware, SlimEncoding::H264Hardware),
        (StreamEncoding::H264Software, SlimEncoding::H264Software),
        (StreamEncoding::Raw, SlimEncoding::Raw),
    ] {
        assert_eq!(slim_encoding(encoding), slim);
        assert_eq!(encoding.name(), slim.name());
    }
}
