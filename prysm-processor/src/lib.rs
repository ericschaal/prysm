use std::marker::PhantomData;
use async_stream::stream;
use futures::{pin_mut, Stream, StreamExt};
use prysm_capture::CaptureMessage;
use prysm_render::TinyFrame;

#[derive(Debug, Default)]
pub struct PrysmProcessor {
}


impl PrysmProcessor {
    #[must_use]
    pub fn new() -> Self {
        Self {
        }
    }

    pub async fn run<'a>(&mut self, input: impl Stream<Item = CaptureMessage> + 'a) -> impl Stream<Item = TinyFrame> + 'a {
        pin_mut!(input);

        stream! {
            while let Some(message) = input.next().await {
                yield TinyFrame;
            }
        }
    }
}