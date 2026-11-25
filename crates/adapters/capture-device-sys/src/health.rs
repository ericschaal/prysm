//! Whether a device capture is still healthy: one rule for V4L2 and
//! AVFoundation, fed what each buffer (or its absence) was.
//!
//! - A frame ends any run of trouble.
//! - A stall (no buffer within [`stall_timeout`]) ends any run too, so a source
//!   that is off, and yields a bad buffer now and then, stays "waiting for
//!   video" instead of failing. V4L2 restarts its stream after a stall, at
//!   once: the stall already waited.
//! - Unusable buffers in a row are dropped quietly until the run has lasted
//!   [`GRACE`] and held that long's worth of buffers at the configured rate;
//!   then capture fails with [`CaptureError::Unusable`], so a stream does not
//!   look alive while delivering nothing. The time bound lets a signal resync
//!   (a few corrupted frames) pass, and a fast burst (such as error-flagged
//!   buffers a disappearing device returns before its node goes and DQBUF says
//!   ENODEV) does not fail early. The count bound keeps a device that delivers
//!   bad buffers well below its rate, without stalling, from failing after a
//!   few of them.
//! - An interruption (V4L2's EIO on DQBUF or QBUF) restarts the stream once:
//!   STREAMOFF clears the queue error that fails every DQBUF until then. A
//!   second one before a frame or a stall fails capture with it, since
//!   nothing shows that more restarts would help. An interruption also ends
//!   a run of unusable buffers: the restarted stream starts afresh.
use crate::{BufferRejection, CaptureError};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

/// [`GRACE`] in whole seconds.
const GRACE_SECONDS: u32 = 3;

/// How long a run of unusable buffers lasts before capture fails: long
/// enough for a signal to come back.
const GRACE: Duration = Duration::from_secs(GRACE_SECONDS as u64);

/// How long capture waits for a buffer before it counts a stall: a second,
/// plus one frame period at `rate`.
pub(crate) fn stall_timeout(rate: NonZeroU32) -> Duration {
    Duration::from_secs(1) + Duration::from_secs(1) / rate.get()
}

/// What one wait for a buffer brought.
#[derive(Debug)]
pub(crate) enum Event {
    /// A usable frame, delivered.
    Frame,
    /// A buffer that could not be used, and why.
    Rejected(BufferRejection),
    /// No buffer within [`stall_timeout`].
    Stalled,
    /// An I/O error a stream restart may clear; `error` if it does not.
    #[cfg_attr(
        not(target_os = "linux"),
        allow(dead_code, reason = "only a V4L2 stream is interrupted")
    )]
    Interrupted(CaptureError),
}

/// What capture does next.
#[derive(Debug)]
pub(crate) enum Verdict {
    /// Wait for the next buffer.
    Continue,
    /// Restart the stream, then wait for the next buffer.
    Restart,
    /// Stop capturing with this error.
    Fail(CaptureError),
}

/// `count` unusable buffers in a row, the first at `since`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rejecting {
    since: Instant,
    count: u32,
}

pub(crate) struct Health {
    /// Buffers in [`GRACE`] at the configured rate.
    limit: u32,
    /// The stream was restarted after an interruption, and has not
    /// delivered since.
    restarted: bool,
    rejecting: Option<Rejecting>,
}

impl Health {
    pub(crate) fn new(rate: NonZeroU32) -> Self {
        Self {
            limit: rate.get().saturating_mul(GRACE_SECONDS),
            restarted: false,
            rejecting: None,
        }
    }

