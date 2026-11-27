//! A faulted production engine refuses mutations until shutdown.
use prysm_ports::{Error, LightingTarget, Processing, Registry, SessionState};
use prysm_runtime::{
    Engine,
    fakes::{
        CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
    },
};
use std::time::Duration;

#[tokio::test]
async fn a_faulted_engine_refuses_every_command() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let log = CallLog::new();
        let mut registry = Registry::default();
        registry.add_source(FakeSources(log.clone()));
        registry.add_output(FakeOutputs(log.clone()));
        let mut settings = prysm_runtime::fakes::settings();
        settings.capture = Some(prysm_ports::AdapterSettings::new(
            "fake",
            &FakeSourceSettings {
                fail_close: true,
                ..Default::default()
            },
        ));
        settings.output = Some(prysm_ports::AdapterSettings::new(
            "fake",
            &FakeOutputSettings::default(),
        ));
        let (handle, engine) = Engine::start(
            registry,
            Box::new(MemoryStore::with(settings.clone())),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        handle
            .snapshot()
            .wait_for(|s| matches!(s.session, SessionState::ScreenSync))
            .await
            .unwrap();
        handle.set_target(LightingTarget::Off).await.unwrap();
        handle
            .snapshot()
            .wait_for(|s| matches!(s.session, SessionState::Faulted(_)))
            .await
            .unwrap();

        let calls = log.events();
        let revision = handle.snapshot().borrow().revision;
        let refused = |result: prysm_ports::Result<()>| {
            assert!(matches!(result, Err(Error::Faulted(_))), "{result:?}");
        };
        refused(handle.set_calibration(None).await);
        refused(handle.try_processing(Processing::default()).await);
        refused(handle.set_target(LightingTarget::Off).await);
        refused(handle.press_power().await);
        refused(handle.resume_screen_sync().await);
        refused(handle.pause().await);
        refused(handle.recheck().await);
        refused(handle.replace_settings(revision, settings).await.map(drop));
        assert_eq!(log.events(), calls);
        assert_eq!(handle.snapshot().borrow().revision, revision);
        assert!(engine.shutdown().await.is_err());
    })
    .await
    .unwrap();
}
