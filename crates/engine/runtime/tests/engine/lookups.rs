//! Candidates and capture modes: lookups outside the mailbox, each once at
//! a time.
use crate::common::*;
use prysm_ports::{AdapterSettings, Section};
use prysm_runtime::fakes::{CallLog, FakeSourceSettings};

/// The gate a gated lookup waits for.
const OPEN: &str = "test.open";

fn modes(gated: bool, width: u32) -> AdapterSettings {
    AdapterSettings::new(
        "fake",
        &FakeSourceSettings {
            modes_until: gated.then(|| OPEN.to_owned()),
            width,
            height: 9,
            ..Default::default()
        },
    )
}

/// Who asks for a lookup that runs gets its answer, also after the first
/// asker stopped waiting: no second blocking call starts. Another lookup
/// runs beside it, and the same one runs again once it has returned.
#[tokio::test]
async fn a_running_lookup_is_shared_by_who_asks_meanwhile() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        let gated = modes(true, 32);
        let first = tokio::spawn({
            let (handle, gated) = (handle.clone(), gated.clone());
            async move { handle.capture_modes(gated).await }
        });
        assert!(log.wait_for("source.modes", 1).await);
        handle.capture_modes(modes(false, 64)).await.unwrap();
        handle.candidates(Section::Output, "fake").await.unwrap();
        assert_eq!(log.count("source.modes"), 2);
        first.abort();
        let (shared, ()) = tokio::join!(handle.capture_modes(gated.clone()), async {
            log.record(OPEN);
        });
        let shared = shared.unwrap();
        assert_eq!(log.count("source.modes"), 2, "no second call");
        assert_eq!(handle.capture_modes(gated).await.unwrap(), shared);
        assert_eq!(log.count("source.modes"), 3);
        engine.shutdown().await.unwrap();
    })
    .await;
}
