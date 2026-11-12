use crate::link::{ForwarderLink, Uplink, Upstream};
use crate::video::VideoFeed;
use prysm_capture_remote::{RemoteSender, RemoteSource, Stream};
use prysm_core::Frame;
use prysm_h264::{Backend, EncoderSettings};
use prysm_ports::{Error, FrameSink, FrameSource, Pull, Result};
use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{CallLog, FakeSink, FakeSource, FakeSourceSettings};
use prysm_webos_link::worker::{Link, Peer, Stop};
use prysm_webos_link::{Command, Event, LightingTarget, SlimCapture, Video, WorkerStatus};
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

fn start(settings: FakeSourceSettings) -> (Peer, Feed, Running) {
    let log = CallLog::new();
    let sink = log.clone();
    start_with(settings, &log, move || Ok(FakeSink(sink.clone())))
}

impl Upstream for FakeSink {
    fn paused(&self) -> bool {
        false
    }
}

/// The worker with `connect` opening its uplink, retried every 20 ms.
fn start_with<S: Upstream + 'static>(
    settings: FakeSourceSettings,
    log: &CallLog,
    connect: impl FnMut() -> Result<S> + Send + 'static,
) -> (Peer, Feed, Running) {
    start_source(FakeSource::new(log, settings), SlimEncoding::Raw, connect)
}

/// The worker reading `source` and sending with `encoding`.
fn start_source<S: Upstream + 'static>(
    source: impl FrameSource + 'static,
    encoding: SlimEncoding,
    connect: impl FnMut() -> Result<S> + Send + 'static,
) -> (Peer, Feed, Running) {
    let (link, peer) = Link::memory();
    let feed = Feed::default();
    let stop = link.stop.clone();
    let (sink, flow) = Uplink::new(encoding, connect);
    let sink = sink.retry_every(Duration::from_millis(20));
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

fn capturing(event: &Event) -> Option<bool> {
    match event {
        Event::Status(status) => Some(status.capturing),
        Event::Faulted { .. } => None,
    }
}

fn waiting(event: &Event) -> Option<&str> {
    match event {
        Event::Status(status) if !status.capturing => status.failure.as_deref(),
        _ => None,
    }
}

/// Records `"sink.send"` and fails after `left` frames, like a receiver whose
/// host stopped lighting.
struct Dropping {
    log: CallLog,
    left: u32,
}

impl Upstream for Dropping {
    fn paused(&self) -> bool {
        false
    }
}

impl FrameSink for Dropping {
    fn send(&mut self, _frame: &Frame) -> Result<()> {
        if self.left == 0 {
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
    let (mut peer, _feed, task) = start_with(FakeSourceSettings::default(), &log, {
        let (log, attempts) = (log.clone(), attempts.clone());
        move || {
            // Refused three times, then a receiver that drops after 5 frames,
            // then one that stays.
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            match attempt {
                0..=2 => Err(Error::other("cannot connect to receiver: refused")),
                3 => Ok(Dropping {
                    log: log.clone(),
                    left: 5,
                }),
                _ => Ok(Dropping {
                    log: log.clone(),
                    left: u32::MAX,
                }),
            }
        }
    });
    let event = peer.wait_for(|e| waiting(e).is_some()).await.unwrap();
    assert_eq!(
        waiting(&event),
        Some("waiting for the receiver: cannot connect to receiver: refused")
    );
    let Event::Status(status) = &event else {
        unreachable!()
    };
    assert_eq!(status.failure_kind, Some(FailureKind::Receiver));
    // Connected: frames flow and no wait is reported.
    peer.wait_for(|e| matches!(e, Event::Status(s) if s.capturing && s.failure.is_none()))
        .await
        .unwrap();
    // The first receiver drops after 5 frames; the uplink reconnects.
    assert!(log.wait_for("sink.send", 20).await);
    assert!(attempts.load(Ordering::SeqCst) >= 5);
    assert!(!task.is_finished(), "the worker keeps running throughout");

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
    let (mut peer, _feed, task) = start_with(FakeSourceSettings::default(), &log, move || {
        RemoteSender::connect(address, Stream::Raw)
    });
    peer.wait_for(|e| waiting(e).is_some()).await.unwrap();

    let mut receiver = RemoteSource::bind(address).unwrap();
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
    peer.wait_for(|e| capturing(e) == Some(true)).await.unwrap();
    Box::new(receiver).close().unwrap();

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
                let _ = stream.write_all(b"PRSM\x03\x01\x00\x00");
            }
        }
    });
    let log = CallLog::new();
    let (mut peer, _feed, task) = start_with(FakeSourceSettings::default(), &log, move || {
        RemoteSender::connect(address, Stream::Raw)
    });
    let event = peer
        .wait_for(|e| waiting(e).is_some_and(|reason| reason.contains("version 3")))
        .await
        .unwrap();
    let reason = waiting(&event).unwrap();
    assert!(
        reason.contains("refused the stream") && reason.contains("this end version 2"),
        "{reason}"
    );
    assert!(!task.is_finished(), "the worker keeps running");
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

