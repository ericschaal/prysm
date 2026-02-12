#![no_main]

use std::collections::HashMap;
use futures::Stream;
use prysm_render::{Color, PrysmRenderer, Zone};

pub struct DesktopRenderer {

}

impl DesktopRenderer {
    pub fn new() -> Self {Self {}}
}

impl PrysmRenderer for DesktopRenderer {
    fn run(&mut self, input: impl Stream<Item=HashMap<Zone, Color>>) {
        todo!()
    }
}