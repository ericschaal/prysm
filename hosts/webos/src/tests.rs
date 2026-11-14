use crate::link::EngineLink;
use prysm_capture_webos::{Readiness, VideoSignal};
use prysm_ports::{AdapterSettings, Error, LightingTarget, Result, SessionPhase, Settings};
use prysm_runtime::Prysm;
use prysm_runtime::fakes::{
    self, CallLog, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_webos_link::worker::{Link, Peer, Stop};
use prysm_webos_link::{Command, Event, Video, WorkerStatus};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn source(settings: &FakeSourceSettings) -> Settings {
    Settings {
        capture: Some(AdapterSettings::new("fake", settings)),
        ..fakes::settings()
    }
}

/// The host task: its result and the stop reason.
type Running = tokio::task::JoinHandle<(Result<()>, Option<Stop>)>;

const PORTAL: &str = "http://10.0.0.2:8787/";

/// Run a fake engine with the link surface until the link stops.
fn start(settings: Settings, initial: LightingTarget) -> (Peer, VideoSignal, CallLog, Running) {
    start_with_portal(
        settings,
        initial,
        &Arc::new(Mutex::new(Some(PORTAL.into()))),
    )
}

/// The same, with the portal URL read from `portal` at every heartbeat.
fn start_with_portal(
    settings: Settings,
    initial: LightingTarget,
    portal: &Arc<Mutex<Option<String>>>,
) -> (Peer, VideoSignal, CallLog, Running) {
    let (link, peer) = Link::memory();
    let feed = VideoSignal::default();
    let log = CallLog::new();
    let stop = link.stop.clone();
    let portal = portal.clone();
    let surface = EngineLink::new(
        link,
        feed.clone(),
        initial,
        Box::new(move || portal.lock().unwrap().clone()),
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
            fps: None,
            target: Some(LightingTarget::Off),
            portal: Some(PORTAL.into()),
            failure: None,
            encoding: None,
            paused: false,
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
    assert_eq!(
        feed.current(),
        Readiness {
            video: Video::Available,
            generation: 1
        },
        "set, then one reset"
    );
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
    let running = status(&running).unwrap().clone();
    assert!(running.fps.is_some_and(|fps| fps > 0), "{running:?}");
    // Unchanged status keeps arriving (the rate may have been measured anew).
    let same = |status: &WorkerStatus| WorkerStatus {
        fps: None,
        ..status.clone()
    };
    for _ in 0..3 {
        let event = peer.event().await.unwrap();
        assert_eq!(same(status(&event).unwrap()), same(&running));
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
            .message
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
            .message
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
    // The status just before the fault says lighting no longer runs.
    let mut previous = None;
    let fault = loop {
        let event = peer.event().await.unwrap();
        if matches!(event, Event::Faulted { .. }) {
            break event;
        }
        previous = Some(event);
    };
    let last = previous.as_ref().and_then(status).unwrap();
    assert!(!last.capturing && last.failure.is_some(), "{last:?}");
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

/// A new LAN address (a DHCP renewal) is advertised at the next heartbeat.
#[tokio::test]
async fn the_portal_url_follows_the_tvs_address() {
    let portal = Arc::new(Mutex::new(Some(PORTAL.to_owned())));
    let (mut peer, _feed, _log, task) =
        start_with_portal(fakes::settings(), LightingTarget::Off, &portal);
    let first = peer.event().await.unwrap();
    assert_eq!(status(&first).unwrap().portal.as_deref(), Some(PORTAL));
    *portal.lock().unwrap() = Some("http://10.0.0.9:8787/".into());
    peer.wait_for(|e| {
        status(e).is_some_and(|s| s.portal.as_deref() == Some("http://10.0.0.9:8787/"))
    })
    .await
    .unwrap();
    // No LAN address at all: none is advertised.
    *portal.lock().unwrap() = None;
    peer.wait_for(|e| status(e).is_some_and(|s| s.portal.is_none()))
        .await
        .unwrap();
    peer.close();
    task.await.unwrap().0.unwrap();
}

#[test]
fn the_portal_url_is_a_concrete_lan_address() {
    assert_eq!(
        prysm_web_portal::lan_url("192.168.1.5:8787".parse().unwrap()).as_deref(),
        Some("http://192.168.1.5:8787/")
    );
    // A wildcard resolves to the default route's interface address: never
    // 0.0.0.0, and never loopback, which a phone could not open.
    if let Some(url) = prysm_web_portal::lan_url("0.0.0.0:8787".parse().unwrap()) {
        assert!(!url.contains("0.0.0.0") && !url.contains("127."), "{url}");
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

#[tokio::test]
async fn a_session_waiting_for_video_is_not_capturing() {
    let settings = source(&FakeSourceSettings {
        quiet_after: Some(0),
        ..FakeSourceSettings::default()
    });
    let (mut peer, _feed, log, task) = start(settings, LightingTarget::Processed);
    peer.send(Command::Video {
        video: Video::Unavailable,
    });
    // Lighting is on but no frame arrives: not capturing, output untouched.
    let waiting = peer
        .wait_for(|e| {
            status(e).is_some_and(|s| {
                s.target == Some(LightingTarget::Processed) && !s.capturing && s.fps == Some(0)
            })
        })
        .await
        .unwrap();
    assert!(status(&waiting).unwrap().failure.is_none());
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert_eq!(log.count("output.open:"), 0, "{:?}", log.events());

    peer.send(Command::Lighting {
        target: LightingTarget::Off,
    });
    peer.wait_for(|e| status(e).is_some_and(|s| s.target == Some(LightingTarget::Off)))
        .await
        .unwrap();
    peer.close();
    task.await.unwrap().0.unwrap();
}
