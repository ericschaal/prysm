//! V4L2 states rates as frame intervals in seconds: discrete fractions, or a
//! stepwise or continuous range of them, per size. Capture sets 1/fps and
//! requires the driver to answer with that rate.
use super::{MAX_FPS, Rates, TOLERANCE_PER_MILLE};

/// Whether a frame interval of `numerator / denominator` seconds is `fps`
/// frames per second.
pub(crate) fn interval_is(numerator: u32, denominator: u32, fps: u32) -> bool {
    // |1/interval - fps| <= fps * tolerance, without dividing; in `u128`,
    // which no product of `u32`s and 1000 overflows.
    let stated = u128::from(denominator) * 1000;
    let whole = u128::from(numerator) * u128::from(fps) * 1000;
    let slack = u128::from(numerator) * u128::from(fps) * u128::from(TOLERANCE_PER_MILLE);
    numerator != 0 && stated.abs_diff(whole) <= slack
}

impl Rates {
    /// A frame interval of `numerator / denominator` seconds, when it is a
    /// whole number of frames per second.
    pub(crate) fn interval(&mut self, numerator: u32, denominator: u32) {
        if numerator == 0 {
            return;
        }
        let nearest = (u64::from(denominator) + u64::from(numerator) / 2) / u64::from(numerator);
        if let Ok(fps) = u32::try_from(nearest)
            && (1..=MAX_FPS).contains(&fps)
            && interval_is(numerator, denominator, fps)
        {
            self.0.insert(fps);
        }
    }

    /// The whole-number rates whose interval 1/fps lies in `min..=max`
    /// seconds (fractions as numerator and denominator) and, unless the
    /// range is continuous (`step` `None`), is `min` plus whole steps.
    pub(crate) fn intervals(&mut self, min: (u32, u32), max: (u32, u32), step: Option<(u32, u32)>) {
        let (a, b) = (u128::from(min.0), u128::from(min.1));
        let (c, d) = (u128::from(max.0), u128::from(max.1));
        if b == 0 || d == 0 || step.is_some_and(|(e, f)| e == 0 || f == 0) {
            return;
        }
        self.0.extend((1..=MAX_FPS).filter(|&fps| {
            let n = u128::from(fps);
            // a/b <= 1/n <= c/d
            let inside = a * n <= b && d <= c * n;
            inside
                && step.is_none_or(|(e, f)| {
                    // (1/n - a/b) / (e/f) = f(b - an) / (bne) is whole.
                    let (e, f) = (u128::from(e), u128::from(f));
                    (f * (b - a * n)).is_multiple_of(b * n * e)
                })
        }));
    }

    /// The rates in both: those at both ends of a range of sizes.
    #[must_use]
    pub(crate) fn and(&self, other: &Self) -> Self {
        Self(self.0.intersection(&other.0).copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{span, spans};
    use super::*;
    use prysm_ports::Span;

    #[test]
    fn intervals_give_whole_rates() {
        assert_eq!(
            spans(|r| {
                for (n, d) in [(1, 30), (1, 25), (2, 59), (2, 1), (0, 0), (2, 60)] {
                    r.interval(n, d);
                }
            }),
            [Span::exactly(25), Span::exactly(30)]
        );
        assert_eq!(
            spans(|r| {
                for (n, d) in [(333_333, 10_000_000), (166_666, 10_000_000), (1001, 24_000)] {
                    r.interval(n, d);
                }
            }),
            [Span::exactly(24), Span::exactly(30), Span::exactly(60)]
        );
        assert!(interval_is(333_333, 10_000_000, 30));
        assert!(interval_is(1, 30, 30));
        assert!(!interval_is(1, 25, 30));
        assert!(!interval_is(0, 0, 30));
        assert!(interval_is(u32::MAX, u32::MAX, 1));
        assert!(!interval_is(u32::MAX, 1, u32::MAX));
        assert!(!interval_is(u32::MAX, u32::MAX, u32::MAX));
        assert_eq!(spans(|r| r.intervals((1, 60), (1, 1), None)), [span(1, 60)]);
        assert_eq!(
            spans(|r| r.intervals((1, 30), (1, 10), Some((1, 60)))),
            [
                Span::exactly(10),
                Span::exactly(12),
                Span::exactly(15),
                Span::exactly(20),
                Span::exactly(30)
            ]
        );
        assert_eq!(spans(|r| r.intervals((1, 0), (1, 1), None)), []);
        assert_eq!(spans(|r| r.intervals((1, 30), (1, 1), Some((0, 1)))), []);
        assert_eq!(
            spans(|r| r.intervals((0, 1), (1, 1), None)),
            [span(1, MAX_FPS)]
        );
    }

    #[test]
    fn rates_intersect() {
        let mut low = Rates::default();
        low.intervals((1, 60), (1, 1), None);
        let mut high = Rates::default();
        high.intervals((1, 30), (1, 1), None);
        high.interval(1, 50);
        high.interval(1, 70);
        assert_eq!(low.and(&high).spans(), [span(1, 30), Span::exactly(50)]);
    }
}
