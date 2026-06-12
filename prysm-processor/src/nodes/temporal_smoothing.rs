use crate::pipeline::Node;
use prysm_core::EdgeSpectra;

/// Temporal smoothing node
///
/// Blends current frame with previous frame to reduce color flickering
/// and create smoother color transitions over time.
#[derive(Debug, Clone)]
pub struct TemporalSmoothing {
    /// Smoothing factor (0.0 = no smoothing, 1.0 = maximum smoothing)
    smoothing: f32,
    /// Previous frame's spectra for blending
    previous_spectra: Option<EdgeSpectra>,
}

impl TemporalSmoothing {
    /// Create new temporal smoothing node
    ///
    /// # Arguments
    /// * `smoothing` - Smoothing factor (0.0 to 1.0)
    ///   - 0.0 = no smoothing (current frame only)
    ///   - 1.0 = maximum smoothing (heavy bias toward previous frames)
    ///   - 0.7 = recommended default
    pub fn new(smoothing: f32) -> Self {
        Self {
            smoothing: smoothing.clamp(0.0, 1.0),
            previous_spectra: None,
        }
    }
}

impl Node<EdgeSpectra, EdgeSpectra> for TemporalSmoothing {
    fn process(&mut self, input: EdgeSpectra) -> EdgeSpectra {
        // Apply temporal smoothing by blending with previous frame
        let smoothed = if let Some(ref prev) = self.previous_spectra {
            // Blend: ratio=1.0-smoothing means higher smoothing gives more weight to previous
            prev.blend(&input, 1.0 - self.smoothing)
        } else {
            // First frame - no previous data to blend with
            input.clone()
        };

        // Store current smoothed result for next frame
        self.previous_spectra = Some(smoothed.clone());
        smoothed
    }
}

impl Default for TemporalSmoothing {
    fn default() -> Self {
        Self::new(0.7)
    }
}