    /// The verdict on `event` at `now`. A run that starts is logged once.
    pub(crate) fn on(&mut self, event: Event, now: Instant) -> Verdict {
        match event {
            Event::Frame => {
                self.restarted = false;
                self.rejecting = None;
                Verdict::Continue
            }
            Event::Stalled => {
                self.restarted = false;
                self.rejecting = None;
                Verdict::Restart
            }
            Event::Rejected(reason) => {
                let run = match self.rejecting {
                    Some(Rejecting { since, count }) => Rejecting {
                        since,
                        count: count.saturating_add(1),
                    },
                    None => {
                        tracing::warn!(%reason, "Capture device delivered an unusable buffer");
                        Rejecting {
                            since: now,
                            count: 1,
                        }
                    }
                };
                self.rejecting = Some(run);
                if run.count >= self.limit && now.saturating_duration_since(run.since) >= GRACE {
                    Verdict::Fail(CaptureError::Unusable {
                        rejected: run.count,
                        source: reason,
                    })
                } else {
                    Verdict::Continue
                }
            }
            Event::Interrupted(error) => {
                self.rejecting = None;
                if self.restarted {
                    return Verdict::Fail(error);
                }
                self.restarted = true;
                tracing::warn!(error = %prysm_ports::chain(&error), "Capture interrupted; restarting the stream");
                Verdict::Restart
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::PayloadError;

    const RATE: NonZeroU32 = NonZeroU32::new(2).unwrap();
    /// Buffers at `RATE` in [`GRACE`].
    const LIMIT: u32 = 6;

    /// The verdicts, comparable.
    #[derive(Debug, PartialEq)]
    enum Kind {
        Continue,
        Restart,
        Unusable,
        Interrupted,
    }

    fn kind(verdict: &Verdict) -> Kind {
        match verdict {
            Verdict::Continue => Kind::Continue,
            Verdict::Restart => Kind::Restart,
            Verdict::Fail(CaptureError::Unusable { .. }) => Kind::Unusable,
            Verdict::Fail(_) => Kind::Interrupted,
        }
    }

    fn corrupted() -> Event {
        Event::Rejected(BufferRejection::Payload(PayloadError::Corrupted))
    }

    fn interrupted() -> Event {
        Event::Interrupted(CaptureError::Backend(anyhow::anyhow!("EIO")))
    }

    /// Every state against every event: the state after it and the verdict.
    #[test]
    fn every_state_meets_every_event() {
        let start = Instant::now();
        let now = start + GRACE * 10;
        // A run's start: just now, or longer ago than the grace.
        let young = now - GRACE / 2;
        let old = now - GRACE;
        let rejecting = |since, count| Some(Rejecting { since, count });
        let states = [
            ("clean", (false, None)),
            ("rejecting, young", (false, rejecting(young, LIMIT))),
            ("rejecting, few", (false, rejecting(old, LIMIT - 2))),
            ("rejecting, due", (false, rejecting(old, LIMIT - 1))),
            ("restarted", (true, None)),
            (
                "restarted, rejecting, due",
                (true, rejecting(old, LIMIT - 1)),
            ),
        ];
        type Named = (&'static str, fn() -> Event);
        let events: [Named; 4] = [
            ("frame", || Event::Frame),
            ("rejected", corrupted),
            ("stalled", || Event::Stalled),
            ("interrupted", interrupted),
        ];
        use Kind::{Continue, Interrupted, Restart, Unusable};
        let clean = (false, None);
        // Rows follow `states`, columns `events`.
        let expected = [
            [
                (clean, Continue),
                ((false, rejecting(now, 1)), Continue),
                (clean, Restart),
                ((true, None), Restart),
            ],
            [
                (clean, Continue),
                ((false, rejecting(young, LIMIT + 1)), Continue),
                (clean, Restart),
                ((true, None), Restart),
            ],
            [
                (clean, Continue),
                ((false, rejecting(old, LIMIT - 1)), Continue),
                (clean, Restart),
                ((true, None), Restart),
            ],
            [
                (clean, Continue),
                ((false, rejecting(old, LIMIT)), Unusable),
                (clean, Restart),
                ((true, None), Restart),
            ],
            [
                (clean, Continue),
                ((true, rejecting(now, 1)), Continue),
                (clean, Restart),
                ((true, None), Interrupted),
            ],
            [
                (clean, Continue),
                ((true, rejecting(old, LIMIT)), Unusable),
                (clean, Restart),
                ((true, None), Interrupted),
            ],
        ];
        for ((name, (restarted, run)), row) in states.into_iter().zip(expected) {
            for ((event_name, event), (next, verdict)) in events.iter().zip(row) {
                let mut health = Health {
                    limit: LIMIT,
                    restarted,
                    rejecting: run,
                };
                let got = health.on(event(), now);
                assert_eq!(
                    ((health.restarted, health.rejecting), kind(&got)),
                    (next, verdict),
                    "{name} + {event_name}"
                );
            }
        }
    }

    /// Checks that `end` ends a run: neither its count nor its start carries
    /// into the next one.
    fn assert_ends_the_run(end: fn() -> Event) {
        let start = Instant::now();
        // The count: one short of it either side, far apart in time.
        let mut health = Health::new(RATE);
        for buffer in 0..LIMIT - 1 {
            let verdict = health.on(corrupted(), start + GRACE * buffer);
            assert_eq!(kind(&verdict), Kind::Continue);
        }
        health.on(end(), start + GRACE * LIMIT);
        for buffer in 0..LIMIT - 1 {
            let verdict = health.on(corrupted(), start + GRACE * (10 + buffer));
            assert_eq!(kind(&verdict), Kind::Continue);
        }
        // The start: a single old buffer, then twice the count at one instant.
        let mut health = Health::new(RATE);
        health.on(corrupted(), start);
        health.on(end(), start);
        for _ in 0..LIMIT * 2 {
            let verdict = health.on(corrupted(), start + GRACE * 10);
            assert_eq!(kind(&verdict), Kind::Continue);
        }
    }

    #[test]
    fn a_delivered_frame_ends_the_run() {
        assert_ends_the_run(|| Event::Frame);
    }

    #[test]
    fn a_stall_ends_the_run() {
        assert_ends_the_run(|| Event::Stalled);
    }

    #[test]
    fn a_run_fails_with_its_last_reason_after_its_time_and_count() {
        let mut health = Health::new(RATE);
        let start = Instant::now();
        let period = GRACE / LIMIT;
        for buffer in 0..LIMIT {
            let verdict = health.on(corrupted(), start + period * buffer);
            assert_eq!(kind(&verdict), Kind::Continue);
        }
        let short = BufferRejection::Payload(PayloadError::Short {
            bytes: 10,
            frame: 20,
        });
        let Verdict::Fail(error) = health.on(Event::Rejected(short.clone()), start + GRACE) else {
            panic!("the run is due");
        };
        let CaptureError::Unusable { rejected, source } = &error else {
            panic!("expected unusable buffers, got {error:?}")
        };
        assert_eq!(*rejected, LIMIT + 1);
        assert_eq!(*source, short);
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
        let mut health = Health::new(RATE);
        let start = Instant::now();
        for _ in 0..LIMIT * 100 {
            assert_eq!(kind(&health.on(corrupted(), start)), Kind::Continue);
        }
        assert_eq!(kind(&health.on(corrupted(), start + GRACE)), Kind::Unusable);
    }

    #[test]
    fn a_slow_run_waits_for_its_count() {
        // A bad buffer every second: below `RATE`, but closer together than
        // the stall timeout, so no stall ends the run.
        let mut health = Health::new(RATE);
        let start = Instant::now();
        let period = Duration::from_secs(1);
        for buffer in 0..LIMIT - 1 {
            let verdict = health.on(corrupted(), start + period * buffer);
            assert_eq!(kind(&verdict), Kind::Continue);
        }
        assert_eq!(
            kind(&health.on(corrupted(), start + period * (LIMIT - 1))),
            Kind::Unusable
        );
    }

    #[test]
    fn the_stall_timeout_is_a_second_and_a_frame() {
        assert_eq!(
            stall_timeout(NonZeroU32::new(4).unwrap()),
            Duration::from_millis(1250)
        );
    }
}
