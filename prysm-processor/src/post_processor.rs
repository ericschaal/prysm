use prysm_core::EdgeSpectrums;
use std::fmt::Debug;

/// `PostProcessor` trait for optional post-processing steps
///
/// Post-processors transform `EdgeSpectrums` after algorithm processing.
/// They can be chained together to compose multiple transformations.
pub trait PostProcessor: Debug + Send {
    /// Process edge spectrums
    ///
    /// # Arguments
    /// * `input` - `EdgeSpectrums` from algorithm or previous post-processor
    ///
    /// # Returns
    /// Transformed `EdgeSpectrums`
    fn process(&mut self, input: EdgeSpectrums) -> EdgeSpectrums;
}
