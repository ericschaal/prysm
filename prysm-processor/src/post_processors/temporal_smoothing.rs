use crate::post_processor::PostProcessor;
use prysm_core::EdgeSpectrums;

/// Temporal smoothing post-processor
///
/// Blends current frame with previous frame to reduce color flickering
/// and create smoother color transitions over time.
#[derive(Debug, Clone)]
pub struct TemporalSmoothingProcessor {
    /// Smoothing factor (0.0 = no smoothing, 1.0 = maximum smoothing)
    smoothing: f32,
    /// Previous frame's spectrums for blending
    previous_spectrums: Option<EdgeSpectrums>,
}

impl TemporalSmoothingProcessor {
    /// Create new temporal smoothing processor
    ///
    /// # Arguments
    /// * `smoothing` - Smoothing factor (0.0 to 1.0)
    ///   - 0.0 = no smoothing (current frame only)
    ///   - 1.0 = maximum smoothing (heavy bias toward previous frames)
    ///   - 0.7 = recommended default
    pub fn new(smoothing: f32) -> Self {
        Self {
            smoothing: smoothing.clamp(0.0, 1.0),
            previous_spectrums: None,
        }
    }
}

impl PostProcessor for TemporalSmoothingProcessor {
    fn process(&mut self, input: EdgeSpectrums) -> EdgeSpectrums {
        // Apply temporal smoothing by blending with previous frame
        let smoothed = if let Some(ref prev) = self.previous_spectrums {
            // Blend: ratio=1.0-smoothing means higher smoothing gives more weight to previous
            prev.blend(&input, 1.0 - self.smoothing)
        } else {
            // First frame - no previous data to blend with
            input.clone()
        };

        // Store current smoothed result for next frame
        self.previous_spectrums = Some(smoothed.clone());
        smoothed
    }
}

impl Default for TemporalSmoothingProcessor {
    fn default() -> Self {
        Self::new(0.7)
    }
}
