use crate::link::EngineLink;
use prysm_capture_webos::{Readiness, VideoSignal};
use prysm_ports::{AdapterSettings, Error, LightingTarget, Result, SessionPhase, Settings};
use prysm_runtime::Prysm;
use prysm_runtime::fakes::{
    self, CallLog, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_webos_link::worker::{Link, Peer, Stop};
use prysm_webos_link::{Command, Escalation, Event, FullStatus, Video, WorkerStatus};
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
    start_timed(settings, initial, portal, Duration::from_millis(50))
}

/// The same, with this heartbeat.
fn start_timed(
    settings: Settings,
    initial: LightingTarget,
    portal: &Arc<Mutex<Option<String>>>,
    heartbeat: Duration,
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
    .with_heartbeat(heartbeat);
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

/// A status event's report; the full worker sends only full ones.
fn status(event: &Event) -> Option<&FullStatus> {
    match event {
        Event::Status(WorkerStatus::Full(status)) => Some(status),
        Event::Status(WorkerStatus::Slim(status)) => panic!("a slim status: {status:?}"),
        Event::Failed(_) => None,
    }
}

#[tokio::test]
async fn commands_drive_video_and_lighting_and_status_follows() {
    let (mut peer, feed, log, task) = start(fakes::settings(), LightingTarget::Off);
    let first = peer.event().await.unwrap();
    assert_eq!(
        status(&first).unwrap(),
        &FullStatus {
            capturing: false,
            fps: None,
            target: LightingTarget::Off,
            lights: None,
            applied: 0,
            portal: Some(PORTAL.into()),
            failure: None,
        }
    );

    peer.send(Command::Video {
        video: Video::Available,
    });
    peer.send(Command::ResetVideo {});
    peer.send(Command::Target {
        target: LightingTarget::ScreenSync,
        sequence: 1,
    });
    let running = peer
        .wait_for("a capturing status", |e| {
            status(e).is_some_and(|s| s.capturing)
        })
        .await;
    assert_eq!(status(&running).unwrap().target, LightingTarget::ScreenSync);
    assert_eq!(
        status(&running).unwrap().applied,
        1,
        "it says what it applied"
    );
    assert_eq!(
        feed.current(),
        Readiness {
            video: Video::Available,
            generation: 1,
            sequence: 2
        },
        "set, then one reset"
    );
    assert!(log.wait_for("output.write:", 1).await);

    peer.send(Command::Target {
        target: LightingTarget::Off,
        sequence: 2,
    });
    peer.wait_for("a status that is not capturing", |e| {
        status(e).is_some_and(|s| !s.capturing)
    })
    .await;
    assert!(log.wait_for("output.idle::Off", 1).await);

    peer.close();
    let (result, stop) = task.await.unwrap();
    result.unwrap();
    assert_eq!(stop, Some(Stop::Requested("stdin closed")));
}

#[tokio::test]
async fn initial_target_applies_and_status_repeats_as_heartbeat() {
    let (mut peer, _feed, log, task) = start(fakes::settings(), LightingTarget::ScreenSync);
    let running = peer
        .wait_for("a capturing status", |e| {
            status(e).is_some_and(|s| s.capturing)
        })
        .await;
    let running = status(&running).unwrap().clone();
    assert!(running.fps.is_some_and(|fps| fps > 0), "{running:?}");
    // Unchanged status keeps arriving (the rate may have been measured anew).
    let same = |status: &FullStatus| FullStatus {
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

/// Advance tokio's clock by `by` while no adapter call is in flight (the
/// fakes run on real threads, so time stays paused only for the jump), and
/// return once the link has sent the heartbeat that follows: a status with
/// the portal URL `marker`, which only a heartbeat after the jump reads.
/// Anything the jump made due ran before it.
async fn advance(peer: &mut Peer, portal: &Arc<Mutex<Option<String>>>, by: Duration, marker: &str) {
    *portal.lock().unwrap() = Some(marker.to_owned());
    tokio::time::pause();
    tokio::time::advance(by).await;
    tokio::time::resume();
    peer.wait_for("the heartbeat after the jump", |e| {
        status(e).is_some_and(|s| s.portal.as_deref() == Some(marker))
    })
    .await;
}

/// A failed session is reported and stays failed: the worker tries nothing
/// again by itself, however long it waits, until a target command comes.
/// The heartbeat is 10 s, so nothing is due until the test moves the clock.
#[tokio::test]
async fn a_failed_session_waits_for_the_next_command() {
    const BEAT: Duration = Duration::from_secs(10);
    let settings = source(&FakeSourceSettings {
        fail_open: true,
        ..FakeSourceSettings::default()
    });
    let portal = Arc::new(Mutex::new(Some(PORTAL.to_owned())));
    let opens = |log: &CallLog| log.count("source.open_failed:");
    let (mut peer, _feed, log, task) =
        start_timed(settings, LightingTarget::ScreenSync, &portal, BEAT);
    let failed = peer
        .wait_for("a status with a failure", |e| {
            status(e).is_some_and(|s| s.failure.is_some())
        })
        .await;
    let failure = status(&failed).unwrap().failure.clone().unwrap();
    assert!(failure.message.contains("fake source"), "{failure:?}");
    assert_eq!(failure.escalation, Escalation::FAIL_FAST);
    assert_eq!(opens(&log), 1);

    advance(&mut peer, &portal, BEAT * 60, "http://10.0.0.3:8787/").await;
    assert_eq!(opens(&log), 1, "tried again by itself");

    peer.send(Command::Target {
        target: LightingTarget::ScreenSync,
        sequence: 1,
    });
    peer.wait_for("a failure after the command", |e| {
        status(e).is_some_and(|s| s.applied == 1 && s.failure.is_some())
    })
    .await;
    assert_eq!(opens(&log), 2, "the command opens the source again");
    peer.close();
    task.await.unwrap().0.unwrap();
}

/// Missing settings fail at once, asking for other settings.
#[tokio::test]
async fn missing_settings_are_reported_without_opening_the_source() {
    let settings = Settings {
        output: None,
        ..fakes::settings()
    };
    let (mut peer, _feed, log, task) = start(settings, LightingTarget::ScreenSync);
    let failed = peer
        .wait_for("a status with a failure", |e| {
            status(e).is_some_and(|s| s.failure.is_some())
        })
        .await;
    assert!(
        status(&failed)
            .unwrap()
            .failure
            .as_ref()
            .unwrap()
            .message
            .contains("output")
    );
    assert_eq!(log.count("source.open:"), 0);
    peer.close();
    task.await.unwrap().0.unwrap();
}

/// The status reports a fault; whoever started the worker stops it (here
/// the link's close), and the stop ends in the fault.
async fn the_fault_is_reported_until_the_stop(mut peer: Peer, task: Running) {
    let faulted = peer
        .wait_for("a status with a fault", |e| {
            status(e)
                .and_then(|s| s.failure.as_ref())
                .is_some_and(|failure| failure.escalation == Escalation::Restart)
        })
        .await;
    let status = status(&faulted).unwrap();
    assert!(!status.capturing, "{status:?}");
    let fault = status.failure.clone().unwrap();
    assert!(
        fault.message.contains("confirming the source's release"),
        "{fault:?}"
    );
    peer.close();
    let (result, stop) = task.await.unwrap();
    assert!(matches!(
        result,
        Err(Error::Faulted(SessionPhase::SourceUnreleased))
    ));
    assert_eq!(stop, Some(Stop::Requested("stdin closed")));
    assert_eq!(
        prysm_webos_link::worker::exit_code(stop.as_ref(), &result),
        prysm_webos_link::exit::FAULTED
    );
}

#[tokio::test]
async fn an_unconfirmed_release_is_reported_as_a_fault() {
    let settings = source(&FakeSourceSettings {
        fail_after: Some(2),
        fail_close: true,
        ..FakeSourceSettings::default()
    });
    let (peer, _feed, _log, task) = start(settings, LightingTarget::ScreenSync);
    the_fault_is_reported_until_the_stop(peer, task).await;
}

/// Lighting turned off by the app over a source that cannot confirm its
/// release is reported the same way: the commanded path, not only a
/// session's own end.
#[tokio::test]
async fn an_unconfirmed_release_on_a_commanded_stop_is_reported_as_a_fault() {
    let settings = source(&FakeSourceSettings {
        fail_close: true,
        ..FakeSourceSettings::default()
    });
    let (peer, _feed, log, task) = start(settings, LightingTarget::ScreenSync);
    assert!(log.wait_for("output.write:", 1).await);
    peer.send(Command::Target {
        target: LightingTarget::Off,
        sequence: 1,
    });
    the_fault_is_reported_until_the_stop(peer, task).await;
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
    peer.wait_for("a status with the new portal URL", |e| {
        status(e).is_some_and(|s| s.portal.as_deref() == Some("http://10.0.0.9:8787/"))
    })
    .await;
    // No LAN address at all: none is advertised.
    *portal.lock().unwrap() = None;
    peer.wait_for("a status without a portal URL", |e| {
        status(e).is_some_and(|s| s.portal.is_none())
    })
    .await;
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
    std::fs::write(&path, r#"{"format":1}"#).unwrap();
    crate::seed(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"format":1}"#);
}

#[tokio::test]
async fn a_session_waiting_for_video_is_not_capturing() {
    let settings = source(&FakeSourceSettings {
        quiet_after: Some(0),
        ..FakeSourceSettings::default()
    });
    let (mut peer, _feed, log, task) = start(settings, LightingTarget::ScreenSync);
    peer.send(Command::Video {
        video: Video::Unavailable,
    });
    // Lighting is on but no frame arrives: not capturing, output untouched.
    let waiting = peer
        .wait_for("a status waiting for video", |e| {
            status(e).is_some_and(|s| {
                s.target == LightingTarget::ScreenSync && !s.capturing && s.fps == Some(0)
            })
        })
        .await;
    assert!(status(&waiting).unwrap().failure.is_none());
    // `fps` is first reported after a whole sampling window (about a
    // second) without a frame: the window this proves nothing opened in.
    assert_eq!(log.count("output.open:"), 0, "{:?}", log.events());

    peer.send(Command::Target {
        target: LightingTarget::Off,
        sequence: 1,
    });
    peer.wait_for("a status with lighting off", |e| {
        status(e).is_some_and(|s| s.target == LightingTarget::Off)
    })
    .await;
    peer.close();
    task.await.unwrap().0.unwrap();
}

/// Every adapter section of the `host-webos` contract decodes with the
/// adapters the worker registers: a kind it lacks fails.
#[test]
fn contract_adapter_fixtures_decode_with_the_registered_adapters() {
    for fixture in prysm_testkit::host_adapter_settings("host-webos") {
        let settings: Settings = serde_json::from_value(fixture.clone()).unwrap();
        crate::adapters(
            Prysm::builder(MemoryStore::with(settings)),
            &VideoSignal::default(),
        )
        .check()
        .unwrap_or_else(|error| panic!("{fixture}: {error}"));
    }
}
