use prysm_ports::{AdapterSettings, LightingTarget, Processing, Registry, SessionState, Settings};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{Counters, Engine, EngineHandle};
use std::time::Duration;

async fn start(source: FakeSourceSettings, output: FakeOutputSettings) -> (EngineHandle, Engine) {
    let log = CallLog::new();
    let mut registry = Registry::default();
    registry.add_source(FakeSources(log.clone()));
    registry.add_output(FakeOutputs(log));
    let store = MemoryStore::with(Settings {
        capture: Some(AdapterSettings::new("fake", &source)),
        output: Some(AdapterSettings::new("fake", &output)),
        processing: Processing::default(),
    });
    Engine::start(registry, Box::new(store), Duration::from_secs(2))
        .await
        .unwrap()
}

/// Turn lighting on and wait until the session has ended on its own.
async fn light_until_ended(handle: &EngineHandle) {
    handle.set_target(LightingTarget::Processed).await.unwrap();
    handle
        .snapshot()
        .wait_for(|s| s.session == SessionState::Stopped)
        .await
        .unwrap();
}

#[tokio::test]
async fn counters_total_frames_and_opens_across_sessions() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let source = FakeSourceSettings {
            end_after: Some(20),
            ..Default::default()
        };
        let (handle, engine) = start(source, FakeOutputSettings::default()).await;
        assert_eq!(handle.stats(), Counters::default());

        light_until_ended(&handle).await;
        let first = handle.stats();
        assert_eq!((first.frames_received, first.frames_sent), (20, 20));
        assert_eq!(first.processing.count, 20);
        assert_eq!(first.output_writes.count, 20);
        assert!(first.processing.total > Duration::ZERO);
        assert_eq!((first.source_opens, first.output_opens), (1, 1));
        assert_eq!(first.output_write_failures, 0);

        // A second session reopens both and keeps counting from the totals.
        light_until_ended(&handle).await;
        let second = handle.stats();
        assert_eq!((second.frames_received, second.frames_sent), (40, 40));
        assert_eq!((second.source_opens, second.output_opens), (2, 2));
        assert!(second.processing.total >= first.processing.total);
        engine.shutdown().await.unwrap();
    })
    .await
    .expect("test timed out");
}

#[tokio::test]
async fn a_failing_output_write_is_counted() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let output = FakeOutputSettings {
            fail_after: Some(5),
            ..Default::default()
        };
        let (handle, engine) = start(FakeSourceSettings::default(), output).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        handle
            .snapshot()
            .wait_for(|s| s.failure.is_some())
            .await
            .unwrap();
        let stats = handle.stats();
        assert_eq!((stats.frames_received, stats.frames_sent), (6, 5));
        assert_eq!(stats.output_writes.count, 6);
        assert_eq!(stats.output_write_failures, 1);
        engine.shutdown().await.unwrap();
    })
    .await
    .expect("test timed out");
}
