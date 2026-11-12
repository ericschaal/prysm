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

/// Counts sends and idle ticks.
struct Counting(CallLog);

impl prysm_ports::FrameSink for Counting {
    fn send(&mut self, _frame: &prysm_core::Frame) -> prysm_ports::Result<()> {
        self.0.record("sink.send");
        Ok(())
    }
    fn idle(&mut self) -> prysm_ports::Result<()> {
        self.0.record("sink.idle");
        Ok(())
    }
}

/// A quiet source still gives the sink a turn every tick, so a sink that
/// connects to a peer does not wait for the first frame.
#[tokio::test]
async fn a_quiet_source_gives_the_sink_idle_turns() {
    let log = CallLog::new();
    let source = FakeSource::new(
        &log,
        FakeSourceSettings {
            quiet_after: Some(0),
            ..Default::default()
        },
    );
    let run = Forwarder::new(source, Counting(log.clone())).run({
        let log = log.clone();
        async move {
            assert!(log.wait_for("sink.idle", 2).await);
        }
    });
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("forwarder stops")
        .unwrap();
    assert_eq!(log.count("sink.send"), 0);
}
