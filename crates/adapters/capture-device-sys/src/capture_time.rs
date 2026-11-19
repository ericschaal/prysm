//! Capture times from native stamps: the stamp's age on its own clock becomes
//! an [`Instant`], unless the age is nonsense.
use std::time::{Duration, Instant};

/// A stamp further back than this is not a capture time.
const MAX_AGE: Duration = Duration::from_secs(1);

/// `now` less `age` seconds, unless the age is negative (a stamp in the
/// future), absurdly old or not a number.
pub(crate) fn since(now: Instant, age: f64) -> Option<Instant> {
    (0.0..=MAX_AGE.as_secs_f64())
        .contains(&age)
        .then(|| now.checked_sub(Duration::from_secs_f64(age)))
        .flatten()
}

/// V4L2's timestamp-type bits and the monotonic type (`videodev2.h`);
/// `linux.rs` checks them against the bindings.
#[cfg(any(target_os = "linux", test))]
pub(crate) const V4L2_TIMESTAMP_MASK: u32 = 0xe000;
#[cfg(any(target_os = "linux", test))]
pub(crate) const V4L2_TIMESTAMP_MONOTONIC: u32 = 0x2000;

/// A V4L2 buffer's capture time from its `flags` and `stamp` (seconds,
/// microseconds), given `CLOCK_MONOTONIC` read as `monotonic` (seconds,
/// nanoseconds) at `now`. Only a monotonic stamp is one.
#[cfg(any(target_os = "linux", test))]
#[expect(
    clippy::cast_precision_loss,
    reason = "the guard rejects ages beyond a second, far inside f64 precision"
)]
pub(crate) fn v4l2(
    flags: u32,
    stamp: (i64, i64),
    monotonic: (i64, i64),
    now: Instant,
) -> Option<Instant> {
    if flags & V4L2_TIMESTAMP_MASK != V4L2_TIMESTAMP_MONOTONIC {
        return None;
    }
    let age = monotonic.0.saturating_sub(stamp.0) as f64 + monotonic.1 as f64 * 1e-9
        - stamp.1 as f64 * 1e-6;
    since(now, age)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_future_old_and_invalid_ages() {
        let now = Instant::now();
        assert_eq!(since(now, 0.02), now.checked_sub(Duration::from_millis(20)));
        assert_eq!(since(now, 0.0), Some(now));
        for nonsense in [-0.001, 1.5, f64::NAN, f64::INFINITY] {
            assert_eq!(since(now, nonsense), None, "{nonsense}");
        }
    }

    #[test]
    fn v4l2_uses_only_monotonic_stamps() {
        let now = Instant::now();
        // Stamped at 99.990000 s, read at 100.005000 s: 15 ms ago.
        let at = |flags| v4l2(flags, (99, 990_000), (100, 5_000_000), now);
        let captured = at(V4L2_TIMESTAMP_MONOTONIC).unwrap();
        let age = now - captured;
        assert!(age.abs_diff(Duration::from_millis(15)) < Duration::from_micros(1));
        // Other bits do not matter; another timestamp type (unknown 0x0000,
        // copy 0x4000) is not a capture time.
        assert!(at(V4L2_TIMESTAMP_MONOTONIC | 0x0001).is_some());
        assert_eq!(at(0), None);
        assert_eq!(at(0x4000), None);
        // A stamp after the clock read, or ages ago, is nonsense.
        assert_eq!(
            v4l2(
                V4L2_TIMESTAMP_MONOTONIC,
                (100, 6_000),
                (100, 5_000_000),
                now
            ),
            None
        );
        assert_eq!(v4l2(V4L2_TIMESTAMP_MONOTONIC, (10, 0), (100, 0), now), None);
    }
}
