//! Consecutive buffers the device delivered but capture could not use. One
//! is dropped quietly; a run of them ends capture with
//! [`CaptureError::Unusable`], so the stream does not look alive while
//! delivering nothing.
use crate::{BufferRejection, CaptureError, Result};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

/// How long a run of unusable buffers lasts before capture fails. Several
/// seconds, so a signal resync (a few corrupted frames) passes, and a fast
/// burst (such as error-flagged buffers a disappearing device may return
/// before its node goes and DQBUF says ENODEV) does not fail early. The
/// count bound (this long's worth of buffers) keeps a few buffers far apart,
/// either side of stalls, from failing.
pub(crate) const UNUSABLE_FOR: Duration = Duration::from_secs(3);

pub(crate) struct Rejections {
    /// Buffers in [`UNUSABLE_FOR`] at the configured rate.
    limit: u32,
    rejected: u32,
    since: Option<Instant>,
}

impl Rejections {
    pub(crate) fn new(rate: NonZeroU32) -> Self {
        let seconds = u32::try_from(UNUSABLE_FOR.as_secs()).expect("a few seconds");
        Self {
            limit: rate.get().saturating_mul(seconds),
            rejected: 0,
            since: None,
        }
    }

    /// A frame was delivered: the run is over.
    pub(crate) fn accepted(&mut self) {
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

    #[test]
    fn a_single_unusable_buffer_is_harmless() {
        let mut rejections = Rejections::new(RATE);
        let start = Instant::now();
        rejections
            .rejected(BufferRejection::Corrupted, start)
            .unwrap();
        // However long ago, one buffer is not a run.
        rejections.accepted();
        rejections
            .rejected(BufferRejection::Corrupted, start + UNUSABLE_FOR * 10)
            .unwrap();
    }

    #[test]
    fn a_delivered_frame_ends_the_run() {
        let mut rejections = Rejections::new(RATE);
        let start = Instant::now();
        for buffer in 0..LIMIT - 1 {
            rejections
                .rejected(BufferRejection::Corrupted, start + UNUSABLE_FOR * buffer)
                .unwrap();
        }
        rejections.accepted();
        for buffer in 0..LIMIT - 1 {
            rejections
                .rejected(
                    BufferRejection::Corrupted,
                    start + UNUSABLE_FOR * (10 + buffer),
                )
                .unwrap();
        }
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
            chain(&error),
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
        // Buffers far apart, such as either side of stalls, need the count too.
        let mut rejections = Rejections::new(RATE);
        let start = Instant::now();
        for buffer in 0..LIMIT - 1 {
            rejections
                .rejected(BufferRejection::Corrupted, start + UNUSABLE_FOR * buffer)
                .unwrap();
        }
        assert!(
            rejections
                .rejected(BufferRejection::Corrupted, start + UNUSABLE_FOR * LIMIT)
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

    /// The error and its causes, joined as people read them.
    fn chain(error: &CaptureError) -> String {
        let mut text = error.to_string();
        let mut cause = std::error::Error::source(error);
        while let Some(next) = cause {
            text.push_str(": ");
            text.push_str(&next.to_string());
            cause = next.source();
        }
        text
    }
}
