//! Totals of what the engine's sessions did since the process started, for
//! the portal's opt-in `/metrics`. The session thread only adds to atomics
//! (no locks, no allocation); readers copy them out as [`Counters`].
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::time::{Duration, Instant};

/// Shared by the engine and its sessions; never reset, so every total only
/// grows and Prometheus can take rates and averages across sessions.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    /// Odd while [`Self::frame`] adds a frame's totals, and changed by each
    /// frame, so a reader can tell a copy that split a frame (a sequence lock
    /// whose writer never waits).
    frame_writes: AtomicU64,
    frames_received: AtomicU64,
    frames_sent: AtomicU64,
    processing: Timer,
    flushed: Timer,
    /// Like `flushed`, from the frame's capture time, for frames that carry one.
    captured: Timer,
    /// For flushed frames, the loop's time for each: from the previous frame
    /// being done to its own write returning, waiting for it included.
    cycles: Timer,
    output_writes: Timer,
    output_write_failures: AtomicU64,
    source_opens: AtomicU64,
    output_opens: AtomicU64,
}

/// A copy of the totals at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Frames taken from the source.
    pub frames_received: u64,
    /// Frames whose colors were written to the output.
    pub frames_sent: u64,
    /// Turning frames into colors.
    pub processing: Timing,
    /// Frames whose colors reached the output, each timed from when Prysm
    /// received it from the source to when the output write returned.
    pub flushed: Timing,
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
    /// One more span: two relaxed adds.
    fn add(&self, span: Duration) {
        let nanos = u64::try_from(span.as_nanos()).unwrap_or(u64::MAX);
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    fn read(&self) -> Timing {
        Timing {
            count: self.count.load(Ordering::Relaxed),
            total: Duration::from_nanos(self.nanos.load(Ordering::Relaxed)),
        }
    }
}

/// When one frame passed each step on the session thread.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameTimes {
    /// The previous frame was done (its write returned or failed); `None`
    /// for the session's first frame.
    pub(crate) previous_done: Option<Instant>,
    /// The source handed it over.
    pub(crate) received: Instant,
    /// The source captured it, if it says.
    pub(crate) captured: Option<Instant>,
    /// Its colors were ready.
    pub(crate) processed: Instant,
    /// The write of its colors returned successfully; `None` if opening or
    /// writing the output failed.
    pub(crate) written: Option<Instant>,
}

/// The totals the engine measures a window from, copied at one moment: no
/// frame is in some of them and not in others.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub(crate) counters: Counters,
    /// For flushed frames with a capture time, the span from that time to
    /// Prysm receiving the frame. Not a metric: the engine reports it as the
    /// capture part of the latency when every frame of a window had one.
    pub(crate) captured: Timing,
    /// For flushed frames, the loop's time for each, waiting for it included.
    /// Not a metric: against it the engine judges whether the loop has slack.
    pub(crate) cycles: Timing,
}

/// Copies a reader tries before taking a split one: a frame's adds take
/// nanoseconds, frames come at most every few milliseconds.
const COPY_ATTEMPTS: usize = 64;

impl Stats {
    #[must_use]
    pub(crate) fn read(&self) -> Counters {
        self.totals().counters
    }

    /// The totals with every frame in all of them or in none.
    #[must_use]
    pub(crate) fn totals(&self) -> Totals {
        let mut copy = Totals::default();
        for _ in 0..COPY_ATTEMPTS {
            let whole;
            (copy, whole) = self.try_totals();
            if whole {
                break;
            }
            std::hint::spin_loop();
        }
        // Still split only behind a writer that stopped mid-frame: what
        // readers had before.
        copy
    }

    /// One copy, and whether no frame was being added meanwhile.
    fn try_totals(&self) -> (Totals, bool) {
        // Acquire pairs with the release ending each frame: every frame
        // counted before `before` is in the copy.
        let before = self.frame_writes.load(Ordering::Acquire);
        let copy = self.copy();
        // Pairs with the fence starting each frame: a copy that saw any of
        // a frame's adds sees its start in `after`.
        fence(Ordering::Acquire);
        let after = self.frame_writes.load(Ordering::Relaxed);
        (copy, before == after && before.is_multiple_of(2))
    }

    fn copy(&self) -> Totals {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let counters = Counters {
            frames_received: load(&self.frames_received),
            frames_sent: load(&self.frames_sent),
            processing: self.processing.read(),
            flushed: self.flushed.read(),
            output_writes: self.output_writes.read(),
            output_write_failures: load(&self.output_write_failures),
            source_opens: load(&self.source_opens),
            output_opens: load(&self.output_opens),
        };
        Totals {
            counters,
            captured: self.captured.read(),
            cycles: self.cycles.read(),
        }
    }

    /// One frame taken from the source: counted and its processing timed;
    /// if its colors reached the output, also counted as sent and its spans
    /// from receipt (and from capture, if known) to the write returning.
    pub(crate) fn frame(&self, times: FrameTimes) {
        self.begin_frame();
        self.frames_received.fetch_add(1, Ordering::Relaxed);
        self.processing.add(times.processed - times.received);
        if let Some(written) = times.written {
            self.frames_sent.fetch_add(1, Ordering::Relaxed);
            self.flushed.add(written - times.received);
            // The first frame waited for nothing that counts.
            self.cycles
                .add(written - times.previous_done.unwrap_or(times.received));
            if let Some(captured) = times.captured {
                // Capture to received: the part before Prysm.
                self.captured
                    .add(times.received.saturating_duration_since(captured));
            }
        }
        self.end_frame();
    }

    /// Mark a frame's adds as under way; see [`Self::try_totals`].
    fn begin_frame(&self) {
        self.frame_writes.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
    }

    fn end_frame(&self) {
        self.frame_writes.fetch_add(1, Ordering::Release);
    }

    /// One output write, a frame's or a keep-alive rewrite, that took `span`.
    pub(crate) fn output_write(&self, span: Duration, ok: bool) {
        self.output_writes.add(span);
        if !ok {
            self.output_write_failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn source_opened(&self) {
        self.source_opens.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn output_opened(&self) {
        self.output_opens.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flushed_frame(stats: &Stats, at: Instant) {
        let span = Duration::from_millis(5);
        stats.frame(FrameTimes {
            previous_done: None,
            received: at,
            captured: Some(at.checked_sub(span).unwrap_or(at)),
            processed: at + span,
            written: Some(at + 2 * span),
        });
    }

    #[test]
    fn a_frame_is_in_every_total_or_in_none() {
        let stats = Stats::default();
        flushed_frame(&stats, Instant::now());
        let (totals, whole) = stats.try_totals();
        assert!(whole);
        assert_eq!(totals.counters.frames_received, 1);
        assert_eq!(totals.counters.flushed.count, 1);
        assert_eq!(totals.captured.count, 1);
        assert_eq!(totals.cycles.count, 1);
        assert_eq!(stats.totals(), totals);
    }

    /// A copy taken while a frame's adds are under way is refused: the
    /// frame may be in some totals and not yet in others.
    #[test]
    fn a_copy_during_a_frame_is_refused() {
        let stats = Stats::default();
        stats.begin_frame();
        stats.frames_received.fetch_add(1, Ordering::Relaxed);
        assert!(!stats.try_totals().1);
        stats.end_frame();
        let (totals, whole) = stats.try_totals();
        assert!(whole);
        assert_eq!(totals.counters.frames_received, 1);
    }
}
