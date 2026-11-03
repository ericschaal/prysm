//! Allocation-free counters with bounded log frequency, independent of Prometheus.
use prysm_core::FramePoolStats;
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Window {
    frames: u64,
    bytes: u64,
    failures: u64,
    cached: u64,
    elapsed: Duration,
    maximum: Duration,
}

/// One producer/worker's successful frames, wall-time cost and failures.
/// Emits INFO on `prysm::stats` every 30 seconds of activity and on drop.
/// Read durations include I/O waits. Pool counters are cumulative snapshots.
/// This adds no per-frame allocation, lock or background task.
pub struct TraceStats {
    stage: &'static str,
    enabled: bool,
    since: Instant,
    window: Window,
    total_frames: u64,
    total_failures: u64,
    total_cached: u64,
    pool: Option<FramePoolStats>,
}
impl TraceStats {
    #[must_use]
    pub fn new(stage: &'static str) -> Self {
        Self {
            stage,
            enabled: tracing::enabled!(target: "prysm::stats", tracing::Level::INFO),
            since: Instant::now(),
            window: Window::default(),
            total_frames: 0,
            total_failures: 0,
            total_cached: 0,
            pool: None,
        }
    }
    /// Start a measurement only when summary logging is enabled.
    #[must_use]
    pub fn start(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }
    /// Record one successful operation; bytes refer to this stage's payload.
    pub fn record(&mut self, started: Option<Instant>, bytes: usize, pool: Option<FramePoolStats>) {
        let Some(started) = started else {
            return;
        };
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(started);
        self.window.frames += 1;
        self.window.bytes += bytes as u64;
        self.window.elapsed += elapsed;
        self.window.maximum = self.window.maximum.max(elapsed);
        self.total_frames += 1;
        self.pool = pool;
        self.report_if_due(now);
    }
    /// Count a failed operation. Returns true for the first failure in this window.
    /// Callers can use this to avoid repeating the same warning every frame.
    pub fn failure(&mut self) -> bool {
        self.report_if_due(Instant::now());
        let first = self.window.failures == 0;
        self.window.failures += 1;
        self.total_failures += 1;
        first
    }
    /// Count a preview response served from the encoded cache, without a new encode.
    pub fn cache_hit(&mut self) {
        if !self.enabled {
            return;
        }
        self.window.cached += 1;
        self.total_cached += 1;
        self.report_if_due(Instant::now());
    }
    fn report_if_due(&mut self, now: Instant) {
        if now.saturating_duration_since(self.since) >= INTERVAL {
            self.report(now, false);
        }
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "diagnostic rates use floating-point counters"
    )]
    fn report(&mut self, now: Instant, final_report: bool) {
        if self.enabled {
            let seconds = now.saturating_duration_since(self.since).as_secs_f64();
            let frames = self.window.frames as f64;
            tracing::info!(target: "prysm::stats",
            stage = self.stage, final_report,
            window_seconds = seconds,
            frames = self.window.frames,
            fps = frames / seconds.max(f64::EPSILON),
            bytes = self.window.bytes,
            mean_ms = self.window.elapsed.as_secs_f64() * 1000.0 / frames.max(1.0),
            max_ms = self.window.maximum.as_secs_f64() * 1000.0,
            failures = self.window.failures, cached = self.window.cached,
            total_frames = self.total_frames,
            total_failures = self.total_failures,
            total_cached = self.total_cached,
            pool = ?self.pool,
            "frame statistics"
            );
        }
        self.window = Window::default();
        self.since = now;
    }
}
impl Drop for TraceStats {
    fn drop(&mut self) {
        if self.total_frames + self.total_failures + self.total_cached > 0 {
            self.report(Instant::now(), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn summaries_are_periodic_and_final_and_preserve_totals() {
        let capture = Capture::default();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let mut stats = TraceStats::new("test");
            let mut pool = prysm_core::FramePool::default();
            let _ = pool.acquire(4, 2, prysm_core::PixelFormat::RGB24).unwrap();
            stats.record(stats.start(), 24, Some(pool.stats()));
            assert!(capture.0.lock().unwrap().is_empty());
            assert_eq!(stats.window.frames, 1);
            stats.report_if_due(stats.since + INTERVAL);
            assert_eq!(stats.window.frames, 0);
            assert_eq!(stats.total_frames, 1);
            stats.cache_hit();
            assert!(stats.failure());
            assert!(!stats.failure());
            drop(stats);
        });
        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert_eq!(output.matches("frame statistics").count(), 2, "{output}");
        for field in [
            "final_report=false",
            "final_report=true",
            "frames=1",
            "bytes=24",
            "cached=1",
            "failures=2",
            "total_frames=1",
            "created: 1",
            "retained_buffers: 1",
        ] {
            assert!(output.contains(field), "missing {field}: {output}");
        }
    }
    #[test]
    fn disabled_summaries_skip_measurements_and_output() {
        let capture = Capture::default();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::registry()
            .with(
                "info,prysm::stats=off"
                    .parse::<tracing_subscriber::filter::Targets>()
                    .unwrap(),
            )
            .with(tracing_subscriber::fmt::layer().with_writer(move || writer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            let mut stats = TraceStats::new("disabled");
            assert!(stats.start().is_none());
            stats.record(stats.start(), 100, None);
            stats.cache_hit();
            assert_eq!(stats.total_frames, 0);
            assert!(stats.failure());
            assert!(!stats.failure());
            stats.report_if_due(stats.since + INTERVAL);
            assert!(stats.failure());
        });
        assert!(capture.0.lock().unwrap().is_empty());
    }
}
