//! Buffers the device delivered without a gap but capture could not use. One
//! is dropped quietly; a run of them ends capture with
//! [`CaptureError::Unusable`], so the stream does not look alive while
//! delivering nothing. A frame or a stall (no buffer within the dequeue
//! timeout) ends the run, so a source that is off and yields a bad buffer
//! now and then stays "waiting for video".
use crate::{BufferRejection, CaptureError, Result};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

/// [`UNUSABLE_FOR`] in whole seconds.
const UNUSABLE_SECONDS: u32 = 3;

/// How long a run of unusable buffers lasts before capture fails. Several
/// seconds, so a signal resync (a few corrupted frames) passes, and a fast
/// burst (such as error-flagged buffers a disappearing device may return
/// before its node goes and DQBUF says ENODEV) does not fail early. The
/// count bound (this long's worth of buffers at the configured rate) keeps a
/// device that delivers bad buffers well below its rate, without stalling,
/// from failing after only a few of them.
const UNUSABLE_FOR: Duration = Duration::from_secs(UNUSABLE_SECONDS as u64);

pub(crate) struct Rejections {
    /// Buffers in [`UNUSABLE_FOR`] at the configured rate.
    limit: u32,
    rejected: u32,
    since: Option<Instant>,
}

impl Rejections {
    pub(crate) fn new(rate: NonZeroU32) -> Self {
        Self {
            limit: rate.get().saturating_mul(UNUSABLE_SECONDS),
            rejected: 0,
            since: None,
        }
    }

    /// A frame was delivered: the run is over.
    pub(crate) fn accepted(&mut self) {
        self.end_run();
    }

    /// No buffer arrived within the dequeue timeout: buffers either side of
    /// the gap are not in a row.
    pub(crate) fn stalled(&mut self) {
        self.end_run();
    }

    fn end_run(&mut self) {
        self.rejected = 0;
        self.since = None;
    }

    /// A buffer was unusable for `reason` at `now`. Fails once the run has
    /// lasted [`UNUSABLE_FOR`] and held that long's worth of buffers.
    pub(crate) fn rejected(&mut self, reason: BufferRejection, now: Instant) -> Result<()> {
        self.rejected = self.rejected.saturating_add(1);
        let since = *self.since.get_or_insert(now);
        if self.rejected >= self.limit && now.saturating_duration_since(since) >= UNUSABLE_FOR {
            return Err(CaptureError::Unusable {
                rejected: self.rejected,
                source: reason,
            });
        }
        Ok(())
    }
}

/// Why a dequeued buffer of `bytes` was unusable, when a full frame is
/// `frame` bytes and the driver set its error flag if `corrupted`.
pub(crate) fn rejection(corrupted: bool, bytes: usize, frame: usize) -> BufferRejection {
    if corrupted {
        BufferRejection::Corrupted
    } else if bytes < frame {
        BufferRejection::Short { bytes, frame }
    } else {
        BufferRejection::Unreadable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: NonZeroU32 = NonZeroU32::new(2).unwrap();
    /// Buffers at `RATE` in [`UNUSABLE_FOR`].
    const LIMIT: u32 = 6;

    /// Checks that `end` ends a run: neither its count nor its start carries
    /// into the next one.
    fn assert_ends_the_run(end: fn(&mut Rejections)) {
        let start = Instant::now();
        // The count: one short of it either side, far apart in time.
        let mut rejections = Rejections::new(RATE);
        for buffer in 0..LIMIT - 1 {
            rejections
                .rejected(BufferRejection::Corrupted, start + UNUSABLE_FOR * buffer)
                .unwrap();
        }
        end(&mut rejections);
        for buffer in 0..LIMIT - 1 {
            rejections
                .rejected(
                    BufferRejection::Corrupted,
                    start + UNUSABLE_FOR * (10 + buffer),
                )
                .unwrap();
        }
        // The start: a single old buffer, then twice the count at one instant.
        let mut rejections = Rejections::new(RATE);
        rejections
            .rejected(BufferRejection::Corrupted, start)
            .unwrap();
        end(&mut rejections);
        for _ in 0..LIMIT * 2 {
            rejections
                .rejected(BufferRejection::Corrupted, start + UNUSABLE_FOR * 10)
                .unwrap();
        }
    }

    #[test]
    fn a_delivered_frame_ends_the_run() {
        assert_ends_the_run(Rejections::accepted);
    }

    #[test]
    fn a_stall_ends_the_run() {
        assert_ends_the_run(Rejections::stalled);
    }

    #[test]
    fn a_run_fails_with_its_last_reason_after_its_time_and_count() {
        let mut rejections = Rejections::new(RATE);
        let start = Instant::now();
        let period = UNUSABLE_FOR / LIMIT;
        for buffer in 0..LIMIT {
            rejections
                .rejected(BufferRejection::Corrupted, start + period * buffer)
                .unwrap();
        }
        let error = rejections
            .rejected(
                BufferRejection::Short {
                    bytes: 10,
                    frame: 20,
                },
                start + UNUSABLE_FOR,
            )
            .unwrap_err();
        let CaptureError::Unusable { rejected, source } = &error else {
            panic!("expected unusable buffers, got {error:?}")
        };
        assert_eq!(*rejected, LIMIT + 1);
        assert_eq!(
            *source,
            BufferRejection::Short {
                bytes: 10,
                frame: 20
            }
        );
        assert_eq!(
            prysm_ports::chain(&error),
            "capture device delivered 7 unusable buffers in a row: \
             buffer held 10 of the 20 bytes of a frame"
        );
    }

    #[test]
    fn a_fast_run_waits_for_its_time() {
        // Error-flagged buffers a vanishing device returns at once must not
        // fail before its node is gone and DQBUF reports it.
        let mut rejections = Rejections::new(RATE);
        let start = Instant::now();
        for _ in 0..LIMIT * 100 {
            rejections
                .rejected(BufferRejection::Corrupted, start)
                .unwrap();
        }
        assert!(
            rejections
                .rejected(BufferRejection::Corrupted, start + UNUSABLE_FOR)
                .is_err()
        );
    }

    #[test]
    fn a_slow_run_waits_for_its_count() {
        // A bad buffer every second: below `RATE`, but closer together than
        // the dequeue timeout, so no stall ends the run.
        let mut rejections = Rejections::new(RATE);
        let start = Instant::now();
        let period = Duration::from_secs(1);
        for buffer in 0..LIMIT - 1 {
            rejections
                .rejected(BufferRejection::Corrupted, start + period * buffer)
                .unwrap();
        }
        assert!(
            rejections
                .rejected(BufferRejection::Corrupted, start + period * (LIMIT - 1))
                .is_err()
        );
    }

    #[test]
    fn the_reason_is_the_flag_then_the_size() {
        assert_eq!(rejection(true, 0, 20), BufferRejection::Corrupted);
        assert_eq!(rejection(true, 20, 20), BufferRejection::Corrupted);
        assert_eq!(
            rejection(false, 19, 20),
            BufferRejection::Short {
                bytes: 19,
                frame: 20
            }
        );
        assert_eq!(rejection(false, 20, 20), BufferRejection::Unreadable);
    }
}
