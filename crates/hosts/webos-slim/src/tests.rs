use crate::capture::{RequestedCapture, settings};
use crate::link::{ForwarderLink, Uplink, Upstream};
use crate::resolve::Resolver;
use prysm_capture_remote::testkit::Portal;
use prysm_capture_remote::{RemoteSender, RemoteSettings, SendError};
use prysm_capture_webos::{Readiness, VideoSignal, WebosSettings};
use prysm_core::Frame;
use prysm_ports::StreamEncoding;
use prysm_ports::{Error, FrameSource, Pull, Result};
use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{CallLog, FakeSource, FakeSourceSettings};
use prysm_webos_link::worker::{Link, Peer, Stop, StopHandle};
use prysm_webos_link::{Command, Event, LightingTarget, SlimStatus, Video, WorkerStatus};
use prysm_webos_link::{FailureKind, SlimLink};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The host task: its result and the stop reason.
type Running = tokio::task::JoinHandle<(Result<()>, Option<Stop>)>;

const RAW: RemoteSettings = RemoteSettings {
    width: 320,
    height: 180,
    fps: 30,
    include_ui: true,
    encoding: StreamEncoding::Raw,
};

const SOFTWARE: RemoteSettings = RemoteSettings {
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
    /// How many NV12 captures were closed.
    closed: Arc<AtomicU32>,
    fake: FakeSourceSettings,
    /// NV12 pictures instead of the fake's RGB.
    nv12: bool,
}

