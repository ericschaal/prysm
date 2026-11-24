//! What makes a failure work again: set once where the error is typed
//! ([`crate::Error::recovery`]), read by every layer, derived by none.
//!
//! A [`Recovery::Retry`] or [`Recovery::Wait`] is acted on only by the layer
//! that owns the operation and never leaves it; giving up is always
//! [`Escalation::TRY_AGAIN`]. Only an [`Escalation`] travels further.
use std::time::{Duration, Instant};

/// What makes a failure work again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// It passes by itself: the owning layer tries again on the schedule of
    /// [`Converges`], whose variant says why that reaches a working state.
    Retry(Converges),
    /// It passes when this changes; nothing is timed.
    Wait(Signal),
    /// Nothing in the layer that met it can make it work again.
    Escalate(Escalation),
}

/// What is left once a failure leaves the layer that met it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    /// Only a new process recovers (a hung or unconfirmed native call,
    /// vendor state left broken): the supervisor starts one, once.
    Restart,
    /// The person must act: shown with this step, and resumed only by it.
    Ask(Fix),
}

impl Escalation {
    /// Fail fast: the default for an adapter failure, and what a retry
    /// becomes when its schedule gives up.
    pub const TRY_AGAIN: Self = Self::Ask(Fix::TryAgain);
}

/// A change that ends a [`Recovery::Wait`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// The TV's video signal or context changes (input, app, picture mode,
    /// power).
    Video,
    /// The slim receiver sends a new stream request.
    ReceiverRequest,
}

/// The person's next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// Start again (the lighting mode, capture).
    TryAgain,
    /// Change the settings; a save that validates replaces them.
    ChangeSettings,
    /// Run the TV service setup again.
    SetUpService,
    /// Install matching versions of Prysm and the TV app.
    Update,
    /// Restart the TV: a native resource stays held until a reboot.
    RestartTv,
    /// Nothing makes this work on this device.
    Unsupported,
}

/// Why trying again reaches a working state: one variant per timed retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Converges {
    /// LG finalizes a capture session that is not ready yet (status 1), and
    /// the next initialization starts it again once the TV has woken.
    DriverPreparing,
    /// A native capture start lost its wake-up; one fresh start recovers it.
    StartStall,
    /// A video device reports an I/O error while its signal resyncs, and
    /// delivers again once it has.
    SignalResyncs,
    /// The lights are off, booting or off the network, and come back on
    /// their own; only a connection shows that they have.
    LightsReachable,
    /// The receiving Prysm accepts once its lighting is on or it is updated;
    /// the TV is the client, so connecting is the only way to learn it.
    ReceiverListens,
    /// The TV's own services register after Prysm's at boot, and a call
    /// starts a service that is not running.
    ServiceRegisters,
}

impl Converges {
    /// When to try again after each failure.
    #[must_use]
    pub const fn schedule(self) -> Schedule {
        match self {
            Self::DriverPreparing => Schedule::Within {
                every: Duration::from_secs(1),
                limit: Duration::from_secs(10),
            },
            Self::StartStall => Schedule::Once,
            Self::SignalResyncs => Schedule::Within {
                every: Duration::from_millis(100),
                limit: Duration::from_secs(3),
            },
            Self::LightsReachable => Schedule::Doubling {
                first: Duration::from_secs(1),
                max: Duration::from_secs(30),
            },
            Self::ReceiverListens => Schedule::Every(Duration::from_secs(1)),
            Self::ServiceRegisters => Schedule::Every(Duration::from_secs(5)),
        }
    }
}

/// When a [`Recovery::Retry`] tries again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// After this long, for as long as it fails.
    Every(Duration),
    /// After `first`, doubling up to `max`, for as long as it fails.
    Doubling { first: Duration, max: Duration },
    /// After `every`, until failures have lasted `limit`.
    Within { every: Duration, limit: Duration },
    /// At once, one time.
    Once,
}

/// The tries of one [`Recovery::Retry`]: the only state a timed retry keeps.
#[derive(Debug, Clone)]
pub struct Attempts {
    converges: Converges,
    /// Failures since the last success.
    failures: u32,
    /// The first of them.
    since: Option<Instant>,
}

impl Attempts {
    #[must_use]
    pub const fn new(converges: Converges) -> Self {
        Self {
            converges,
            failures: 0,
            since: None,
        }
    }

    /// After a failure at `now`: when to try again, or, once the schedule
    /// gives up, the person's next step.
    ///
    /// # Errors
    /// [`Escalation::TRY_AGAIN`] once the schedule gives up.
    pub fn failed(&mut self, now: Instant) -> Result<Instant, Escalation> {
        self.failures = self.failures.saturating_add(1);
        let since = *self.since.get_or_insert(now);
        let wait = match self.converges.schedule() {
            Schedule::Every(every) => Some(every),
            Schedule::Doubling { first, max } => Some(
                first
                    .saturating_mul(2_u32.saturating_pow(self.failures - 1))
                    .min(max),
            ),
            Schedule::Within { every, limit } => {
                (now.duration_since(since) < limit).then_some(every)
            }
            Schedule::Once => (self.failures == 1).then_some(Duration::ZERO),
        };
        wait.map(|wait| now + wait).ok_or(Escalation::TRY_AGAIN)
    }

    /// It worked: the next failure starts the schedule over.
    pub fn succeeded(&mut self) {
        self.failures = 0;
        self.since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The waits of `failures` failures one `apart`, `None` for a give-up.
    fn waits(converges: Converges, failures: u32, apart: Duration) -> Vec<Option<Duration>> {
        let start = Instant::now();
        let mut attempts = Attempts::new(converges);
        (0..failures)
            .map(|n| {
                let now = start + apart * n;
                attempts.failed(now).ok().map(|at| at - now)
            })
            .collect()
    }

    const fn secs(seconds: u64) -> Option<Duration> {
        Some(Duration::from_secs(seconds))
    }

    #[test]
    fn every_retries_for_as_long_as_it_fails() {
        assert_eq!(
            waits(Converges::ReceiverListens, 4, Duration::from_secs(60)),
            [secs(1); 4]
        );
    }

    #[test]
    fn doubling_is_capped_and_starts_over_after_a_success() {
        let expected = [1, 2, 4, 8, 16, 30, 30].map(secs);
        assert_eq!(
            waits(Converges::LightsReachable, 7, Duration::ZERO),
            expected
        );
        let now = Instant::now();
        let mut attempts = Attempts::new(Converges::LightsReachable);
        attempts.failed(now).unwrap();
        attempts.failed(now).unwrap();
        attempts.succeeded();
        assert_eq!(attempts.failed(now), Ok(now + Duration::from_secs(1)));
    }

    #[test]
    fn within_gives_up_once_failures_last_its_limit() {
        let tries = waits(Converges::DriverPreparing, 12, Duration::from_secs(1));
        assert_eq!(tries[..10], [secs(1); 10]);
        assert_eq!(tries[10..], [None, None]);

        let now = Instant::now();
        let late = now + Duration::from_secs(10);
        let mut attempts = Attempts::new(Converges::DriverPreparing);
        attempts.failed(now).unwrap();
        assert_eq!(attempts.failed(late), Err(Escalation::TRY_AGAIN));
        attempts.succeeded();
        assert_eq!(attempts.failed(late), Ok(late + Duration::from_secs(1)));
    }

    #[test]
    fn once_retries_at_once_one_time() {
        assert_eq!(
            waits(Converges::StartStall, 3, Duration::ZERO),
            [Some(Duration::ZERO), None, None]
        );
    }
}
