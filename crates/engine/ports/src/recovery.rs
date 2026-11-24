//! What makes a failure work again: set once where the error is built
//! ([`crate::Error::recovery`]) and read, never changed, by every layer.
//!
//! Only the layer that owns an operation acts on a [`Recovery::Retry`]
//! ([`Attempts::failed`]) or a [`Recovery::Wait`]; every layer above reads
//! [`Recovery::escalation`].
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

impl Recovery {
    /// What a layer above the operation's owner sees. Only the owner acts on
    /// a retry or a wait, so one that reaches another layer fails fast.
    #[must_use]
    pub const fn escalation(self) -> Escalation {
        match self {
            Self::Escalate(escalation) => escalation,
            Self::Retry(_) | Self::Wait(_) => Escalation::FAIL_FAST,
        }
    }
}

/// What is left once a failure leaves the layer that met it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    /// Only a new process recovers (a session that did not stop or did not
    /// confirm its release): the supervisor starts one, once. If that one
    /// faults again before it has worked, it is `Ask(Fix::TryAgain)`. A host
    /// without a supervisor fails fast.
    Restart,
    /// The person must act: shown with this step, and resumed only by it.
    Ask(Fix),
}

impl Escalation {
    /// Fail fast: nothing recovers this automatically; the person is told
    /// and decides whether to try again.
    pub const FAIL_FAST: Self = Self::Ask(Fix::TryAgain);
}

/// A change that ends a [`Recovery::Wait`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// The TV's video signal or context changes (input, app, picture mode,
    /// power).
    Video,
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
    /// Nothing makes this work on this device.
    Unsupported,
}

/// Why trying again reaches a working state: one variant per timed retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Converges {
    /// After the TV wakes, LG's capture driver answers status 1 (not ready)
    /// while it initializes again; the same driver then delivers, so trying
    /// again reaches it. One still not ready after 10 s fails fast.
    DriverPreparing,
    /// A native capture start lost its wake-up and delivers no frame; a
    /// fresh start is woken. One that stalls again fails fast.
    StartStall,
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
        }
    }
}

/// When a [`Recovery::Retry`] tries again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// After `every`, never later than `limit` after the first failure.
    Within { every: Duration, limit: Duration },
    /// At once, one time.
    Once,
}

/// The tries of one [`Recovery::Retry`]: the only state a timed retry keeps.
#[derive(Debug, Clone)]
pub struct Attempts {
    converges: Converges,
    /// The first failure since the last success.
    since: Option<Instant>,
}

impl Attempts {
    #[must_use]
    pub const fn new(converges: Converges) -> Self {
        Self {
            converges,
            since: None,
        }
    }

    /// After a failure at `now` whose recovery is `recovery`: when to try
    /// again.
    ///
    /// # Errors
    /// The failure's own [`Recovery::escalation`] unless it is a retry of
    /// this [`Converges`]; [`Escalation::FAIL_FAST`] once the schedule gives
    /// up.
    pub fn failed(&mut self, recovery: Recovery, now: Instant) -> Result<Instant, Escalation> {
        match recovery {
            Recovery::Retry(converges) if converges == self.converges => {}
            Recovery::Retry(_) | Recovery::Wait(_) | Recovery::Escalate(_) => {
                return Err(recovery.escalation());
            }
        }
        let first = self.since.is_none();
        let since = *self.since.get_or_insert(now);
        let retry = match self.converges.schedule() {
            Schedule::Within { every, limit } => {
                let deadline = since + limit;
                (now < deadline).then(|| (now + every).min(deadline))
            }
            Schedule::Once => first.then_some(now),
        };
        retry.ok_or(Escalation::FAIL_FAST)
    }

