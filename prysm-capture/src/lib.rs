#![no_main]

use futures::Stream;

#[derive(Debug, Clone)]
pub struct Frame(pub Vec<u8>);


#[derive(Debug, Clone)]
pub struct Info {
    pub width: u32,
    pub height: u32,
}

pub enum CaptureMessage {
    Frame(Frame),
    Info(Info)
}

pub trait PrysmCapturer {
    fn start(&mut self, width: u32, height: u32) -> impl Stream<Item = CaptureMessage> + '_;
}
