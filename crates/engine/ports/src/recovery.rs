//! What crosses a layer when a failure leaves it ([`Escalation`], carried by
//! [`crate::Error::escalation`]) and the domain-free retry mechanism
//! ([`Schedule`], [`Attempts`]).
//!
//! Each owner of an operation defines, next to it, its own closed set of
//! reasons to retry (each with why it converges and its [`Schedule`]) and of
//! signals to wait for; neither ever leaves that owner.
use std::time::{Duration, Instant};

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

/// The person's next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// Start again.
    TryAgain,
    /// Change the settings; a save that validates replaces them.
    ChangeSettings,
    /// Run the installation's setup again: it lacks a permission it needs.
    SetUp,
    /// Nothing makes this work on this device.
    Unsupported,
}

/// When an owner tries an operation again after it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// After `every`, never later than `limit` after the first failure.
    Within { every: Duration, limit: Duration },
    /// At once, one time.
    Once,
}

/// The tries of one operation on one [`Schedule`]: the only state a retry
/// keeps.
#[derive(Debug, Clone)]
pub struct Attempts {
    schedule: Schedule,
    /// The first failure since the last success.
    since: Option<Instant>,
    /// Failures since the last success.
    failures: u32,
}

impl Attempts {
    #[must_use]
    pub const fn new(schedule: Schedule) -> Self {
        Self {
            schedule,
            since: None,
            failures: 0,
        }
    }

    /// After a failure at `now`: when to try again, or `None` once the
    /// schedule gives up. A give-up is never another retry; what follows it
    /// (an [`Escalation`], or a wait for an event) is the owner's.
    pub fn failed(&mut self, now: Instant) -> Option<Instant> {
        self.failures = self.failures.saturating_add(1);
        let first = self.since.is_none();
        let since = *self.since.get_or_insert(now);
        match self.schedule {
            Schedule::Within { every, limit } => {
                let deadline = since + limit;
                (now < deadline).then(|| (now + every).min(deadline))
            }
            Schedule::Once => first.then_some(now),
        }
    }

    /// Failures since the last success, the latest included.
    #[must_use]
    pub const fn failures(&self) -> u32 {
        self.failures
    }

    /// The operation really worked, so the next failure starts the schedule
    /// over.
    pub fn succeeded(&mut self) {
        self.since = None;
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITHIN: Schedule = Schedule::Within {
        every: Duration::from_secs(1),
        limit: Duration::from_secs(10),
    };

    /// The waits of `failures` failures one `apart`, `None` for a give-up.
    fn waits(schedule: Schedule, failures: u32, apart: Duration) -> Vec<Option<Duration>> {
        let start = Instant::now();
        let mut attempts = Attempts::new(schedule);
        (0..failures)
            .map(|n| {
                let now = start + apart * n;
                attempts.failed(now).map(|at| at - now)
            })
            .collect()
    }

    const fn secs(seconds: u64) -> Option<Duration> {
        Some(Duration::from_secs(seconds))
    }

    #[test]
    fn within_gives_up_once_failures_last_its_limit() {
        let tries = waits(WITHIN, 12, Duration::from_secs(1));
        assert_eq!(tries[..10], [secs(1); 10]);
        assert_eq!(tries[10..], [None, None]);

        let now = Instant::now();
        let late = now + Duration::from_secs(10);
        let mut attempts = Attempts::new(WITHIN);
        attempts.failed(now).unwrap();
        assert_eq!(attempts.failed(late), None);
        attempts.succeeded();
        assert_eq!(attempts.failed(late), Some(late + Duration::from_secs(1)));
    }

    #[test]
    fn within_clamps_to_its_limit_and_schedules_nothing_after_it() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(10);
        let mut attempts = Attempts::new(WITHIN);
        attempts.failed(now).unwrap();
        let just_under = deadline - Duration::from_millis(1);
        assert_eq!(attempts.failed(just_under), Some(deadline));
        assert_eq!(attempts.failed(deadline), None);
    }

    #[test]
    fn within_keeps_its_first_failure_when_now_goes_backwards() {
        let now = Instant::now() + Duration::from_secs(60);
        let earlier = now - Duration::from_secs(5);
        let mut attempts = Attempts::new(WITHIN);
        attempts.failed(now).unwrap();
        assert_eq!(
            attempts.failed(earlier),
            Some(earlier + Duration::from_secs(1))
        );
        let deadline = now + Duration::from_secs(10);
        assert_eq!(attempts.failed(deadline), None);
    }

    #[test]
    fn failures_count_until_a_success() {
        let now = Instant::now();
        let mut attempts = Attempts::new(WITHIN);
        assert_eq!(attempts.failures(), 0);
        for failures in 1..=3 {
            attempts.failed(now);
            assert_eq!(attempts.failures(), failures);
        }
        attempts.failed(now + Duration::from_secs(10));
        assert_eq!(attempts.failures(), 4, "a give-up counts too");
        attempts.succeeded();
        assert_eq!(attempts.failures(), 0);
    }

    #[test]
    fn once_retries_at_once_one_time_and_starts_over_after_a_success() {
        assert_eq!(
            waits(Schedule::Once, 3, Duration::ZERO),
            [Some(Duration::ZERO), None, None]
        );
        let now = Instant::now();
        let mut attempts = Attempts::new(Schedule::Once);
        attempts.failed(now).unwrap();
        assert_eq!(attempts.failed(now), None);
        attempts.succeeded();
        assert_eq!(attempts.failed(now), Some(now));
    }
}
