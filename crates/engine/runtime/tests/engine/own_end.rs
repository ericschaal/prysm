//! An exhausted finite source completes the current request.
use crate::common::*;
use prysm_ports::{LightingTarget, SessionState};
use prysm_runtime::fakes::{CallLog, FakeOutputSettings, FakeSourceSettings};

#[tokio::test]
async fn a_finite_source_turns_off_once_and_can_be_replayed_by_request() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    end_after: Some(2),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        for count in 1..=2 {
            handle.set_target(LightingTarget::ScreenSync).await.unwrap();
            snapshot_until(&handle, |s| {
                s.target == LightingTarget::Off
                    && matches!(s.session, SessionState::Stopped { failure: None })
            })
            .await;
            assert_eq!(log.count("source.open:s"), count);
            assert_eq!(log.count("source.close:s"), count);
            assert_eq!(log.count("output.idle:o:Off"), count);
            assert_eq!(log.count("output.write:o"), count * 2);
        }
        engine.shutdown().await.unwrap();
    })
    .await;
}