#[tokio::test]
async fn reports_frame_flow_and_forwards_video_commands() {
    let (mut peer, feed, task) = start(FakeSourceSettings {
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
    let (peer, _feed, task) = start(FakeSourceSettings::default());
    peer.send(Command::Lighting {
        target: LightingTarget::Processed,
    });
    let (result, stop) = task.await.unwrap();
    result.unwrap();
    assert!(matches!(stop, Some(Stop::Failed(_))));
}

#[tokio::test]
async fn an_unconfirmed_release_fails_the_forwarder() {
    let (mut peer, _feed, task) = start(FakeSourceSettings {
        fail_close: true,
        ..FakeSourceSettings::default()
    });
    peer.close();
    let (result, _) = task.await.unwrap();
    assert!(result.unwrap_err().to_string().contains("unconfirmed"));
}

/// The TV capture's NV12 pictures, every 5 ms.
struct Nv12Source(u32);

impl FrameSource for Nv12Source {
    fn next_frame(&mut self, _timeout: Duration) -> Result<Pull> {
        std::thread::sleep(Duration::from_millis(5));
        self.0 += 1;
        Ok(Pull::Frame(crate::check::picture(64, 32, self.0)))
    }

    fn close(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

const SOFTWARE: Stream = Stream::H264(EncoderSettings {
    backend: Backend::Software,
    fps: 30,
    bitrate: prysm_h264::DEFAULT_BITRATE,
});

fn encoding(event: &Event) -> Option<SlimEncoding> {
    match event {
        Event::Status(status) => status.encoding,
        Event::Faulted { .. } => None,
    }
}

/// Software H.264 end to end: frames decode at a real receiver, and the
/// status names the encoder while connected.
#[tokio::test]
async fn software_h264_reaches_a_receiver_and_is_reported() {
    let mut receiver = RemoteSource::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = receiver.local_addr();
    let (mut peer, _feed, task) =
        start_source(Nv12Source(0), SlimEncoding::H264Software, move || {
            RemoteSender::connect(address, SOFTWARE)
        });
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

/// The configured encoder failing ends the worker with its cause; nothing
/// switches to software or raw.
#[tokio::test]
async fn an_unavailable_hardware_encoder_fails_the_worker() {
    let receiver = RemoteSource::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = receiver.local_addr();
    let hardware = Stream::H264(EncoderSettings::new(Backend::Hardware, 30));
    let (_peer, _feed, task) = start_source(Nv12Source(0), SlimEncoding::H264Hardware, move || {
        RemoteSender::connect(address, hardware)
    });
    let (result, _) = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the worker stops")
        .unwrap();
    let error = result.unwrap_err();
    let failure = prysm_webos_link::chain(&error);
    assert!(failure.contains("hardware H.264"), "{failure}");
    assert_eq!(crate::link::failure_kind(&error), FailureKind::Encoder);
    Box::new(receiver).close().unwrap();
}

/// H.264 needs the capture's NV12: anything else is an encoder failure.
#[tokio::test]
async fn h264_of_other_pixels_fails_the_worker() {
    let receiver = RemoteSource::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let address = receiver.local_addr();
    let log = CallLog::new();
    let (_peer, _feed, task) = start_source(
        FakeSource::new(&log, FakeSourceSettings::default()),
        SlimEncoding::H264Software,
        move || RemoteSender::connect(address, SOFTWARE),
    );
    let (result, _) = task.await.unwrap();
    let error = result.unwrap_err();
    let failure = prysm_webos_link::chain(&error);
    assert!(failure.contains("NV12"), "{failure}");
    assert_eq!(crate::link::failure_kind(&error), FailureKind::Encoder);
    assert_eq!(
        crate::link::failure_kind(&Error::other("capture failed")),
        FailureKind::Other
    );
    Box::new(receiver).close().unwrap();
}

/// A receiver that refuses H.264 is a wait naming the refusal; the worker
/// does not downgrade to raw.
#[tokio::test]
async fn a_receiver_refusing_h264_is_reported_as_the_wait() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let hellos = Arc::new(Mutex::new(Vec::new()));
    std::thread::spawn({
        let hellos = hellos.clone();
        move || {
            for mut stream in listener.incoming().flatten() {
                let mut hello = [0; 8];
                if stream.read_exact(&mut hello).is_ok() {
                    hellos.lock().unwrap().push(hello[5]);
                    let _ = stream.write_all(b"PRSM\x02\x02\x00\x00");
                }
            }
        }
    });
    let (mut peer, _feed, task) =
        start_source(Nv12Source(0), SlimEncoding::H264Software, move || {
            RemoteSender::connect(address, SOFTWARE)
        });
    let event = peer
        .wait_for(|e| waiting(e).is_some_and(|reason| reason.contains("encoding 1")))
        .await
        .unwrap();
    assert!(waiting(&event).unwrap().contains("refused the stream"));
    peer.wait_for(|e| waiting(e).is_some()).await.unwrap();
    assert!(!task.is_finished());
    assert!(
        hellos.lock().unwrap().iter().all(|&code| code == 1),
        "never raw"
    );
    peer.close();
    let (result, _) = task.await.unwrap();
    result.unwrap();
}

#[test]
fn encodings_map_to_streams() {
    let capture = |encoding| SlimCapture {
        encoding,
        ..SlimCapture::default()
    };
    assert_eq!(
        crate::stream(&capture(SlimEncoding::H264Hardware)),
        Stream::H264(EncoderSettings::new(Backend::Hardware, 30))
    );
    assert_eq!(
        crate::stream(&capture(SlimEncoding::H264Software)),
        Stream::H264(EncoderSettings::new(Backend::Software, 30))
    );
    assert_eq!(crate::stream(&capture(SlimEncoding::Raw)), Stream::Raw);
}

#[test]
fn capture_maps_to_webos_settings() {
    let settings = crate::settings(SlimCapture::default());
    assert_eq!(
        (
            settings.width,
            settings.height,
            settings.fps,
            settings.include_ui
        ),
        (320, 180, 30, true)
    );
    settings.validate().unwrap();
}
