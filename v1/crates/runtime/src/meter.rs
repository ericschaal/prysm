//! The running session's once-a-second input measurement: frame rates and
//! the mean frame span, from the totals [`Stats`] keeps.
use crate::stats::{Counters, Stats, Timing};
use prysm_ports::{Input, Latency};
use std::time::Duration;
use tokio::time::{Instant, Interval, MissedTickBehavior};

/// How often [`Snapshot::input`] is measured and republished.
const MEASURE: Duration = Duration::from_secs(1);

/// Frame-rate measurement of the running session: the session thread only
/// counts ([`Stats`]); the engine samples the count every [`MEASURE`].
pub(crate) struct Meter {
    interval: Interval,
    /// The totals at the last sample.
    last: Counters,
    last_captured: Timing,
    at: Instant,
}

impl Meter {
    /// The next measurement is due. Cancel-safe.
    pub(crate) async fn tick(&mut self) {
        self.interval.tick().await;
    }

    /// Measure from the totals when the session starts.
    pub(crate) fn new(stats: &Stats) -> Self {
        let mut interval = tokio::time::interval_at(Instant::now() + MEASURE, MEASURE);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self {
            interval,
            last: stats.read(),
            last_captured: stats.captured(),
            at: Instant::now(),
        }
    }

    /// The rates and the mean frame spans since the last sample, to one
    /// decimal: received to written for every flushed frame, and capture to
    /// received when every frame flushed since had a capture time.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "per-second frame counts and spans are far below f64 precision and f32 range"
    )]
    pub(crate) fn sample(&mut self, stats: &Stats) -> Input {
        let (counters, captured) = (stats.read(), stats.captured());
        let now = Instant::now();
        let elapsed = now.duration_since(self.at).as_secs_f64();
        let received = counters
            .frames_received
            .saturating_sub(self.last.frames_received);
        let sent = counters.frames_sent.saturating_sub(self.last.frames_sent);
        let flushed = counters
            .flushed
            .count
            .saturating_sub(self.last.flushed.count);
        // Received to written, for every flushed frame.
        let output = counters
            .flushed
            .total
            .saturating_sub(self.last.flushed.total);
        // Capture to received, only when every flushed frame had a capture time.
        let stamped = captured.count.saturating_sub(self.last_captured.count);
        let capture = (flushed > 0 && stamped == flushed)
            .then(|| captured.total.saturating_sub(self.last_captured.total));
        self.last = counters;
        self.last_captured = captured;
        self.at = now;
        let tenth = |value: f64| ((value * 10.0).round() / 10.0) as f32;
        let rate = |counted: u64| {
            if elapsed > 0.0 {
                tenth(counted as f64 / elapsed)
            } else {
                0.0
            }
        };
        Input {
            fps: rate(received),
            receiving: received > 0,
            sent_fps: rate(sent),
            latency: (flushed > 0).then(|| {
                let mean_ms = |span: Duration| tenth(span.as_secs_f64() * 1000.0 / flushed as f64);
                Latency {
                    capture_ms: capture.map(mean_ms),
                    output_ms: mean_ms(output),
                }
            }),
        }
    }
}
