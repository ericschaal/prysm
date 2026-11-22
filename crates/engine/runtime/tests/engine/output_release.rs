//! When the output shows the session's colours: device native, quiet sources, the colors feed.
use crate::common::*;
use prysm_ports::{LightingTarget, Registry, SessionState, Settings};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{ColorsWatch, Engine};

#[tokio::test]
async fn device_native_idles_the_output_without_a_session() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        handle
            .set_target(LightingTarget::DeviceNative)
            .await
            .unwrap();
        assert_eq!(handle.snapshot().borrow().session, SessionState::Native);
        assert_eq!(log.events(), ["output.open:a", "output.idle:a:Native"]);

        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(log.count("output.idle:a:Off"), 1);
        assert_eq!(log.count("source.open:a"), 0);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn quiet_source_rewrites_last_colors() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let source = FakeSourceSettings {
            name: "s".into(),
            quiet_after: Some(2),
            ..Default::default()
        };
        let output = FakeOutputSettings {
            name: "o".into(),
            ..Default::default()
        };
        let mut registry = Registry::default();
        registry.add_source(FakeSources(log.clone()));
        registry.add_output(FakeOutputs(log.clone()));
        let store = MemoryStore::with(settings(source, output));
        let (handle, engine) = Engine::start(registry, Box::new(store), DEADLINE)
            .await
            .unwrap();
        handle.set_target(LightingTarget::Processed).await.unwrap();
        // Two frames, then at least one refresh of the last colors.
        assert!(log.wait_for("output.write:o", 3).await);
        engine.shutdown().await.unwrap();
    })
    .await;
}

fn quiet(after: u32, for_ms: u64) -> Settings {
    settings(
        FakeSourceSettings {
            name: "s".into(),
            quiet_after: Some(after),
            quiet_for_ms: for_ms,
            ..Default::default()
        },
        FakeOutputSettings {
            name: "o".into(),
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn a_source_without_frames_never_takes_the_output() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, quiet(0, 0), DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        let waiting = snapshot_until(&handle, |s| {
            s.input.is_some_and(|i| !i.receiving && i.fps == 0.0)
        })
        .await;
        assert_eq!(waiting.session, SessionState::Running);

        // Off leaves an untouched output untouched, and the whole session
        // never opened it.
        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(log.events(), ["source.open:s", "source.close:s"]);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_quiet_source_releases_the_output_until_frames_return() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, quiet(3, 3_600), DEADLINE).await;
        let colors = handle.colors();
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:o", 3).await);
        assert_eq!(log.count("output.open:o"), 1);
        colors_until(&colors, true).await;

        // After the grace period the device returns to its own state, and
        // the colors it no longer shows are withdrawn.
        assert!(log.wait_for("output.idle:o:Native", 1).await);
        assert!(colors.latest().is_none());
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.session, SessionState::Running);
        assert!(snapshot.input.is_some_and(|i| !i.receiving));
        assert!(snapshot.failure.is_none());

        // The next frame takes it again.
        assert!(log.wait_for("output.open:o", 2).await);
        assert!(log.position("output.idle:o:Native") < log.last_position("output.open:o"));
        let writes = log.count("output.write:o");
        assert!(log.wait_for("output.write:o", writes + 1).await);
        colors_until(&colors, true).await;

        handle.set_target(LightingTarget::Off).await.unwrap();
        assert!(colors.latest().is_none());
        assert_eq!(log.count("output.idle:o:Off"), 1);
        assert_eq!(log.count("output.open:o"), 2);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn stopping_while_released_leaves_the_output_in_the_stop_mode() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, quiet(2, 0), DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.idle:o:Native", 1).await);

        // Already native: nothing to reopen.
        handle
            .set_target(LightingTarget::DeviceNative)
            .await
            .unwrap();
        assert_eq!(log.count("output.open:o"), 1);
        assert_eq!(handle.snapshot().borrow().session, SessionState::Native);

        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.idle:o:Native", 2).await);
        // Released to native, stopped Off: reopened only to turn it off.
        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(log.count("output.open:o"), 3);
        assert!(before(&log, "source.close:s", "output.idle:o:Off"));
        assert_eq!(log.count("output.idle:o:Off"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// Wait until the colors feed holds colors (`present`) or none.
async fn colors_until(watch: &ColorsWatch, present: bool) {
    eventually_async("the colors feed", LIMIT, || {
        (watch.latest().is_some() == present).then_some(())
    })
    .await;
}

/// Colors are those the LEDs show: none while off or showing the device's own
/// effects, and none published at all while nobody watches.
#[tokio::test]
async fn colors_follow_the_lights_and_only_while_watched() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        let colors = handle.colors();
        assert_eq!(handle.colors_viewers(), 1);
        assert_eq!(handle.preview_viewers(), 0, "colors are not a preview");
        assert!(colors.latest().is_none(), "off");

        handle.set_target(LightingTarget::Processed).await.unwrap();
        colors_until(&colors, true).await;
        let written = colors.latest().unwrap();
        assert!(!written.top.is_empty() && !written.left.is_empty());

        handle
            .set_target(LightingTarget::DeviceNative)
            .await
            .unwrap();
        assert!(colors.latest().is_none(), "effects show their own colors");

        handle.set_target(LightingTarget::Processed).await.unwrap();
        colors_until(&colors, true).await;
        handle.set_target(LightingTarget::Off).await.unwrap();
        assert!(colors.latest().is_none());
        drop(colors);
        assert_eq!(handle.colors_viewers(), 0);
        engine.shutdown().await.unwrap();
    })
    .await;
}
