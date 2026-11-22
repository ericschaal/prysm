//! Whether a device capture is still healthy: one rule for V4L2 and
//! AVFoundation, fed what each buffer (or its absence) was.
//!
//! - A frame ends any run of trouble.
//! - A stall (no buffer within [`stall_timeout`]) ends any run too, so a source
//!   that is off, and yields a bad buffer now and then, stays "waiting for
//!   video" instead of failing. V4L2 restarts its stream after a stall.
//! - Unusable buffers in a row are dropped quietly until the run has lasted
//!   [`GRACE`] and held that long's worth of buffers at the configured rate;
//!   then capture fails with [`CaptureError::Unusable`], so a stream does not
//!   look alive while delivering nothing. The time bound lets a signal resync
//!   (a few corrupted frames) pass, and a fast burst (such as error-flagged
//!   buffers a disappearing device returns before its node goes and DQBUF says
//!   ENODEV) does not fail early. The count bound keeps a device that delivers
//!   bad buffers well below its rate, without stalling, from failing after a
//!   few of them.
//! - Interruptions (V4L2's possibly temporary EIO, or a failed restart)
//!   restart the stream, after [`PAUSE`], until a run of them with no frame
//!   and no stall between has lasted [`GRACE`]; then capture fails with the
//!   last one, so a dead device does not restart forever. Unusable buffers
//!   after an interruption neither end nor extend that run, and are held
//!   to the same two bounds as a run of their own, counted from the run's
//!   first interruption: a stream that delivers only bad buffers after an
//!   interruption does not look alive either, and one corrupted buffer
//!   after a late restart does not fail capture.
//! - An interruption ends a run of unusable buffers and starts its own.
//! - Interruptions with stalls between them (EIO, restart, stall, restart,
//!   EIO, ...) never fail capture, on purpose: a source that is off stalls,
//!   and capture keeps "waiting for video".
use crate::{BufferRejection, CaptureError};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

/// [`GRACE`] in whole seconds.
const GRACE_SECONDS: u32 = 3;

/// How long a run of unusable buffers or interruptions lasts before capture
/// fails: long enough for a signal to come back.
const GRACE: Duration = Duration::from_secs(GRACE_SECONDS as u64);

/// The pause before each restart, so a device that fails at once is not
/// restarted in a tight loop.
#[cfg(any(target_os = "linux", test))]
pub(crate) const PAUSE: Duration = Duration::from_millis(100);

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
    /// The device failed in a way that may pass; `error` if it does not.
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

/// The run of trouble capture is in.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Run {
    Clean,
    /// `count` unusable buffers in a row, the first at `since`.
    Rejecting {
        since: Instant,
        count: u32,
    },
    /// Interruptions in a row, the first at `since`, with `rejected`
    /// unusable buffers between them.
    Interrupted {
        since: Instant,
        rejected: u32,
    },
}

pub(crate) struct Health {
    /// Buffers in [`GRACE`] at the configured rate.
    limit: u32,
    run: Run,
}

impl Health {
    pub(crate) fn new(rate: NonZeroU32) -> Self {
        Self {
            limit: rate.get().saturating_mul(GRACE_SECONDS),
            run: Run::Clean,
        }
    }

