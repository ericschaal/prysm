use crate::link::{ForwarderLink, Uplink};
use crate::video::VideoFeed;
use prysm_capture_remote::{RemoteSender, RemoteSource};
use prysm_core::Frame;
use prysm_ports::{Error, FrameSink, FrameSource, Pull, Result};
use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{CallLog, FakeSink, FakeSource, FakeSourceSettings};
use prysm_webos_link::worker::{Link, Peer, Stop};
use prysm_webos_link::{Command, Event, LightingTarget, SlimCapture, Video, WorkerStatus};
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

/// The worker with `connect` opening its uplink, retried every 20 ms.
fn start_with<S: FrameSink + 'static>(
    settings: FakeSourceSettings,
    log: &CallLog,
    connect: impl FnMut() -> Result<S> + Send + 'static,
) -> (Peer, Feed, Running) {
    let (link, peer) = Link::memory();
    let feed = Feed::default();
    let stop = link.stop.clone();
    let (sink, flow) = Uplink::new(connect);
    let sink = sink.retry_every(Duration::from_millis(20));
    let surface = ForwarderLink::new(link, feed.clone(), flow)
        .with_timing(Duration::from_millis(30), Duration::from_millis(100));
    let source = FakeSource::new(log, settings);
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
        RemoteSender::connect(address)
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
