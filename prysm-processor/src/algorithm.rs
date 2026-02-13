use prysm_capture::Frame;
use prysm_core::{Config, EdgeSpectrums};
use std::fmt::Debug;

/// Algorithm trait defines frame processing strategies
///
/// Implementations transform raw video frames into edge color spectrums
/// using different analysis techniques (edge sampling, edge detection, etc.)
pub trait Algorithm: Debug + Send {
    /// Process a frame and extract edge spectrums
    ///
    /// # Arguments
    /// * `frame` - Frame with Arc-wrapped data (zero-copy)
    /// * `config` - Processing configuration
    ///
    /// # Returns
    /// `EdgeSpectrums` with color gradients for all four edges
    fn process(&self, frame: &Frame, config: &Config) -> EdgeSpectrums;
}
