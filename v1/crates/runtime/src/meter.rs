//! The running session's once-a-second input measurement: frame rates, the
//! mean frame spans and whether Prysm keeps up, from the totals [`Stats`]
//! keeps.
use crate::stats::{Counters, Stats, Timing};
use prysm_ports::{Behind, Input, Latency};
use std::time::Duration;
use tokio::time::{Instant, Interval, MissedTickBehavior};

/// How often [`Snapshot::input`] is measured and republished.
const MEASURE: Duration = Duration::from_secs(1);

/// A window counts as saturated when the frame loop spent at least this share
/// of it on frames (received to written): little or no idle time waiting for
/// the next frame, so frames are being dropped.
const SATURATED: f64 = 0.85;

/// Consecutive windows that set [`Behind`] (saturated) or clear it (not), so
/// one slow second neither reports nor flickers.
const GRACE: u8 = 3;

/// [`Behind`] with hysteresis over the one-second windows.
#[derive(Debug, Default)]
struct Pace {
    reported: Behind,
    /// Consecutive windows disagreeing with `reported`.
    streak: u8,
    /// The cause of the disagreeing windows so far.
    pending: Behind,
}

impl Pace {
    /// One window's verdict in, what to report out.
    fn window(&mut self, now: Behind) -> Behind {
        if now == self.reported {
            self.streak = 0;
        } else {
            // A saturated window of either cause counts toward reporting;
            // the latest cause wins. A clear window counts toward clearing.
            let same_side = now.is_none() == self.pending.is_none();
            self.streak = if same_side { self.streak + 1 } else { 1 };
            self.pending = now;
            if self.streak >= GRACE {
                self.reported = now;
                self.streak = 0;
            }
        }
        self.reported
    }
}

/// One window's verdict: which step, if the loop had no slack. `busy` is the
/// received-to-written time of the window's flushed frames, `processing` its
/// received-to-processed part, `elapsed` the window's length.
fn verdict(busy: Duration, processing: Duration, elapsed: Duration) -> Behind {
    if elapsed.is_zero() || busy.as_secs_f64() < SATURATED * elapsed.as_secs_f64() {
        return Behind::None;
    }
    let output = busy.saturating_sub(processing);
    if processing >= output {
        Behind::Processing
    } else {
        Behind::Output
    }
}

/// Frame-rate measurement of the running session: the session thread only
/// counts ([`Stats`]); the engine samples the count every [`MEASURE`].
pub(crate) struct Meter {
    interval: Interval,
    /// The totals at the last sample.
    last: Counters,
    last_captured: Timing,
    at: Instant,
    pace: Pace,
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
            pace: Pace::default(),
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
        let window = now.duration_since(self.at);
        let elapsed = window.as_secs_f64();
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
        // No slack, and which step took it.
        let processing = counters
            .processing
            .total
            .saturating_sub(self.last.processing.total);
        let behind = self.pace.window(verdict(output, processing, window));
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
            behind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);
    const fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn a_busy_window_names_the_larger_step() {
        assert_eq!(verdict(ms(950), ms(800), SECOND), Behind::Processing);
        assert_eq!(verdict(ms(950), ms(100), SECOND), Behind::Output);
    }

    /// A source sending fewer frames (24 fps film, a dim camera) leaves the
    /// loop idle between them: never behind, however slow each frame is.
    #[test]
    fn idle_time_between_frames_is_never_behind() {
        // 24 frames of 20 ms each: busy 480 ms of the second.
        assert_eq!(verdict(ms(480), ms(400), SECOND), Behind::None);
        assert_eq!(
            verdict(Duration::ZERO, Duration::ZERO, SECOND),
            Behind::None
        );
        assert_eq!(verdict(ms(10), ms(10), Duration::ZERO), Behind::None);
    }

    #[test]
    fn it_takes_several_windows_to_report_and_to_clear() {
        let mut pace = Pace::default();
        // One slow window is not falling behind.
        assert_eq!(pace.window(Behind::Output), Behind::None);
        assert_eq!(pace.window(Behind::None), Behind::None);
        // Three in a row are; mixed causes still count, the latest names it.
        assert_eq!(pace.window(Behind::Output), Behind::None);
        assert_eq!(pace.window(Behind::Processing), Behind::None);
        assert_eq!(pace.window(Behind::Output), Behind::Output);
        // One good window does not clear it.
        assert_eq!(pace.window(Behind::None), Behind::Output);
        assert_eq!(pace.window(Behind::Output), Behind::Output);
        // Three in a row do.
        for _ in 0..2 {
            assert_eq!(pace.window(Behind::None), Behind::Output);
        }
        assert_eq!(pace.window(Behind::None), Behind::None);
    }

    /// While behind, a lasting change of cause is followed after the grace.
    #[test]
    fn a_lasting_new_cause_takes_over() {
        let mut pace = Pace::default();
        for _ in 0..GRACE {
            pace.window(Behind::Output);
        }
        assert_eq!(pace.window(Behind::Processing), Behind::Output);
        assert_eq!(pace.window(Behind::Processing), Behind::Output);
        assert_eq!(pace.window(Behind::Processing), Behind::Processing);
    }
}
