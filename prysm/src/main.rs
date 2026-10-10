use anyhow::Result;
use prysm_capture::{Capturer, PrysmCapturer};

fn main() -> Result<()> {
    prysm::run(|shutdown| {
        let capturer = Capturer::new(None, shutdown)?;
        Ok(capturer.into_stream(prysm::CAPTURE_WIDTH, prysm::CAPTURE_HEIGHT))
    })
}
