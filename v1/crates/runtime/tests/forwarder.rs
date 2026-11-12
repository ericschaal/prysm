use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{CallLog, FakeSink, FakeSource, FakeSourceSettings};
use std::time::Duration;

#[tokio::test]
async fn forwards_frames_and_closes_the_source_on_stop() {
    let log = CallLog::new();
    let source = FakeSource::new(&log, FakeSourceSettings::default());
    let run = Forwarder::new(source, FakeSink(log.clone())).run({
        let log = log.clone();
        async move {
            assert!(log.wait_for("sink.send", 5).await);
        }
    });
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("forwarder stops")
        .unwrap();

    let events = log.events();
    let closed = events.iter().position(|e| e == "source.close:").unwrap();
    assert!(
        events[..closed]
            .iter()
            .filter(|e| *e == "sink.send")
            .count()
            >= 5
    );
    assert_eq!(closed, events.len() - 1);
}

#[tokio::test]
async fn source_failure_ends_forwarding_with_the_failure() {
    let log = CallLog::new();
    let source = FakeSource::new(
        &log,
        FakeSourceSettings {
            fail_after: Some(2),
            ..Default::default()
        },
    );
    let run = Forwarder::new(source, FakeSink(log.clone())).run(std::future::pending());
    let result = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("forwarder ends on failure");
    assert!(result.is_err());
    assert_eq!(log.count("sink.send"), 2);
    assert_eq!(log.count("source.close:"), 1);
}

#[tokio::test]
async fn an_ended_source_finishes_forwarding_without_a_failure() {
    let log = CallLog::new();
    let source = FakeSource::new(
        &log,
        FakeSourceSettings {
            end_after: Some(2),
            ..Default::default()
        },
    );
    let run = Forwarder::new(source, FakeSink(log.clone())).run(std::future::pending());
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("forwarder ends with its source")
        .unwrap();
    assert_eq!(log.count("sink.send"), 2);
    assert_eq!(log.count("source.close:"), 1);
}
