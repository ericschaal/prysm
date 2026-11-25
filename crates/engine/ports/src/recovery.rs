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
    /// confirm its release): whatever starts the process starts one, once.
    /// If that one faults again before it has worked, it is
    /// `Ask(Fix::TryAgain)`. A process that nothing restarts fails fast.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempts {
    schedule: Schedule,
    /// The first failure since the last success.
    since: Option<Instant>,
}

impl Attempts {
    #[must_use]
    pub const fn new(schedule: Schedule) -> Self {
        Self {
            schedule,
            since: None,
        }
    }

    /// After a failure at `now`: when to try again.
    ///
    /// # Errors
    /// [`Escalation::FAIL_FAST`] once the schedule gives up; a give-up is
    /// never another retry.
    pub fn failed(&mut self, now: Instant) -> Result<Instant, Escalation> {
        let first = self.since.is_none();
        let since = *self.since.get_or_insert(now);
        let retry = match self.schedule {
            Schedule::Within { every, limit } => {
                let deadline = since + limit;
                (now < deadline).then(|| (now + every).min(deadline))
            }
            Schedule::Once => first.then_some(now),
        };
        retry.ok_or(Escalation::FAIL_FAST)
    }

    /// The operation really worked, so the next failure starts the schedule
    /// over.
    pub fn succeeded(&mut self) {
        self.since = None;
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
                attempts.failed(now).ok().map(|at| at - now)
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
        assert_eq!(attempts.failed(late), Err(Escalation::FAIL_FAST));
        attempts.succeeded();
        assert_eq!(attempts.failed(late), Ok(late + Duration::from_secs(1)));
    }

    #[test]
    fn within_clamps_to_its_limit_and_schedules_nothing_after_it() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(10);
        let mut attempts = Attempts::new(WITHIN);
        attempts.failed(now).unwrap();
        let just_under = deadline - Duration::from_millis(1);
        assert_eq!(attempts.failed(just_under), Ok(deadline));
        assert_eq!(attempts.failed(deadline), Err(Escalation::FAIL_FAST));
    }

    #[test]
    fn within_keeps_its_first_failure_when_now_goes_backwards() {
        let now = Instant::now() + Duration::from_secs(60);
        let earlier = now - Duration::from_secs(5);
        let mut attempts = Attempts::new(WITHIN);
        attempts.failed(now).unwrap();
        assert_eq!(
            attempts.failed(earlier),
            Ok(earlier + Duration::from_secs(1))
        );
        let deadline = now + Duration::from_secs(10);
        assert_eq!(attempts.failed(deadline), Err(Escalation::FAIL_FAST));
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
        assert_eq!(attempts.failed(now), Err(Escalation::FAIL_FAST));
        attempts.succeeded();
        assert_eq!(attempts.failed(now), Ok(now));
    }
}
