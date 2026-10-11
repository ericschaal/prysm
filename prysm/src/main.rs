use anyhow::Result;
use futures::StreamExt;
use prysm_capture::{Capturer, PrysmCapturer};

fn main() -> Result<()> {
    prysm::run(|shutdown| {
        let capturer = Capturer::new(Some("0x2200000345f2131"), shutdown)?;
        Ok(capturer
            .into_stream(prysm::CAPTURE_WIDTH, prysm::CAPTURE_HEIGHT)
            .map(|frame| frame.map_err(anyhow::Error::from)))
    })
}
