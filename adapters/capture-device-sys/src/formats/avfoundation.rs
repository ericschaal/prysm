//! `AVFoundation` states rates as floating-point ranges, any rate in which
//! is set exactly.
use super::{MAX_FPS, Rates};

impl Rates {
    /// The whole numbers within `min..=max` frames per second.
    pub(crate) fn within(&mut self, min: f64, max: f64) {
        if min.is_nan() || max.is_nan() {
            return;
        }
        let low = min.ceil().max(1.0);
        let high = max.floor().min(f64::from(MAX_FPS));
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
        assert_eq!(spans(|r| r.within(29.97, 29.97)), []);
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
}
