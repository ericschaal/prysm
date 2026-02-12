use futures::{Stream, StreamExt};
use prysm_capture::{Frame};
use prysm_core::{Config};
use prysm_render::{EdgeSpectrums};
use crate::color::ColorProcessor;

mod color;

#[derive(Debug, Default)]
pub struct PrysmProcessor {
    color_processor: ColorProcessor
}

impl PrysmProcessor {

    pub fn new(config: Config) -> Self {
        Self {
            color_processor: ColorProcessor::new(config),
        }
    }

    pub fn run<'a>(
        &'a mut self,
        input: impl Stream<Item = Frame> + 'a
    ) -> impl Stream<Item = EdgeSpectrums> + 'a {
        input.map(|frame| self.color_processor.process_frame(&frame))
    }


}