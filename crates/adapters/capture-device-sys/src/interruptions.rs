//! Dequeues that fail with what V4L2 calls a possibly temporary error: EIO
//! from `VIDIOC_DQBUF`, "an internal error" that "can also indicate temporary
//! problems like signal loss". The driver may have taken a buffer it does not
//! name, or stopped capturing, so the stream is restarted (STREAMOFF returns
//! every buffer, STREAMON after queueing them all again). A run of them that
//! lasts [`INTERRUPTED_FOR`] without a buffer between ends capture with the
//! last error, so a dead device does not restart forever.
use std::time::{Duration, Instant};

/// How long dequeues may keep failing before capture fails: long enough for
/// a signal to come back, as [`crate::rejections`] allows a resync.
const INTERRUPTED_FOR: Duration = Duration::from_secs(3);

/// The pause before each restart, so a device that fails at once is not
/// restarted in a tight loop.
pub(crate) const PAUSE: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(crate) struct Interruptions {
    since: Option<Instant>,
}

impl Interruptions {
    /// A buffer was dequeued, usable or not: the device is streaming again.
    pub(crate) fn dequeued(&mut self) {
        self.since = None;
    }

    /// A dequeue failed at `now`: `true` to restart the stream, `false` once
    /// the run has lasted [`INTERRUPTED_FOR`] and capture fails.
    pub(crate) fn interrupted(&mut self, now: Instant) -> bool {
        let since = *self.since.get_or_insert(now);
        now.saturating_duration_since(since) < INTERRUPTED_FOR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_restarts_until_its_time_is_up() {
        let start = Instant::now();
        let mut interruptions = Interruptions::default();
        assert!(interruptions.interrupted(start));
        assert!(interruptions.interrupted(start + INTERRUPTED_FOR - PAUSE));
        assert!(!interruptions.interrupted(start + INTERRUPTED_FOR));
        assert!(
            !interruptions.interrupted(start + INTERRUPTED_FOR * 2),
            "a dead device keeps failing"
        );
    }

    #[test]
    fn a_dequeued_buffer_ends_the_run() {
        let start = Instant::now();
        let mut interruptions = Interruptions::default();
        assert!(interruptions.interrupted(start));
        interruptions.dequeued();
        let later = start + INTERRUPTED_FOR * 10;
        assert!(interruptions.interrupted(later), "a new run starts");
        assert!(!interruptions.interrupted(later + INTERRUPTED_FOR));
    }
}
