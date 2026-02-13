#![no_main]

use futures::{Stream, StreamExt};
use prysm_capture::Frame;
use prysm_core::{Config, EdgeSpectrums};

mod algorithm;
mod algorithms;
mod pixel_reader;
mod pixel_readers;
mod post_processor;
mod post_processors;

// Re-export public types
pub use algorithm::Algorithm;
pub use algorithms::EdgeSamplingAlgorithm;
pub use post_processor::PostProcessor;
pub use post_processors::{ChainedPostProcessor, TemporalSmoothingProcessor};

/// Main processor facade for frame-to-spectrum transformation
///
/// Combines an Algorithm (frame analysis) with optional PostProcessor(s)
/// (temporal smoothing, brightness, etc.) to generate edge color spectrums.
///
/// # Example
/// ```ignore
/// // Default behavior (edge sampling + temporal smoothing)
/// let processor = PrysmProcessor::default();
///
/// // Custom configuration
/// let processor = PrysmProcessor::new()
///     .with_algorithm(EdgeSamplingAlgorithm)
///     .with_post_processor(TemporalSmoothingProcessor::new(0.7));
/// ```
#[derive(Debug)]
pub struct PrysmProcessor {
    algorithm: Box<dyn Algorithm>,
    post_processor: Option<Box<dyn PostProcessor>>,
}

impl PrysmProcessor {
    /// Create a new processor with default algorithm and no post-processing
    #[must_use]
    pub fn new() -> Self {
        Self {
            algorithm: Box::new(EdgeSamplingAlgorithm),
            post_processor: None,
        }
    }

    /// Set the processing algorithm
    #[must_use]
    pub fn with_algorithm<A: Algorithm + 'static>(mut self, algorithm: A) -> Self {
        self.algorithm = Box::new(algorithm);
        self
    }

    /// Set the post-processor
    #[must_use]
    pub fn with_post_processor<P: PostProcessor + 'static>(mut self, processor: P) -> Self {
        self.post_processor = Some(Box::new(processor));
        self
    }

    /// Disable post-processing
    #[must_use]
    pub fn without_post_processing(mut self) -> Self {
        self.post_processor = None;
        self
    }

    /// Process a single frame
    ///
    /// # Arguments
    /// * `frame` - Frame to process
    /// * `config` - Processing configuration
    ///
    /// # Returns
    /// `EdgeSpectrums` for the frame
    pub fn process_frame(&mut self, frame: &Frame, config: &Config) -> EdgeSpectrums {
        let spectrums = self.algorithm.process(frame, config);

        if let Some(ref mut post_processor) = self.post_processor {
            post_processor.process(spectrums)
        } else {
            spectrums
        }
    }

    /// Convert into a stream processor
    ///
    /// # Arguments
    /// * `config` - Processing configuration
    /// * `input` - Input frame stream
    ///
    /// # Returns
    /// Stream of `EdgeSpectrums`
    pub fn into_stream(
        mut self,
        config: Config,
        input: impl Stream<Item = Frame> + Send + 'static,
    ) -> impl Stream<Item = EdgeSpectrums> + Send + 'static {
        input.map(move |frame| self.process_frame(&frame, &config))
    }
}

impl Default for PrysmProcessor {
    fn default() -> Self {
        Self::new()
            .with_algorithm(EdgeSamplingAlgorithm)
            .with_post_processor(TemporalSmoothingProcessor::default())
    }
}