impl Opened {
    fn fake(log: &CallLog, fake: FakeSourceSettings) -> Self {
        Self {
            log: log.clone(),
            shapes: Arc::default(),
            closed: Arc::default(),
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

    fn closed(&self) -> u32 {
        self.closed.load(Ordering::SeqCst)
    }

    fn open(&self, shape: WebosSettings) -> Result<Box<dyn FrameSource>> {
        self.shapes.lock().unwrap().push(shape);
        Ok(if self.nv12 {
            Box::new(Nv12Source {
                count: 0,
                width: shape.width,
                height: shape.height,
                closed: self.closed.clone(),
            })
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
) -> (Peer, VideoSignal, Running) {
    let (link, peer) = Link::memory();
    let feed = VideoSignal::default();
    let stop = link.stop.clone();
    let (sink, flow) = Uplink::new(connect);
    let sink = sink.retry_every(Duration::from_millis(20));
    let source = RequestedCapture::new(flow.clone(), StopHandle::default(), {
        let opened = opened.clone();
        move |shape| opened.open(shape)
    });
    let surface = ForwarderLink::new(link, feed.clone(), flow)
        .with_timing(Duration::from_millis(30), Duration::from_millis(100));
    let task = tokio::spawn(async move {
        let result = Forwarder::new(source, sink, crate::STOP_DEADLINE)
            .surface(surface)
            .run(stop.stopped())
            .await;
        (result, stop.reason())
    });
    (peer, feed, task)
}

/// A worker whose receiver always accepts a raw stream.
fn start_fake(fake: FakeSourceSettings) -> (Peer, VideoSignal, Running) {
    let log = CallLog::new();
    let sink = log.clone();
    start(&Opened::fake(&log, fake), move || {
        Ok(Fake::new(&sink, RAW, u32::MAX))
    })
}

/// A status event's report; the slim worker sends only slim ones.
fn status(event: &Event) -> Option<&SlimStatus> {
    match event {
        Event::Status(WorkerStatus::Slim(status)) => Some(status),
        Event::Status(WorkerStatus::Full(status)) => panic!("a full status: {status:?}"),
        Event::Failed { .. } | Event::Faulted { .. } => None,
    }
}

fn capturing(event: &Event) -> Option<bool> {
    status(event).map(|status| status.capturing)
}

fn link(event: &Event) -> Option<&SlimLink> {
    status(event).map(|status| &status.link)
}

/// Paused, and no frames left from before.
fn paused(event: &Event) -> bool {
    status(event).is_some_and(|status| status.link == SlimLink::Paused {} && !status.capturing)
}

/// The link's wait or failure message, if it is of `kind`.
fn failure(event: &Event, kind: FailureKind) -> Option<&str> {
    link(event)
        .and_then(SlimLink::failure)
        .filter(|failure| failure.kind == kind)
        .map(|failure| failure.message.as_str())
}

fn waiting(event: &Event) -> Option<&str> {
    failure(event, FailureKind::Receiver).filter(|_| capturing(event) == Some(false))
}

fn version_differs(event: &Event) -> Option<&str> {
    failure(event, FailureKind::Version)
}

fn encoder_failure(event: &Event) -> Option<&str> {
    failure(event, FailureKind::Encoder)
}

fn replaced(event: &Event) -> Option<&str> {
    failure(event, FailureKind::Replaced)
}

fn encoding(event: &Event) -> Option<StreamEncoding> {
    match link(event)? {
        SlimLink::Sending { encoding, .. } => Some(*encoding),
        _ => None,
    }
}

/// Sending, without a wait or a failure.
fn sending(event: &Event) -> bool {
    encoding(event).is_some()
}

/// Whether the capture, once it closed (within 5 s), closed only once.
async fn released(opened: &Opened) -> bool {
    prysm_testkit::eventually_async("the capture to close", Duration::from_secs(5), || {
        (opened.closed() > 0).then_some(())
    })
    .await;
    opened.closed() == 1
}

/// A receiver connection: records `sink.send` and `sink.refuse`, and fails
/// after `left` frames like a receiver whose host stopped lighting (or, with
/// `encoder`, like a failing encoder).
struct Fake {
    log: CallLog,
    request: RemoteSettings,
    left: u32,
    encoder: bool,
}

impl Fake {
    fn new(log: &CallLog, request: RemoteSettings, left: u32) -> Self {
        Self {
            log: log.clone(),
            request,
            left,
            encoder: false,
        }
    }
}

impl Upstream for Fake {
    fn request(&self) -> RemoteSettings {
        self.request
    }
    fn refuse_encoding(&mut self, reason: &str) {
        self.log.record(format!("sink.refuse:{reason}"));
    }
    fn deliver(&mut self, _frame: &Frame) -> std::result::Result<(), SendError> {
        if self.left == 0 {
            if self.encoder {
                return Err(SendError::Refused(Error::adapter(
                    "cannot encode frame",
                    std::io::Error::other("no encoder here"),
                )));
            }
            return Err(Error::other("receiver closed the connection").into());
        }
        self.left -= 1;
        self.log.record("sink.send");
        Ok(())
    }
    fn tick(&mut self) -> Result<()> {
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
    let event = peer
        .wait_for("a waiting status", |e| waiting(e).is_some())
        .await;
    assert_eq!(
        waiting(&event),
        Some("waiting for the receiver: cannot connect to receiver: refused")
    );
    // Connected: frames flow and no wait is reported.
    peer.wait_for("a capturing status that is sending", |e| {
        capturing(e) == Some(true) && sending(e)
    })
    .await;
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

/// A sender that connects to Prysm through a portal stand-in.
fn connect(portal: &Portal) -> impl FnMut() -> Result<RemoteSender> + Send + 'static {
    let (url, address) = (portal.url(), portal.addr);
    move || RemoteSender::connect(&url, address, &prysm_capture_remote::testkit::offer())
}

/// A real receiver that starts after the sender, as when lighting is switched
/// on at the headless host while the TV is already forwarding: until then
/// the portal answers `503` and the sender waits.
#[tokio::test]
async fn frames_reach_a_receiver_that_starts_later() {
    let portal = Portal::start();
    let log = CallLog::new();
    let (mut peer, _feed, task) = start(
        &Opened::fake(&log, FakeSourceSettings::default()),
        connect(&portal),
    );
    let event = peer
        .wait_for("a waiting status", |e| waiting(e).is_some())
        .await;
    assert!(waiting(&event).unwrap().contains("503"), "{event:?}");

    let mut receiver = portal.source(RAW);
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
    peer.wait_for("a capturing status sending raw", |e| {
        capturing(e) == Some(true) && encoding(e) == Some(StreamEncoding::Raw)
    })
    .await;
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
    let portal = Portal::start();
    let mut receiver = portal.source(SOFTWARE);
    let opened = Opened::nv12();
    let (mut peer, _feed, task) = start(&opened, connect(&portal));
    peer.wait_for("a capturing status sending software H.264", |e| {
        capturing(e) == Some(true) && encoding(e) == Some(StreamEncoding::H264Software)
    })
    .await;
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

    let changed = RemoteSettings {
        width: 96,
        height: 48,
        fps: 15,
        include_ui: false,
        encoding: StreamEncoding::Raw,
    };
    let mut receiver = portal.source(changed);
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
    peer.wait_for("a status without an encoding", |e| encoding(e).is_none())
        .await;
    assert_eq!(opened.shapes(), [settings(&SOFTWARE), settings(&changed)]);
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// A Prysm of another capture stream version refuses the upgrade: the
/// worker keeps running and reports a `version` failure naming both
/// versions.
#[tokio::test]
async fn a_prysm_of_another_version_is_reported_as_such() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let attempts = Arc::new(AtomicU32::new(0));
    let counted = attempts.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            counted.fetch_add(1, Ordering::SeqCst);
            let mut head = [0; 1024];
            if stream.read(&mut head).is_ok() {
                let _ = stream.write_all(
                    b"HTTP/1.1 426 Upgrade Required\r\nsec-websocket-protocol: prysm-capture.v9\r\ncontent-length: 0\r\n\r\n",
                );
            }
        }
    });
    let url: tungstenite::http::Uri = format!("ws://{address}/api/v1/capture/stream")
        .parse()
        .unwrap();
    let log = CallLog::new();
    let (mut peer, _feed, task) = start(
        &Opened::fake(&log, FakeSourceSettings::default()),
        move || RemoteSender::connect(&url, address, &prysm_capture_remote::testkit::offer()),
    );
    let event = peer
        .wait_for("a version mismatch", |e| version_differs(e).is_some())
        .await;
    let reason = version_differs(&event).unwrap();
    assert!(
        reason.contains("Prysm: 9") && reason.contains("update the older one"),
        "{reason}"
    );
    assert!(attempts.load(Ordering::SeqCst) >= 1);
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
        Event::Status(WorkerStatus::Slim(SlimStatus::default()))
    );
    peer.wait_for("a capturing status", |e| capturing(e) == Some(true))
        .await;
    // The fake goes quiet after 40 frames.
    peer.wait_for("a status that is not capturing", |e| {
        capturing(e) == Some(false)
    })
    .await;
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
        feed.current(),
        Readiness {
            video: Video::Unavailable,
            generation: 1
        },
        "set, then one reset"
    );
}

#[tokio::test]
async fn a_target_is_protocol_misuse() {
    let (peer, _feed, task) = start_fake(FakeSourceSettings::default());
    peer.send(Command::Target {
        target: LightingTarget::ScreenSync,
        sequence: 1,
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
    peer.wait_for("a capturing status", |e| capturing(e) == Some(true))
        .await;
    peer.close();
    let (result, _) = task.await.unwrap();
    assert!(result.unwrap_err().to_string().contains("unconfirmed"));
}

/// The TV capture's NV12 pictures at the requested size, every 5 ms.
struct Nv12Source {
    count: u32,
    width: u32,
    height: u32,
    closed: Arc<AtomicU32>,
}

impl FrameSource for Nv12Source {
    fn next_frame(&mut self, _timeout: Duration) -> Result<Pull> {
        std::thread::sleep(Duration::from_millis(5));
        self.count += 1;
        Ok(Pull::Frame(crate::check::picture(
            self.width,
            self.height,
            self.count,
        )))
    }

    fn close(self: Box<Self>) -> Result<()> {
        self.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Software H.264 end to end: frames decode at a real receiver, and the
/// status names the encoder while connected.
#[tokio::test]
async fn software_h264_reaches_a_receiver_and_is_reported() {
    let portal = Portal::start();
    let mut receiver = portal.source(SOFTWARE);
    let opened = Opened::nv12();
    let (mut peer, _feed, task) = start(&opened, connect(&portal));
    peer.wait_for("a capturing status sending software H.264", |e| {
        capturing(e) == Some(true) && encoding(e) == Some(StreamEncoding::H264Software)
    })
    .await;
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

    // While frames go out, the rate is reported.
    peer.wait_for(
        "a status sending frames",
        |e| matches!(link(e), Some(SlimLink::Sending { fps, .. }) if *fps > 0),
    )
    .await;

    // Lighting off: the receiver closes the stream as stopped. Paused, not a
    // failure; the TV capture stops (and stays stopped while the worker
    // keeps reconnecting: `link::tests::every_uplink_state_takes_every_event`).
    Box::new(receiver).close().unwrap();
    peer.wait_for("a paused status", paused).await;
    assert!(released(&opened).await, "the capture stops while stopped");
    assert_eq!(opened.shapes().len(), 1, "no capture while lighting is off");
    // Lighting on again: a new connection resumes the capture.
    let receiver = portal.source(SOFTWARE);
    peer.wait_for("a capturing status that is sending", |e| {
        capturing(e) == Some(true) && sending(e)
    })
    .await;
    assert_eq!(
        opened.shapes(),
        [settings(&SOFTWARE); 2],
        "reopened on resume"
    );
    Box::new(receiver).close().unwrap();
    assert!(!task.is_finished());
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// Another TV takes over the stream: this worker releases the capture,
/// reports `replaced` and connects no more, so the two never take the
/// stream from each other in turn.
#[tokio::test]
async fn a_replaced_sender_gives_up() {
    let portal = Portal::start();
    let receiver = portal.source(RAW);
    let opened = Opened::nv12();
    let (mut peer, _feed, task) = start(&opened, connect(&portal));
    peer.wait_for("a capturing status", |e| capturing(e) == Some(true))
        .await;

    let mut other = connect(&portal);
    let other = tokio::task::spawn_blocking(move || other().unwrap())
        .await
        .unwrap();
    let event = peer
        .wait_for("a replaced status", |e| replaced(e).is_some())
        .await;
    assert!(
        replaced(&event)
            .unwrap()
            .ends_with("another sender replaced this one at Prysm"),
        "{event:?}"
    );
    // That it never connects again is `link::tests::attempts_follow_the_state`.
    assert!(released(&opened).await, "the capture is released");
    assert_eq!(opened.shapes().len(), 1, "and not opened again");
    assert!(!task.is_finished(), "the worker keeps running");

    drop(other);
    tokio::task::spawn_blocking(move || Box::new(receiver).close().unwrap())
        .await
        .unwrap();
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// A desktop updated while its lighting was off: the TV stopped by the old
/// Prysm shows the version failure, not a pause.
#[tokio::test]
async fn a_version_mismatch_replaces_a_stopped_stream() {
    use std::io::{Read, Write};
    let portal = Portal::start();
    let receiver = portal.source(RAW);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let newer = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = [0; 1024];
            if stream.read(&mut head).is_ok() {
                let _ = stream.write_all(
                    b"HTTP/1.1 426 Upgrade Required\r\nsec-websocket-protocol: prysm-capture.v9\r\ncontent-length: 0\r\n\r\n",
                );
            }
        }
    });
    let newer_url: tungstenite::http::Uri = format!("ws://{newer}/api/v1/capture/stream")
        .parse()
        .unwrap();
    let mut old = connect(&portal);
    let attempts = AtomicU32::new(0);
    let (mut peer, _feed, task) = start(&Opened::nv12(), move || {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            old()
        } else {
            RemoteSender::connect(&newer_url, newer, &prysm_capture_remote::testkit::offer())
        }
    });
    peer.wait_for("a capturing status", |e| capturing(e) == Some(true))
        .await;
    tokio::task::spawn_blocking(move || Box::new(receiver).close().unwrap())
        .await
        .unwrap();
    // The capture stays released: nothing can be sent to that Prysm.
    peer.wait_for("a version mismatch without capture", |e| {
        version_differs(e).is_some() && capturing(e) == Some(false)
    })
    .await;
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// A receiver that vanishes without a close code (a crash, the network)
/// keeps the capture running.
#[tokio::test]
async fn a_receiver_lost_without_a_close_code_keeps_the_capture() {
    let log = CallLog::new();
    let opened = Opened::fake(&log, FakeSourceSettings::default());
    let attempts = Arc::new(AtomicU32::new(0));
    let (mut peer, _feed, task) = start(&opened, {
        let (log, attempts) = (log.clone(), attempts.clone());
        move || match attempts.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(Fake::new(&log, RAW, 3)),
            _ => Err(Error::other("cannot connect to receiver: refused")),
        }
    });
    peer.wait_for("a waiting status naming the refusal", |e| {
        waiting(e).is_some_and(|w| w.contains("refused"))
    })
    .await;
    assert!(!log.events().iter().any(|e| e.starts_with("source.close")));
    assert_eq!(opened.shapes(), [settings(&RAW)]);
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// The receiver given by name: resolved at each attempt, so a name that
/// does not resolve yet is the wait, and frames flow once it does.
#[tokio::test]
async fn a_receiver_host_name_is_resolved_at_each_attempt() {
    static RESOLVES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
    let portal = Portal::start();
    let receiver = portal.source(RAW);
    PORT.store(portal.addr.port(), Ordering::SeqCst);
    let prysm: prysm_webos_link::PrysmAddress = "http://prysm.local:8787".parse().unwrap();
    let url = prysm.capture_stream();
    let mut resolver = Resolver::new(prysm).with_lookup(
        |host, _| {
            assert_eq!(host, "prysm.local");
            if RESOLVES.load(Ordering::SeqCst) {
                Ok(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    PORT.load(Ordering::SeqCst),
                )))
            } else {
                Err(std::io::Error::other("no such host"))
            }
        },
        Duration::from_millis(100),
    );
    let log = CallLog::new();
    let (mut peer, _feed, task) = start(
        &Opened::fake(&log, FakeSourceSettings::default()),
        move || {
            RemoteSender::connect(
                &url,
                resolver.resolve()?,
                &prysm_capture_remote::testkit::offer(),
            )
        },
    );
    let event = peer
        .wait_for("a waiting status", |e| waiting(e).is_some())
        .await;
    assert_eq!(
        waiting(&event),
        Some("waiting for the receiver: cannot resolve http://prysm.local:8787: no such host")
    );
    RESOLVES.store(true, Ordering::SeqCst);
    peer.wait_for("a capturing status", |e| capturing(e) == Some(true))
        .await;
    Box::new(receiver).close().unwrap();
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// A resolver that never answers delays neither the reconnects nor the
/// stop: each attempt waits at most its bound.
#[tokio::test]
async fn a_hanging_lookup_does_not_hold_up_the_stop() {
    let prysm: prysm_webos_link::PrysmAddress = "http://prysm.local:8787".parse().unwrap();
    let url = prysm.capture_stream();
    let mut resolver = Resolver::new(prysm).with_lookup(
        |_, _| {
            std::thread::sleep(Duration::from_secs(60));
            Err(std::io::Error::other("too late"))
        },
        Duration::from_millis(100),
    );
    let log = CallLog::new();
    let (mut peer, _feed, task) = start(
        &Opened::fake(&log, FakeSourceSettings::default()),
        move || {
            RemoteSender::connect(
                &url,
                resolver.resolve()?,
                &prysm_capture_remote::testkit::offer(),
            )
        },
    );
    let event = peer
        .wait_for("a waiting status", |e| waiting(e).is_some())
        .await;
    assert!(waiting(&event).unwrap().ends_with("no answer yet"));
    let started = std::time::Instant::now();
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

/// The requested hardware encoder is unavailable (any desktop): the worker
/// keeps running and reports it, the receiver's source fails with the TV's
/// reason, and nothing switches to software or raw.
#[tokio::test]
async fn an_unavailable_hardware_encoder_is_reported_on_both_ends() {
    // The hardware encoder's smallest size, so the backend itself is tried.
    let hardware = RemoteSettings {
        width: 176,
        height: 144,
        encoding: StreamEncoding::H264Hardware,
        ..SOFTWARE
    };
    let portal = Portal::start();
    let mut receiver = portal.source(hardware);
    let (mut peer, _feed, task) = start(&Opened::nv12(), connect(&portal));
    let event = peer
        .wait_for("an encoder failure", |e| encoder_failure(e).is_some())
        .await;
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
    // The desktop sees an invalid `capture.encoding` with the TV's reason.
    let Error::Invalid(invalid) = &error else {
        panic!("{error:?}");
    };
    assert_eq!(invalid.field, "capture.encoding");
    assert!(
        invalid.reason.starts_with(
            "the sender cannot stream 176x144 at 30 fps with menus as h264_hardware: "
        ) && invalid.reason.contains("hardware H.264"),
        "{}",
        invalid.reason
    );

    // Still the encoder failure while the receiver is away.
    peer.wait_for("an encoder failure", |e| encoder_failure(e).is_some())
        .await;
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
            let hardware = RemoteSettings {
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
                    RemoteSettings {
                        encoding: StreamEncoding::H264Software,
                        ..RAW
                    },
                    u32::MAX,
                ),
            })
        }
    });
    let event = peer
        .wait_for("an encoder failure", |e| encoder_failure(e).is_some())
        .await;
    assert!(encoder_failure(&event).unwrap().contains("no encoder here"));
    peer.wait_for("a capturing status sending software H.264", |e| {
        capturing(e) == Some(true) && encoding(e) == Some(StreamEncoding::H264Software)
    })
    .await;
    let refusals: Vec<String> = log
        .events()
        .into_iter()
        .filter(|event| event.starts_with("sink.refuse:"))
        .collect();
    assert_eq!(refusals.len(), 5, "{refusals:?}");
    assert!(refusals.iter().all(|r| r.contains("no encoder here")));
    // The capture closed while the request was refused and opened again
    // for the new one.
    assert_eq!(opened.shapes(), [settings(&RAW); 2]);
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

