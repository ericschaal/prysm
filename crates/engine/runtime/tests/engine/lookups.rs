//! Candidates and capture modes: lookups outside the mailbox, each once at
//! a time.
use crate::common::*;
use prysm_ports::{AdapterSettings, Section};
use prysm_runtime::fakes::{CallLog, FakeSourceSettings};

fn modes(delay_ms: u64, width: u32) -> AdapterSettings {
    AdapterSettings::new(
        "fake",
        &FakeSourceSettings {
            modes_delay_ms: delay_ms,
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
        let slow = modes(300, 32);
        let first = tokio::spawn({
            let (handle, slow) = (handle.clone(), slow.clone());
            async move { handle.capture_modes(slow).await }
        });
        assert!(log.wait_for("source.modes", 1).await);
        // Other settings are another lookup, and so are candidates.
        handle.capture_modes(modes(0, 64)).await.unwrap();
        handle.candidates(Section::Output, "fake").await.unwrap();
        assert_eq!(log.count("source.modes"), 2);
        // Given up on, the first still runs; asking the same waits for it.
        first.abort();
        let shared = handle.capture_modes(slow.clone()).await.unwrap();
        assert_eq!(log.count("source.modes"), 2, "no second call");
        // Returned: the next ask looks again.
        assert_eq!(handle.capture_modes(slow).await.unwrap(), shared);
        assert_eq!(log.count("source.modes"), 3);
        engine.shutdown().await.unwrap();
    })
    .await;
}
