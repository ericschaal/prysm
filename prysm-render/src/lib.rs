#![no_main]

use futures::Stream;

pub struct TinyFrame;

pub trait PrysmRenderer {
    fn start(&mut self, input: impl Stream<Item = TinyFrame>);
}