    /// The verdict on `event` at `now`. A run that starts is logged once.
    pub(crate) fn on(&mut self, event: Event, now: Instant) -> Verdict {
        let lasted = |since: Instant| now.saturating_duration_since(since) >= GRACE;
        let (run, verdict) = match (self.run, event) {
            (_, Event::Frame) => (Run::Clean, Verdict::Continue),
            (_, Event::Stalled) => (Run::Clean, Verdict::Restart),
            // An unusable buffer after an interruption is not the device
            // recovering: the interruption run and its clock go on, under
            // the bounds of a run of unusable buffers.
            (Run::Interrupted { since, rejected }, Event::Rejected(reason)) => {
                let rejected = rejected.saturating_add(1);
                let verdict = if rejected >= self.limit && lasted(since) {
                    Verdict::Fail(CaptureError::Unusable {
                        rejected,
                        source: reason,
                    })
                } else {
                    Verdict::Continue
                };
                (Run::Interrupted { since, rejected }, verdict)
            }
            (run, Event::Rejected(reason)) => {
                let (since, count) = if let Run::Rejecting { since, count } = run {
                    (since, count.saturating_add(1))
                } else {
                    tracing::warn!(%reason, "Capture device delivered an unusable buffer");
                    (now, 1)
                };
                let verdict = if count >= self.limit && lasted(since) {
                    Verdict::Fail(CaptureError::Unusable {
                        rejected: count,
                        source: reason,
                    })
                } else {
                    Verdict::Continue
                };
                (Run::Rejecting { since, count }, verdict)
            }
            (run, Event::Interrupted(error)) => {
                let (since, rejected) = match run {
                    Run::Interrupted { since, rejected } => (since, rejected),
                    Run::Clean | Run::Rejecting { .. } => (now, 0),
                };
                let verdict = if lasted(since) {
                    Verdict::Fail(error)
                } else {
                    tracing::warn!(error = %prysm_ports::chain(&error), "Capture interrupted; restarting the stream");
                    Verdict::Restart
                };
                (Run::Interrupted { since, rejected }, verdict)
            }
        };
        self.run = run;
        verdict
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

    fn health(run: Run) -> Health {
        Health { limit: LIMIT, run }
    }

    /// Every run against every event: the run after it and the verdict.
    #[test]
    fn every_run_meets_every_event() {
        let start = Instant::now();
        let now = start + GRACE * 10;
        // A run's start: just now, or longer ago than the grace.
        let young = now - GRACE / 2;
        let old = now - GRACE;
        let interrupting = |since, rejected| Run::Interrupted { since, rejected };
        let runs = [
            ("clean", Run::Clean),
            (
                "rejecting, young",
                Run::Rejecting {
                    since: young,
                    count: LIMIT,
                },
            ),
            (
                "rejecting, few",
                Run::Rejecting {
                    since: old,
                    count: LIMIT - 2,
                },
            ),
            (
                "rejecting, due",
                Run::Rejecting {
                    since: old,
                    count: LIMIT - 1,
                },
            ),
            ("interrupted, young", interrupting(young, LIMIT - 1)),
            ("interrupted, due", interrupting(old, LIMIT - 1)),
            ("interrupted, due, few rejected", interrupting(old, 0)),
        ];
        type Named = (&'static str, fn() -> Event);
        let events: [Named; 4] = [
            ("frame", || Event::Frame),
            ("rejected", corrupted),
            ("stalled", || Event::Stalled),
            ("interrupted", interrupted),
        ];
        use Kind::{Continue, Interrupted, Restart, Unusable};
        let clean = Run::Clean;
        let rejecting = |since, count| Run::Rejecting { since, count };
        // Rows follow `runs`, columns `events`.
        let expected = [
            [
                (clean, Continue),
                (rejecting(now, 1), Continue),
                (clean, Restart),
                (interrupting(now, 0), Restart),
            ],
            [
                (clean, Continue),
                (rejecting(young, LIMIT + 1), Continue),
                (clean, Restart),
                (interrupting(now, 0), Restart),
            ],
            [
                (clean, Continue),
                (rejecting(old, LIMIT - 1), Continue),
                (clean, Restart),
                (interrupting(now, 0), Restart),
            ],
            [
                (clean, Continue),
                (rejecting(old, LIMIT), Unusable),
                (clean, Restart),
                (interrupting(now, 0), Restart),
            ],
            [
                (clean, Continue),
                (interrupting(young, LIMIT), Continue),
                (clean, Restart),
                (interrupting(young, LIMIT - 1), Restart),
            ],
            [
                (clean, Continue),
                (interrupting(old, LIMIT), Unusable),
                (clean, Restart),
                (interrupting(old, LIMIT - 1), Interrupted),
            ],
            [
                (clean, Continue),
                (interrupting(old, 1), Continue),
                (clean, Restart),
                (interrupting(old, 0), Interrupted),
            ],
        ];
        for ((name, run), row) in runs.into_iter().zip(expected) {
            for ((event_name, event), (next, verdict)) in events.iter().zip(row) {
                let mut health = health(run);
                let got = health.on(event(), now);
                assert_eq!(
                    (health.run, kind(&got)),
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
    fn interruptions_restart_until_their_time_is_up() {
        let start = Instant::now();
        let mut health = Health::new(RATE);
        assert_eq!(kind(&health.on(interrupted(), start)), Kind::Restart);
        assert_eq!(
            kind(&health.on(interrupted(), start + GRACE - PAUSE)),
            Kind::Restart
        );
        assert_eq!(
            kind(&health.on(interrupted(), start + GRACE)),
            Kind::Interrupted
        );
        assert_eq!(
            kind(&health.on(interrupted(), start + GRACE * 2)),
            Kind::Interrupted,
            "a dead device keeps failing"
        );
        // A frame ends the run; the next starts afresh.
        let later = start + GRACE * 10;
        health.on(Event::Frame, later);
        assert_eq!(kind(&health.on(interrupted(), later)), Kind::Restart);
        assert_eq!(
            kind(&health.on(interrupted(), later + GRACE)),
            Kind::Interrupted
        );
    }

    #[test]
    fn alternating_unusable_buffers_and_interruptions_fail_within_the_grace() {
        let start = Instant::now();
        let mut health = Health::new(RATE);
        let mut now = start;
        let failed = loop {
            assert!(now < start + GRACE * 2, "never failed");
            health.on(corrupted(), now);
            now += PAUSE / 2;
            if let Verdict::Fail(error) = health.on(interrupted(), now) {
                break error;
            }
            now += PAUSE / 2;
        };
        assert!(now >= start + GRACE && now < start + GRACE + PAUSE);
        assert!(!matches!(failed, CaptureError::Unusable { .. }));
    }

    #[test]
    fn unusable_buffers_after_one_interruption_fail_after_the_grace() {
        let start = Instant::now();
        let mut health = Health::new(RATE);
        assert_eq!(kind(&health.on(interrupted(), start)), Kind::Restart);
        let period = Duration::from_secs(1) / RATE.get();
        let mut now = start;
        let failed = loop {
            now += period;
            assert!(now <= start + GRACE + period, "never failed");
            if let Verdict::Fail(error) = health.on(corrupted(), now) {
                break error;
            }
        };
        assert!(now >= start + GRACE);
        assert!(
            matches!(failed, CaptureError::Unusable { .. }),
            "{failed:?}"
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