/// Prysm answered with another version, then became unreachable: the
/// version is still the news, and the capture stays closed.
#[test]
fn a_version_mismatch_outlasts_an_unreachable_receiver() {
    use prysm_capture_remote::WireError;
    use prysm_ports::FrameSink;
    let log = CallLog::new();
    let mut answers = vec![
        Err(Error::other("cannot connect to receiver: refused")),
        Err(Error::adapter(
            "cannot connect to receiver",
            WireError::Version {
                ours: 6,
                theirs: Some(9),
            },
        )),
        Ok(Fake::new(&log, RAW, 0)),
    ];
    let (uplink, flow) = Uplink::new(move || answers.pop().expect("an answer"));
    let mut uplink = uplink.retry_every(Duration::ZERO);
    uplink.tick().unwrap();
    assert_eq!(flow.wanted(), Some(RAW));
    // The receiver drops; Prysm comes back updated, then goes away.
    uplink.send(&crate::check::picture(16, 16, 1)).unwrap();
    uplink.tick().unwrap();
    assert_eq!(flow.wanted(), None);
    uplink.tick().unwrap();
    assert_eq!(flow.wanted(), None, "the capture reopened");
}

/// A capture that does nothing, and asks the worker to stop when it is
/// closed: the stop arrives while it is released.
struct StopsWhileClosing(StopHandle);

