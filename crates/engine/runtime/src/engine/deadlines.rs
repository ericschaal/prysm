//! What the engine does without a command, on time: the engine loop waits
//! for the next of these as one timer.
use super::Core;
use tokio::time::Instant;

/// What is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Due {
    /// The calibration pattern nobody set again is cleared.
    Calibration,
    /// The tried processing nobody tried again or saved is dropped.
    Trial,
}

/// When each [`Due`] is, read from the state that holds it: the calibration
/// expiry, the trial beside its processing, the wait in the lighting. A copy
/// for one turn of the loop, so the loop waits on it while it borrows the
/// lighting.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Deadlines {
    calibration: Option<Instant>,
    trial: Option<Instant>,
}

impl Deadlines {
    /// Wait for the earliest deadline and say what it is; on a tie, the
    /// first in [`Due`]'s order. Without one, never. Cancel-safe.
    pub(super) async fn next(self) -> Due {
        let earliest = [
            (self.calibration, Due::Calibration),
            (self.trial, Due::Trial),
        ]
        .into_iter()
        .filter_map(|(at, due)| Some((at?, due)))
        .min_by_key(|(at, _)| *at);
        match earliest {
            Some((at, due)) => {
                tokio::time::sleep_until(at).await;
                due
            }
            None => std::future::pending().await,
        }
    }
}

impl Core {
    /// The deadlines as they stand now.
    pub(super) fn deadlines(&self) -> Deadlines {
        Deadlines {
            calibration: self.calibration_expires,
            trial: self.trial.as_ref().map(|(_, at)| *at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The earliest is due first, at its time; a tie goes to the first in
    /// order; none never is. Seconds from each case's start, by
    /// calibration, trial and again.
    #[tokio::test(start_paused = true)]
    async fn the_earliest_deadline_is_due() {
        let cases = [
            ((Some(10), Some(5)), Due::Trial, 5),
            ((Some(3), Some(5)), Due::Calibration, 3),
            ((None, Some(2)), Due::Trial, 2),
            ((Some(4), Some(4)), Due::Calibration, 4),
        ];
        for ((calibration, trial), expected, seconds) in cases {
            let start = Instant::now();
            let at = |seconds: Option<u64>| seconds.map(|s| start + Duration::from_secs(s));
            let deadlines = Deadlines {
                calibration: at(calibration),
                trial: at(trial),
            };
            assert_eq!(deadlines.next().await, expected);
            assert_eq!(start.elapsed(), Duration::from_secs(seconds));
        }
        let none = tokio::time::timeout(Duration::from_secs(3600), Deadlines::default().next());
        assert!(none.await.is_err(), "no deadline is never due");
    }
}
