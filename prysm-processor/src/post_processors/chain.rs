use crate::post_processor::PostProcessor;
use prysm_core::EdgeSpectra;

/// Chained post-processor for composing multiple post-processors
///
/// Applies a sequence of post-processors in order, passing the output
/// of each processor as input to the next.
///
/// # Example
/// ```ignore
/// let chain = ChainedPostProcessor::new(vec![
///     Box::new(TemporalSmoothingProcessor::new(0.7)),
///     Box::new(BrightnessProcessor::new(0.8)),
/// ]);
/// ```
#[derive(Debug, Default)]
pub struct ChainedPostProcessor {
    processors: Vec<Box<dyn PostProcessor>>,
}

impl ChainedPostProcessor {
    /// Create a new chained post-processor
    pub fn new(processors: Vec<Box<dyn PostProcessor>>) -> Self {
        Self { processors }
    }

    /// Add a post-processor to the chain
    pub fn add(&mut self, processor: Box<dyn PostProcessor>) {
        self.processors.push(processor);
    }
}

impl PostProcessor for ChainedPostProcessor {
    fn process(&mut self, mut input: EdgeSpectra) -> EdgeSpectra {
        for processor in &mut self.processors {
            input = processor.process(input);
        }
        input
    }
}
