//! Totals of what the engine's sessions did since the process started, for
//! the portal's opt-in `/metrics`. Every total lives behind one mutex: the
//! session thread locks it once per frame and once per output write, only to
//! add; readers lock it to copy.
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Shared by the engine and its sessions; never reset, so every total only
/// grows and Prometheus can take rates and averages across sessions.
#[derive(Debug, Default)]
pub(crate) struct Stats(Mutex<Totals>);

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

impl Timing {
    fn add(&mut self, span: Duration) {
        self.count += 1;
        self.total = self.total.saturating_add(span);
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

/// Every total, copied under the lock: a frame, a write or an open is in all
/// of them or in none.
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

impl Stats {
    #[must_use]
    pub(crate) fn read(&self) -> Counters {
        self.totals().counters
    }

    #[must_use]
    pub(crate) fn totals(&self) -> Totals {
        *self.lock()
    }

    // Plain counters: a panicking holder cannot leave them half-written.
    fn lock(&self) -> std::sync::MutexGuard<'_, Totals> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One frame taken from the source: counted and its processing timed;
    /// if its colors reached the output, also counted as sent and its spans
    /// from receipt (and from capture, if known) to the write returning.
    pub(crate) fn frame(&self, times: FrameTimes) {
        let processing = times.processed - times.received;
        let flushed = times.written.map(|written| {
            (
                written - times.received,
                // The first frame waited for nothing that counts.
                written - times.previous_done.unwrap_or(times.received),
                // Capture to received: the part before Prysm.
                times
                    .captured
                    .map(|captured| times.received.saturating_duration_since(captured)),
            )
        });
        let mut totals = self.lock();
        totals.counters.frames_received += 1;
        totals.counters.processing.add(processing);
        if let Some((flushed, cycle, captured)) = flushed {
            totals.counters.frames_sent += 1;
            totals.counters.flushed.add(flushed);
            totals.cycles.add(cycle);
            if let Some(captured) = captured {
                totals.captured.add(captured);
            }
        }
    }

    /// One output write, a frame's or a keep-alive rewrite, that took `span`.
    pub(crate) fn output_write(&self, span: Duration, ok: bool) {
        let mut totals = self.lock();
        totals.counters.output_writes.add(span);
        if !ok {
            totals.counters.output_write_failures += 1;
        }
    }

    pub(crate) fn source_opened(&self) {
        self.lock().counters.source_opens += 1;
    }

    pub(crate) fn output_opened(&self) {
        self.lock().counters.output_opens += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    const SPAN: Duration = Duration::from_millis(5);

    fn flushed_frame(stats: &Stats, at: Instant) {
        stats.frame(FrameTimes {
            previous_done: None,
            received: at,
            captured: Some(at.checked_sub(SPAN).unwrap_or(at)),
            processed: at + SPAN,
            written: Some(at + 2 * SPAN),
        });
    }

    #[test]
    fn a_flushed_frame_is_in_every_frame_total() {
        let stats = Stats::default();
        flushed_frame(&stats, Instant::now());
        let totals = stats.totals();
        assert_eq!(totals.counters.frames_received, 1);
        assert_eq!(totals.counters.frames_sent, 1);
        assert_eq!(totals.counters.processing.count, 1);
        assert_eq!(totals.counters.flushed.count, 1);
        assert_eq!(totals.cycles.count, 1);
        assert_eq!(totals.captured.count, 1);
        assert_eq!(totals.counters.flushed.total, 2 * SPAN);
        assert_eq!(stats.read(), totals.counters);
    }

    #[test]
    fn writes_and_opens_are_counted() {
        let stats = Stats::default();
        stats.output_write(SPAN, true);
        stats.output_write(SPAN, false);
        stats.source_opened();
        stats.output_opened();
        let counters = stats.read();
        assert_eq!(counters.output_writes.count, 2);
        assert_eq!(counters.output_writes.total, 2 * SPAN);
        assert_eq!(counters.output_write_failures, 1);
        assert_eq!((counters.source_opens, counters.output_opens), (1, 1));
    }

    /// A reader racing a writer never sees a frame in some totals and not
    /// in others.
    #[test]
    fn copies_taken_while_frames_are_added_are_whole() {
        let frames = if cfg!(miri) { 50 } else { 20_000 };
        let stats = Arc::new(Stats::default());
        let writer = thread::spawn({
            let stats = stats.clone();
            move || {
                let at = Instant::now();
                for _ in 0..frames {
                    flushed_frame(&stats, at);
                }
            }
        });
        // Checked before copying, so the last copy holds every frame added.
        let mut finished = false;
        while !finished {
            finished = writer.is_finished();
            let totals = stats.totals();
            let received = totals.counters.frames_received;
            assert_eq!(totals.counters.frames_sent, received);
            assert_eq!(totals.counters.processing.count, received);
            assert_eq!(totals.counters.flushed.count, received);
            assert_eq!(totals.cycles.count, received);
            assert_eq!(totals.captured.count, received);
            assert_eq!(
                totals.counters.flushed.total,
                2 * SPAN * u32::try_from(received).unwrap()
            );
        }
        // A writer that panicked fails the test here instead of hanging it.
        writer.join().unwrap();
        assert_eq!(stats.read().frames_received, frames);
    }
}
