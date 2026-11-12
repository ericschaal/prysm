use crate::link::{Counted, ForwarderLink};
use crate::video::VideoFeed;
use prysm_ports::Result;
use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{CallLog, FakeSink, FakeSource, FakeSourceSettings};
use prysm_webos_link::worker::{Link, Peer, Stop};
use prysm_webos_link::{Command, Event, LightingTarget, SlimCapture, Video, WorkerStatus};
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
    let (link, peer) = Link::memory();
    let feed = Feed::default();
    let log = CallLog::new();
    let stop = link.stop.clone();
    let (sink, sent) = Counted::new(FakeSink(log.clone()));
    let surface = ForwarderLink::new(link, feed.clone(), sent)
        .with_timing(Duration::from_millis(30), Duration::from_millis(100));
    let source = FakeSource::new(&log, settings);
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
