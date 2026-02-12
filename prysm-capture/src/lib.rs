#![no_main]

use futures::Stream;

#[derive(Debug, Clone)]
pub struct Frame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}


pub trait PrysmCapturer {
    fn run(&mut self, width: u32, height: u32) -> impl Stream<Item = Frame> + '_;
}
