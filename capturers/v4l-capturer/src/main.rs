use crate::capture::VideoCapture;
use anyhow::Result;
use futures::{StreamExt, pin_mut};
use prysm_capture::VideoCaptureBackend;

mod capture;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    let mut capturer = VideoCapture::new("/dev/video0", 100, 100)?;
    let stream = capturer.start();

    pin_mut!(stream); // needed for iteration

    while let Some(value) = stream.next().await {
        println!("Got {value:?}");
    }

    Ok(())
}