impl FrameSource for StopsWhileClosing {
    fn next_frame(&mut self, _timeout: Duration) -> Result<Pull> {
        Ok(Pull::Idle)
    }

    fn close(self: Box<Self>) -> Result<()> {
        self.0.request(Stop::Requested("SIGTERM"));
        Ok(())
    }
}

/// The receiver asks for another shape, and the stop comes while the old
/// capture is released: the new one is never opened, so the stop costs one
/// release, not two.
#[test]
fn a_stop_while_a_replaced_capture_closes_opens_nothing() {
    use prysm_ports::FrameSink;
    let log = CallLog::new();
    // Popped from the end: RAW first, then SOFTWARE.
    let mut answers = vec![
        Ok(Fake::new(&log, SOFTWARE, 0)),
        Ok(Fake::new(&log, RAW, 0)),
    ];
    let (uplink, flow) = Uplink::new(move || answers.pop().expect("an answer"));
    let mut uplink = uplink.retry_every(Duration::ZERO);
    let stop = StopHandle::default();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let mut source = RequestedCapture::new(flow.clone(), stop.clone(), {
        let (opened, stop) = (opened.clone(), stop.clone());
        move |shape| {
            opened.lock().unwrap().push(shape);
            Ok(Box::new(StopsWhileClosing(stop.clone())) as Box<dyn FrameSource>)
        }
    });
    uplink.tick().unwrap();
    assert!(source.next_frame(Duration::ZERO).unwrap().is_idle());
    assert_eq!(*opened.lock().unwrap(), [settings(&RAW)]);
    // The receiver drops, then asks for another shape.
    uplink.send(&crate::check::picture(16, 16, 1)).unwrap();
    uplink.tick().unwrap();
    assert_eq!(flow.wanted(), Some(SOFTWARE));
    assert!(source.next_frame(Duration::ZERO).unwrap().is_idle());
    assert!(stop.reason().is_some());
    assert!(source.next_frame(Duration::ZERO).unwrap().is_idle());
    assert_eq!(*opened.lock().unwrap(), [settings(&RAW)], "nothing opened");
    Box::new(source).close().unwrap();
}

