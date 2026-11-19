//! `AVFoundation` states rates as floating-point ranges, any rate in which
//! is set exactly; a range often holds one rate only.
use super::{MAX_FPS, Rates, TOLERANCE_PER_MILLE};

/// The widest a rate may be stretched to meet a whole number.
const STRETCH: f64 = TOLERANCE_PER_MILLE as f64 / 1000.0;

/// Where a whole-number rate sits in a range of rates a device states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fit {
    /// Inside the range: the device sets it exactly.
    Inside,
    /// Just under the range: its lowest rate is this rate.
    Lowest,
    /// Just over the range: its highest rate is this rate.
    Highest,
}

/// How `min..=max` frames per second delivers `fps`, if it does.
pub(crate) fn fit(min: f64, max: f64, fps: u32) -> Option<Fit> {
    let fps = f64::from(fps);
    if min <= fps && fps <= max {
        Some(Fit::Inside)
    } else if fps < min && min <= fps * (1.0 + STRETCH) {
        Some(Fit::Lowest)
    } else if max < fps && fps * (1.0 - STRETCH) <= max {
        Some(Fit::Highest)
    } else {
        None
    }
}

impl Rates {
    /// The whole numbers `min..=max` frames per second delivers.
    pub(crate) fn within(&mut self, min: f64, max: f64) {
        if min.is_nan() || max.is_nan() {
            return;
        }
        let low = (min / (1.0 + STRETCH)).ceil().max(1.0);
        let high = (max / (1.0 - STRETCH)).floor().min(f64::from(MAX_FPS));
        if low <= high {
            // Both are whole numbers from 1 to MAX_FPS here.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            self.0.extend(low as u32..=high as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{span, spans};
    use super::*;
    use prysm_ports::Span;

    #[test]
    fn floating_ranges_give_whole_rates_within_them() {
        assert_eq!(spans(|r| r.within(1.0, 30.0)), [span(1, 30)]);
        assert_eq!(spans(|r| r.within(29.5, 29.5)), []);
        assert_eq!(spans(|r| r.within(0.5, 2.5)), [span(1, 2)]);
        assert_eq!(
            spans(|r| {
                r.within(15.0, 15.0);
                r.within(25.0, 30.0);
            }),
            [Span::exactly(15), span(25, 30)]
        );
        assert_eq!(spans(|r| r.within(f64::NAN, 30.0)), []);
        assert_eq!(spans(|r| r.within(1.0, 1e9)), [span(1, MAX_FPS)]);
    }

    #[test]
    fn rates_a_hair_off_a_whole_number_are_that_number() {
        // A USB card's 100 ns intervals, and broadcast rates.
        for (stated, whole) in [
            (30.000_030_000_03, 30),
            (60.000_240_000_96, 60),
            (29.97, 30),
            (59.94, 60),
        ] {
            assert_eq!(spans(|r| r.within(stated, stated)), [Span::exactly(whole)]);
        }
        assert_eq!(fit(30.000_03, 30.000_03, 30), Some(Fit::Lowest));
        assert_eq!(fit(29.97, 29.97, 30), Some(Fit::Highest));
        assert_eq!(fit(1.0, 30.0, 15), Some(Fit::Inside));
        assert_eq!(fit(25.0, 25.0, 30), None);
        assert_eq!(fit(1.0, 30.0, 31), None);
    }

    #[test]
    fn listing_and_fitting_agree() {
        for (min, max) in [
            (29.97, 29.97),
            (30.000_03, 60.000_24),
            (0.5, 2.5),
            (14.98, 15.0),
        ] {
            let mut rates = Rates::default();
            rates.within(min, max);
            for fps in 1..=100 {
                assert_eq!(
                    rates.0.contains(&fps),
                    fit(min, max, fps).is_some(),
                    "{fps} in {min}..={max}"
                );
            }
        }
    }
}
