use prysm_core::EdgeSpectra;
use std::time::Instant;

/// Temporal smoothing node
///
/// Blends in linear light using elapsed time, independent of frame rate.
#[derive(Debug, Clone)]
pub struct TemporalSmoothing {
    /// Seconds to complete 95% of a transition.
    seconds: f32,
    previous: Option<(EdgeSpectra, Instant)>,
}

impl TemporalSmoothing {
    /// The processor only constructs this node for finite, positive durations.
    pub fn new(seconds: f32) -> Self {
        Self {
            seconds,
            previous: None,
        }
    }

    pub fn process(&mut self, input: EdgeSpectra, mut now: Instant) -> EdgeSpectra {
        let smoothed = if let Some((prev, previous_time)) = &self.previous {
            now = now.max(*previous_time);
            let elapsed = now.duration_since(*previous_time).as_secs_f64();
            // exp_m1 preserves small contributions when frames arrive close together.
            let ratio = -(-20.0_f64.ln() * elapsed / f64::from(self.seconds)).exp_m1();
            prev.blend(&input, ratio as f32)
        } else {
            input
        };

        self.previous = Some((smoothed.clone(), now));
        smoothed
    }
}

impl Default for TemporalSmoothing {
    fn default() -> Self {
        Self::new(prysm_core::Config::default().smoothing_seconds)
    }
}
