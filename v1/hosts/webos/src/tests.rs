use crate::link::EngineLink;
use crate::video::VideoFeed;
use prysm_ports::{AdapterSettings, Error, LightingTarget, Result, SessionPhase, Settings};
use prysm_runtime::Prysm;
use prysm_runtime::fakes::{
    self, CallLog, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_webos_link::worker::{Link, Peer, Stop};
use prysm_webos_link::{Command, Event, Video, WorkerStatus};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

#[derive(Clone, Default)]
struct Feed(Arc<Mutex<Vec<String>>>);

impl Feed {
    fn seen(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

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

fn source(settings: &FakeSourceSettings) -> Settings {
    Settings {
        capture: Some(AdapterSettings::new("fake", settings)),
        ..fakes::settings()
    }
}

/// The host task: its result and the stop reason.
type Running = tokio::task::JoinHandle<(Result<()>, Option<Stop>)>;

/// Run a fake engine with the link surface until the link stops.
fn start(settings: Settings, initial: LightingTarget) -> (Peer, Feed, CallLog, Running) {
    let (link, peer) = Link::memory();
    let feed = Feed::default();
    let log = CallLog::new();
    let stop = link.stop.clone();
    let surface = EngineLink::new(
        link,
        feed.clone(),
        initial,
        Some("http://10.0.0.2:8787/".into()),
    )
    .with_timing(Duration::from_millis(50), Duration::from_millis(20));
    let (sources, outputs) = (FakeSources(log.clone()), FakeOutputs(log.clone()));
    let task = tokio::spawn(async move {
        let result = Prysm::builder(MemoryStore::with(settings))
            .source(sources)
            .output(outputs)
            .surface(surface)
            .stop_deadline(Duration::from_millis(500))
            .run(stop.stopped())
            .await;
        (result, stop.reason())
    });
    (peer, feed, log, task)
}

fn status(event: &Event) -> Option<&WorkerStatus> {
    match event {
        Event::Status(status) => Some(status),
        Event::Faulted { .. } => None,
    }
}

#[tokio::test]
async fn commands_drive_video_and_lighting_and_status_follows() {
    let (mut peer, feed, log, task) = start(fakes::settings(), LightingTarget::Off);
    let first = peer.event().await.unwrap();
    assert_eq!(
        status(&first).unwrap(),
        &WorkerStatus {
            capturing: false,
            target: Some(LightingTarget::Off),
            portal: Some("http://10.0.0.2:8787/".into()),
            failure: None,
            encoding: None,
        }
    );

    peer.send(Command::Video {
        video: Video::Available,
    });
    peer.send(Command::ResetVideo {});
    peer.send(Command::Lighting {
        target: LightingTarget::Processed,
    });
    let running = peer
        .wait_for(|e| status(e).is_some_and(|s| s.capturing))
        .await
        .unwrap();
    assert_eq!(
        status(&running).unwrap().target,
        Some(LightingTarget::Processed)
    );
    assert_eq!(feed.seen(), ["Available", "reset"]);
    assert!(log.wait_for("output.write:", 1).await);

    peer.send(Command::Lighting {
        target: LightingTarget::Off,
    });
    peer.wait_for(|e| status(e).is_some_and(|s| !s.capturing))
        .await
        .unwrap();
    assert!(log.wait_for("output.idle::Off", 1).await);

    peer.close();
    let (result, stop) = task.await.unwrap();
    result.unwrap();
    assert_eq!(stop, Some(Stop::Requested("stdin closed")));
}

#[tokio::test]
async fn initial_target_applies_and_status_repeats_as_heartbeat() {
    let (mut peer, _feed, log, task) = start(fakes::settings(), LightingTarget::Processed);
    let running = peer
        .wait_for(|e| status(e).is_some_and(|s| s.capturing))
        .await
        .unwrap();
    // Unchanged status keeps arriving.
    for _ in 0..3 {
        assert_eq!(peer.event().await.unwrap(), running);
    }
    assert_eq!(log.count("source.open:"), 1);
    peer.close();
    task.await.unwrap().0.unwrap();
}

#[tokio::test]
async fn a_failed_session_is_retried_until_lighting_is_turned_off() {
    let settings = source(&FakeSourceSettings {
        fail_after: Some(3),
        ..FakeSourceSettings::default()
    });
    let (mut peer, _feed, log, task) = start(settings, LightingTarget::Processed);
    assert!(log.wait_for("source.open:", 3).await, "{:?}", log.events());
    let failed = peer
        .wait_for(|e| status(e).is_some_and(|s| s.failure.is_some()))
        .await
        .unwrap();
    assert!(
        status(&failed)
            .unwrap()
            .failure
            .as_ref()
            .unwrap()
            .contains("fake source failed")
    );

    peer.send(Command::Lighting {
        target: LightingTarget::Off,
    });
    peer.wait_for(|e| status(e).is_some_and(|s| s.target == Some(LightingTarget::Off)))
        .await
        .unwrap();
    let opened = log.count("source.open:");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(log.count("source.open:"), opened, "Off cancels retries");
    peer.close();
    task.await.unwrap().0.unwrap();
}

#[tokio::test]
async fn missing_settings_are_not_retried() {
    let settings = Settings {
        output: None,
        ..fakes::settings()
    };
    let (mut peer, _feed, log, task) = start(settings, LightingTarget::Processed);
    let failed = peer
        .wait_for(|e| status(e).is_some_and(|s| s.failure.is_some()))
        .await
        .unwrap();
    assert!(
        status(&failed)
            .unwrap()
            .failure
            .as_ref()
            .unwrap()
            .contains("output")
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(log.count("source.open:"), 0);
    peer.close();
    task.await.unwrap().0.unwrap();
}

#[tokio::test]
async fn an_unconfirmed_release_faults_the_worker() {
    let settings = source(&FakeSourceSettings {
        fail_after: Some(2),
        fail_close: true,
        ..FakeSourceSettings::default()
    });
    let (mut peer, _feed, _log, task) = start(settings, LightingTarget::Processed);
    let fault = peer
        .wait_for(|e| matches!(e, Event::Faulted { .. }))
        .await
        .unwrap();
    let Event::Faulted { cause } = fault else {
        unreachable!()
    };
    assert!(cause.contains("closing the source"), "{cause}");
    let (result, stop) = task.await.unwrap();
    assert!(matches!(
        result,
        Err(Error::Faulted(SessionPhase::ClosingSource))
    ));
    assert!(matches!(stop, Some(Stop::Faulted(_))));
    assert_eq!(
        prysm_webos_link::worker::exit_code(stop.as_ref(), &result),
        prysm_webos_link::exit::FAULTED
    );
}

#[test]
fn portal_url_uses_a_concrete_address() {
    assert_eq!(
        crate::portal_url("127.0.0.1:8787".parse().unwrap()).as_deref(),
        Some("http://127.0.0.1:8787/")
    );
    // A wildcard resolves to some interface address, never to 0.0.0.0.
    if let Some(url) = crate::portal_url("0.0.0.0:8787".parse().unwrap()) {
        assert!(!url.contains("0.0.0.0"), "{url}");
    }
}

#[test]
fn seed_writes_webos_capture_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    crate::seed(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(r#""kind": "webos""#), "{text}");
    std::fs::write(&path, "{}").unwrap();
    crate::seed(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
}
