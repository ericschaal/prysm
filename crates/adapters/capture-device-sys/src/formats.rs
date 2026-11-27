//! Whole-number frame rates a device delivers, as spans: the arithmetic
//! behind [`list_formats`](crate::list_formats) and behind opening a device at
//! a rate, platform-free so it is tested on any OS. Each platform's way of
//! stating rates is its own module.
//!
//! A device's rate counts as a whole number when it lies within
//! [`TOLERANCE_PER_MILLE`] of it. USB capture cards state rates as frame
//! intervals in steps of 100 ns, so their 30 and 60 frames per second are
//! 30.00003 and 60.00024; broadcast sources state 29.97 and 59.94.
use prysm_ports::Span;
use std::collections::BTreeSet;

#[cfg(any(target_os = "macos", test))]
pub(crate) mod avfoundation;
#[cfg(any(target_os = "linux", test))]
pub(crate) mod v4l2;

/// How far, in thousandths, a device's rate may lie from a whole number of
/// frames per second and still be that rate.
const TOLERANCE_PER_MILLE: u32 = 2;

/// Maximum frame rate included when listing capture modes.
const MAX_FPS: u32 = 1000;

/// Whole-number frame rates a device delivers, 1 to [`MAX_FPS`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Rates(BTreeSet<u32>);

impl Rates {
    /// Runs of consecutive rates, each as one span.
    pub(crate) fn spans(&self) -> Vec<Span> {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &fps in &self.0 {
            match runs.last_mut() {
                Some((_, last)) if *last + 1 == fps => *last = fps,
                _ => runs.push((fps, fps)),
            }
        }
        runs.into_iter()
            .filter_map(|(min, max)| Span::new(min, max, 1))
            .collect()
    }
}

#[cfg(test)]
fn spans(fill: impl FnOnce(&mut Rates)) -> Vec<Span> {
    let mut rates = Rates::default();
    fill(&mut rates);
    rates.spans()
}

#[cfg(test)]
fn span(min: u32, max: u32) -> Span {
    Span::new(min, max, 1).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_rates_are_one_span() {
        let mut rates = Rates::default();
        rates.0.extend([1, 2, 3, 15, 25, 26]);
        assert_eq!(rates.spans(), [span(1, 3), Span::exactly(15), span(25, 26)]);
        assert_eq!(Rates::default().spans(), []);
    }
}
