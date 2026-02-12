#![no_main]

use std::collections::HashMap;
use futures::Stream;

use prysm_core::{Color, Zone};

pub trait PrysmRenderer {
    fn start(&mut self, input: impl Stream<Item = HashMap<Zone, Color>>);
}
