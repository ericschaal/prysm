use anyhow::Result;
use futures::{pin_mut};
use prysm_capture::PrysmCapturer;
use v4l_capturer::V4lCapturer;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    let mut capturer = V4lCapturer::new("/dev/video0")?;
    let stream = capturer.start(1920, 1080);

    pin_mut!(stream);

    // while let Some(frame) = stream.next().await {
    //     info!("got frame {}x{}", frame.width, frame.height)
    // }

    Ok(())
}
