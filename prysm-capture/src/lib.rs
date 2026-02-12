#![no_main]
#![no_std]

use futures::Stream;
use prysm_core::Frame;

pub trait VideoCaptureBackend {
    fn start(&mut self) -> dyn Stream<Item=Frame>;
}