use super::{Core, Event, Lease, Notice};
use crate::engine::control::{Lighting, Ports};
use crate::fakes::{self, CallLog, FakeOutputs, FakeSources, MemoryStore};
use crate::intent::LightingIntent;
use prysm_ports::{LightingTarget, OpenIntent, Registry, SessionState, Snapshot};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{oneshot, watch};

const LIMIT: Duration = Duration::from_secs(5);

fn engine(log: &CallLog) -> Core {
    let settings = fakes::settings();
    let mut registry = Registry::default();
    registry.add_source(FakeSources(log.clone()));
    registry.add_output(FakeOutputs(log.clone()));
    let ports = Ports::new(&registry, &settings, None);
    Core {
        lighting: Lighting::new(LightingIntent::new(None, true), ports),
        registry,
        store: Arc::new(Mutex::new(Box::new(MemoryStore::with(settings.clone())))),
        state: watch::channel(Snapshot {
            settings,
            ..Snapshot::default()
        })
        .0,
        unreadable: None,
        persisted_mode: None,
        calibration_expires: None,
        trial: None,
        feeds: Default::default(),
        stats: Default::default(),
        stop_deadline: LIMIT,
        abandon_at: watch::channel(None).1,
        abandoned: None,
    }
}

/// Intercept the engine's actual reply so the test controls its delivery to the worker.
async fn authorized_open(core: &mut Core) -> (OpenIntent, oneshot::Sender<OpenIntent>) {
    core.check_output();
    core.set_target(LightingTarget::ScreenSync);
    loop {
        match core.lighting.next().await {
            Event::Session(Notice::CaptureOpened { authorize: worker }) => {
                let (authorize, reply) = oneshot::channel();
                core.lighting_event(Event::Session(Notice::CaptureOpened { authorize }))
                    .await;
                let intent = reply.await.expect("the engine authorized this output");
                assert!(matches!(
                    core.lighting.active.as_ref().unwrap().claims.output,
                    Lease::Claimed(_)
                ));
                return (intent, worker);
            }
            event => core.lighting_event(event).await,
        }
    }
}

#[tokio::test]
async fn power_pressed_after_authorization_before_open_stays_busy_until_output_opens() {
    prysm_testkit::within_async(LIMIT, async {
        let log = CallLog::new();
        let mut core = engine(&log);
        let (intent, deliver) = authorized_open(&mut core).await;
        assert!(core.state.borrow().control.busy);
        core.press_power();
        assert_eq!(core.state.borrow().target, LightingTarget::ScreenSync);
        assert_eq!(log.count("output.open:"), 0);
        assert_eq!(log.count("output.idle::Off"), 0);

        deliver.send(intent).unwrap();
        while !matches!(core.state.borrow().session, SessionState::ScreenSync) {
            let event = core.lighting.next().await;
            core.lighting_event(event).await;
        }
        assert!(!core.state.borrow().control.busy);
        assert_eq!(log.count("output.open:"), 1);
        core.press_power();
        while core.lighting.active.is_some() || core.lighting.command.is_some() {
            let event = core.lighting.next().await;
            core.lighting_event(event).await;
        }
        assert_eq!(core.state.borrow().target, LightingTarget::Off);
        assert_eq!(log.count("source.close:"), 1);
        assert_eq!(log.count("output.idle::Off"), 1);
        assert!(log.position("source.close:") < log.position("output.idle::Off"));
    })
    .await;
}

#[tokio::test]
async fn pausing_after_authorization_before_open_releases_capture_without_opening_output() {
    prysm_testkit::within_async(LIMIT, async {
        let log = CallLog::new();
        let mut core = engine(&log);
        let (intent, deliver) = authorized_open(&mut core).await;
        core.pause_lighting();
        let _ = deliver.send(intent);
        while core.lighting.active.is_some() {
            let event = core.lighting.next().await;
            core.lighting_event(event).await;
        }
        assert_eq!(core.state.borrow().target, LightingTarget::Off);
        assert!(core.lighting.fault().is_none());
        assert_eq!(log.count("source.close:"), 1);
        assert_eq!(log.count("output.open:"), 0);
        assert_eq!(log.count("output.idle::Off"), 0);
    })
    .await;
}
