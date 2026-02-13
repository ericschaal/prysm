use prysm_core::EdgeSpectra;
use std::fmt::Debug;

/// `PostProcessor` trait for optional post-processing steps
///
/// Post-processors transform `EdgeSpectra` after algorithm processing.
/// They can be chained together to compose multiple transformations.
pub trait PostProcessor: Debug + Send {
    /// Process edge spectra
    ///
    /// # Arguments
    /// * `input` - `EdgeSpectra` from algorithm or previous post-processor
    ///
    /// # Returns
    /// Transformed `EdgeSpectra`
    fn process(&mut self, input: EdgeSpectra) -> EdgeSpectra;
}
