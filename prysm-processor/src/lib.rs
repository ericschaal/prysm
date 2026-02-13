use crate::color::ColorProcessor;
use futures::{Stream, StreamExt};
use prysm_capture::Frame;
use prysm_core::{Config, EdgeSpectrums};

mod color;

#[derive(Debug, Default)]
pub struct PrysmProcessor {
    color_processor: ColorProcessor,
}

impl PrysmProcessor {
    pub fn new(config: Config) -> Self {
        Self {
            color_processor: ColorProcessor::new(config),
        }
    }

    pub fn into_stream(
        mut self,
        input: impl Stream<Item = Frame> + Send + 'static,
    ) -> impl Stream<Item = EdgeSpectrums> + Send + 'static {
        input.map(move |frame| self.color_processor.process_frame(&frame))
    }
}
