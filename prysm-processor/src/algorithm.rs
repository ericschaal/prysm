use prysm_capture::Frame;
use prysm_core::EdgeSpectra;
use std::fmt::Debug;

/// Algorithm trait defines frame processing strategies
///
/// Implementations transform raw video frames into edge color spectra
/// using different analysis techniques (edge sampling, edge detection, etc.).
///
pub trait Algorithm: Debug + Send {
    /// Process a frame and extract edge spectra
    ///
    /// # Arguments
    /// * `frame` - Frame with Arc-wrapped data (zero-copy)
    ///
    /// # Returns
    /// `EdgeSpectra` with color gradients for all four edges
    fn process(&self, frame: &Frame) -> EdgeSpectra;
}