#[test]
fn requests_map_to_capture_settings() {
    let shape = settings(&RemoteSettings::default());
    assert_eq!(
        (shape.width, shape.height, shape.fps, shape.include_ui),
        (320, 180, 30, true)
    );
    shape.validate().unwrap();
}

#[test]
fn the_offer_is_the_capture_within_each_encoder() {
    use prysm_ports::{Mode, Modes, Span};
    let span = |min, max, step| Span::new(min, max, step).unwrap();
    let offer = crate::capture::offer(&[prysm_capture_webos::MODE]);
    assert_eq!(
        offer.modes(StreamEncoding::H264Hardware),
        Modes::from(Mode::new(
            span(176, 1920, 2),
            span(144, 1080, 2),
            span(1, 30, 1)
        ))
    );
    assert_eq!(
        offer.modes(StreamEncoding::H264Software),
        Modes::from(Mode::new(
            span(16, 1920, 2),
            span(16, 1080, 2),
            span(1, 60, 1)
        ))
    );
    assert_eq!(
        offer.modes(StreamEncoding::Raw),
        Modes::from(prysm_capture_webos::MODE)
    );
    // A capture no encoder takes is offered raw only.
    let tiny = Mode::new(Span::exactly(8), Span::exactly(8), span(1, 60, 1));
    let offer = crate::capture::offer(&[tiny]);
    assert_eq!(offer.modes(StreamEncoding::Raw), Modes::from(tiny));
    assert_eq!(
        offer.modes(StreamEncoding::H264Software),
        Modes::Known(Vec::new())
    );
    let tiny_request = RemoteSettings {
        width: 8,
        height: 8,
        ..RemoteSettings::default()
    };
    assert_eq!(offer.check(&tiny_request).unwrap_err().field, "width");
}
