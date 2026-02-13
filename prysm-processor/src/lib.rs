use futures::{Stream, StreamExt};
use prysm_capture::Frame;
use prysm_core::{Config, EdgeSpectra};

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
/// (temporal smoothing, brightness, etc.) to generate edge color spectra.
///
/// Configuration is baked in at construction time via `Config`, making
/// the processor stateful but eliminating per-frame config overhead.
///
/// # Example
/// ```ignore
/// use prysm_core::Config;
///
/// // Default behavior (edge sampling + temporal smoothing)
/// let processor = PrysmProcessor::default();
///
/// // Custom configuration
/// let mut config = Config::default();
/// config.sample_step = 2;
/// config.temporal_smoothing = 0.8;
/// let processor = PrysmProcessor::new(&config);
///
/// // Or customize algorithm/post-processor directly
/// let processor = PrysmProcessor::new(&config)
///     .with_algorithm(EdgeSamplingAlgorithm::new(4, 50, 100))
///     .with_post_processor(TemporalSmoothingProcessor::new(0.7));
/// ```
#[derive(Debug)]
pub struct PrysmProcessor {
    algorithm: Box<dyn Algorithm>,
    post_processor: Option<Box<dyn PostProcessor>>,
}

impl PrysmProcessor {
    /// Create a new processor with default edge sampling and temporal smoothing post processing
    #[must_use]
    pub fn new(config: &Config) -> Self {
        Self {
            algorithm: Box::new(EdgeSamplingAlgorithm::new(
                config.sample_step,
                config.sample_density,
                config.edge_depth_px,
            )),
            post_processor: Some(Box::new(TemporalSmoothingProcessor::new(
                config.temporal_smoothing,
            ))),
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
    /// Uses the algorithm and post-processor configured during construction.
    ///
    /// # Arguments
    /// * `frame` - Frame to process
    ///
    /// # Returns
    /// `EdgeSpectra` for the frame
    pub fn process_frame(&mut self, frame: &Frame) -> EdgeSpectra {
        let spectra = self.algorithm.process(frame);

        if let Some(ref mut post_processor) = self.post_processor {
            post_processor.process(spectra)
        } else {
            spectra
        }
    }

    /// Convert into a stream processor
    ///
    /// Consumes the processor and transforms a frame stream into an edge spectrum stream.
    /// Uses the algorithm and post-processor configured during construction.
    ///
    /// # Arguments
    /// * `input` - Input frame stream
    ///
    /// # Returns
    /// Stream of `EdgeSpectra`
    pub fn into_stream(
        mut self,
        input: impl Stream<Item = Frame> + Send + 'static,
    ) -> impl Stream<Item = EdgeSpectra> + Send + 'static {
        input.map(move |frame| self.process_frame(&frame))
    }
}

impl Default for PrysmProcessor {
    fn default() -> Self {
        Self::new(&Config::default())
    }
}
