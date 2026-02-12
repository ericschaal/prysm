use std::collections::HashMap;
use futures::{Stream, StreamExt};
use prysm_capture::{Frame};
use prysm_core::{Color, Config, Zone};
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
    ) -> impl Stream<Item = HashMap<Zone, Color>> + 'a {
        input.map(|frame| self.color_processor.process_frame(&frame))
    }


}