    /// The operation really worked (a frame arrived, a connection carried
    /// data), so the next failure starts the schedule over: a failure after
    /// real work is progress, not a loop.
    pub fn succeeded(&mut self) {
        self.since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn retry(converges: Converges) -> Recovery {
        Recovery::Retry(converges)
    }

    /// The waits of `failures` failures one `apart`, `None` for a give-up.
    fn waits(converges: Converges, failures: u32, apart: Duration) -> Vec<Option<Duration>> {
        let start = Instant::now();
        let mut attempts = Attempts::new(converges);
        (0..failures)
            .map(|n| {
                let now = start + apart * n;
                attempts
                    .failed(retry(converges), now)
                    .ok()
                    .map(|at| at - now)
            })
            .collect()
    }

    const fn secs(seconds: u64) -> Option<Duration> {
        Some(Duration::from_secs(seconds))
    }

    #[test]
    fn each_convergence_has_its_schedule() {
        for (converges, schedule) in [
            (
                Converges::DriverPreparing,
                Schedule::Within {
                    every: Duration::from_secs(1),
                    limit: Duration::from_secs(10),
                },
            ),
            (Converges::StartStall, Schedule::Once),
        ] {
            assert_eq!(converges.schedule(), schedule, "{converges:?}");
        }
    }

    #[test]
    fn within_gives_up_once_failures_last_its_limit() {
        let tries = waits(Converges::DriverPreparing, 12, Duration::from_secs(1));
        assert_eq!(tries[..10], [secs(1); 10]);
        assert_eq!(tries[10..], [None, None]);

        let now = Instant::now();
        let late = now + Duration::from_secs(10);
        let preparing = retry(Converges::DriverPreparing);
        let mut attempts = Attempts::new(Converges::DriverPreparing);
        attempts.failed(preparing, now).unwrap();
        assert_eq!(attempts.failed(preparing, late), Err(Escalation::FAIL_FAST));
        attempts.succeeded();
        assert_eq!(
            attempts.failed(preparing, late),
            Ok(late + Duration::from_secs(1))
        );
    }

    #[test]
    fn within_schedules_nothing_after_its_limit() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(10);
        let preparing = retry(Converges::DriverPreparing);
        let mut attempts = Attempts::new(Converges::DriverPreparing);
        attempts.failed(preparing, now).unwrap();
        let just_under = deadline - Duration::from_millis(1);
        assert_eq!(attempts.failed(preparing, just_under), Ok(deadline));
        assert_eq!(
            attempts.failed(preparing, deadline),
            Err(Escalation::FAIL_FAST)
        );
    }

    #[test]
    fn within_keeps_its_first_failure_when_now_goes_backwards() {
        let now = Instant::now() + Duration::from_secs(60);
        let earlier = now - Duration::from_secs(5);
        let preparing = retry(Converges::DriverPreparing);
        let mut attempts = Attempts::new(Converges::DriverPreparing);
        attempts.failed(preparing, now).unwrap();
        assert_eq!(
            attempts.failed(preparing, earlier),
            Ok(earlier + Duration::from_secs(1))
        );
        let deadline = now + Duration::from_secs(10);
        assert_eq!(
            attempts.failed(preparing, deadline),
            Err(Escalation::FAIL_FAST)
        );
    }

    #[test]
    fn once_retries_at_once_one_time_and_starts_over_after_a_success() {
        assert_eq!(
            waits(Converges::StartStall, 3, Duration::ZERO),
            [Some(Duration::ZERO), None, None]
        );
        let now = Instant::now();
        let stall = retry(Converges::StartStall);
        let mut attempts = Attempts::new(Converges::StartStall);
        attempts.failed(stall, now).unwrap();
        assert_eq!(attempts.failed(stall, now), Err(Escalation::FAIL_FAST));
        attempts.succeeded();
        assert_eq!(attempts.failed(stall, now), Ok(now));
    }

    #[test]
    fn attempts_retry_only_their_own_convergence() {
        let now = Instant::now();
        let restart = Escalation::Restart;
        let mut attempts = Attempts::new(Converges::StartStall);
        for (recovery, escalation) in [
            (retry(Converges::DriverPreparing), Escalation::FAIL_FAST),
            (Recovery::Wait(Signal::Video), Escalation::FAIL_FAST),
            (Recovery::Escalate(restart), restart),
        ] {
            assert_eq!(attempts.failed(recovery, now), Err(escalation));
        }
        let stall = retry(Converges::StartStall);
        assert_eq!(attempts.failed(stall, now), Ok(now), "a refusal is no try");
    }

    #[test]
    fn above_the_owner_a_retry_or_wait_fails_fast() {
        let unsupported = Escalation::Ask(Fix::Unsupported);
        for (recovery, escalation) in [
            (retry(Converges::StartStall), Escalation::FAIL_FAST),
            (Recovery::Wait(Signal::Video), Escalation::FAIL_FAST),
            (Recovery::Escalate(Escalation::Restart), Escalation::Restart),
            (Recovery::Escalate(unsupported), unsupported),
        ] {
            assert_eq!(recovery.escalation(), escalation, "{recovery:?}");
        }
    }
}
