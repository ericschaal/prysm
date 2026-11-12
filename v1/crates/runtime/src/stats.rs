//! Totals of what the engine's sessions did since the process started, for
//! the portal's opt-in `/metrics`. The session thread only adds to atomics
//! (no locks, no allocation); readers copy them out with [`Stats::read`].
use prysm_core::EdgeColors;
use prysm_ports::{LedOutput, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Shared by the engine and its sessions; never reset, so every total only
/// grows and Prometheus can take rates and averages across sessions.
#[derive(Debug, Default)]
pub struct Stats {
    frames_received: AtomicU64,
    frames_sent: AtomicU64,
    processing: Timer,
    output_writes: Timer,
    output_write_failures: AtomicU64,
    source_opens: AtomicU64,
    output_opens: AtomicU64,
}

/// A copy of [`Stats`] at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Frames taken from the source.
    pub frames_received: u64,
    /// Frames whose colors were written to the output.
    pub frames_sent: u64,
    /// Turning frames into colors.
    pub processing: Timing,
    /// Every output write: frames and the keep-alive rewrites, failed ones included.
    pub output_writes: Timing,
    /// Output writes that failed (each ends its session).
    pub output_write_failures: u64,
    /// Capture sources opened by starting lighting.
    pub source_opens: u64,
    /// Outputs opened to show colors: at a session's first frame and at the
    /// first frame after a release.
    pub output_opens: u64,
}

/// How many times something ran and how long it took in total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timing {
    pub count: u64,
    pub total: Duration,
}

#[derive(Debug, Default)]
struct Timer {
    count: AtomicU64,
    nanos: AtomicU64,
}

impl Timer {
    fn time<T>(&self, work: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let done = work();
        let nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        done
    }

    fn read(&self) -> Timing {
        Timing {
            count: self.count.load(Ordering::Relaxed),
            total: Duration::from_nanos(self.nanos.load(Ordering::Relaxed)),
        }
    }
}

impl Stats {
    #[must_use]
    pub fn read(&self) -> Counters {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        Counters {
            frames_received: load(&self.frames_received),
            frames_sent: load(&self.frames_sent),
            processing: self.processing.read(),
            output_writes: self.output_writes.read(),
            output_write_failures: load(&self.output_write_failures),
            source_opens: load(&self.source_opens),
            output_opens: load(&self.output_opens),
        }
    }

    pub(crate) fn frame_received(&self) {
        self.frames_received.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn frame_sent(&self) {
        self.frames_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn source_opened(&self) {
        self.source_opens.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn output_opened(&self) {
        self.output_opens.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn process<T>(&self, work: impl FnOnce() -> T) -> T {
        self.processing.time(work)
    }

    /// Write `colors`, timing the write and counting a failure.
    pub(crate) fn write(&self, output: &mut dyn LedOutput, colors: &EdgeColors) -> Result<()> {
        let written = self.output_writes.time(|| output.write(colors));
        if written.is_err() {
            self.output_write_failures.fetch_add(1, Ordering::Relaxed);
        }
        written
    }
}
