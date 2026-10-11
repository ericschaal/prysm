use anyhow::Result;
use futures::StreamExt;
use prysm_capture::{Capturer, PrysmCapturer};

fn main() -> Result<()> {
    prysm::run(|shutdown| {
        let capturer = Capturer::new(None, shutdown)?;
        Ok(capturer
            .into_stream(prysm::CAPTURE_WIDTH, prysm::CAPTURE_HEIGHT)
            .map(|frame| frame.map_err(anyhow::Error::from)))
    })
}
