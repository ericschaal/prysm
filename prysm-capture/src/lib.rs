#![no_main]

use futures::Stream;
use prysm_core::Frame;

pub trait VideoCaptureBackend {
    fn start(&mut self) -> impl Stream<Item = Frame> + '_;
}
