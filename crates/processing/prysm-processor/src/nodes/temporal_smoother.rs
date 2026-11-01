use prysm_core::{ColorStrip, EdgeColors, SmoothingSeconds};
use std::time::Instant;

/// Temporal smoothing node
///
/// Blends in linear light using elapsed time, independent of frame rate.
#[derive(Debug, Clone)]
pub struct TemporalSmoother {
    /// Seconds to complete 95% of a transition.
    seconds: SmoothingSeconds,
    previous: Option<(EdgeColors, Instant)>,
}

impl TemporalSmoother {
    /// The processor only constructs this node for finite, positive durations.
    pub fn new(seconds: SmoothingSeconds) -> Self {
        Self {
            seconds,
            previous: None,
        }
    }

    pub fn configure(&mut self, seconds: SmoothingSeconds) {
        self.seconds = seconds;
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "the exponential smoothing factor is in 0..=1"
    )]
    pub fn process(&mut self, input: EdgeColors, mut now: Instant) -> EdgeColors {
        let smoothed = if let Some((prev, previous_time)) = &mut self.previous {
            now = now.max(*previous_time);
            let elapsed = now.duration_since(*previous_time).as_secs_f64();
            // exp_m1 preserves small contributions when frames arrive close together.
            let ratio = -(-20.0_f64.ln() * elapsed / f64::from(self.seconds.0)).exp_m1();
            // Preserve LED positions across crop changes while adopting the new sample counts.
            for (old, new) in [
                (&mut prev.top, &input.top),
                (&mut prev.right, &input.right),
                (&mut prev.bottom, &input.bottom),
                (&mut prev.left, &input.left),
            ] {
                if old.len() != new.len() {
                    *old =
                        ColorStrip::new(old.resample(new.len())).expect("sample count is nonzero");
                }
            }
            prev.blend(&input, ratio as f32)
        } else {
            input
        };

        self.previous = Some((smoothed.clone(), now));
        smoothed
    }
}

impl Default for TemporalSmoother {
    fn default() -> Self {
        Self::new(prysm_core::Config::default().smoothing_seconds)
    }
}
