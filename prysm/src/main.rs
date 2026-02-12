
use anyhow::Result;
use futures::{pin_mut, StreamExt};
use tracing::info;
use prysm_capture::PrysmCapturer;
use prysm_processor::PrysmProcessor;
use v4l_capturer::V4lCapturer;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    let mut capturer = V4lCapturer::new("/dev/video0")?;
    let video_feed = capturer.start(1920, 1080);

    let mut processor = PrysmProcessor::default();
    let stream =  processor.run(video_feed);

    pin_mut!(stream);

    while let Some(update) = stream.next().await {
        info!("Update:");
        for (zone, color) in &update {
            info!("{:?}: rgb({}, {}, {})", zone, color.r, color.g, color.b);
        }
    }



    Ok(())
}
