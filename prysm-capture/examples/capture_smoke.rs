use futures::StreamExt;
use prysm_capture::{Capturer, PrysmCapturer};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    let shutdown_token = CancellationToken::new();
    let capturer = Capturer::new(None, shutdown_token.clone())?;
    let mut stream = capturer.into_stream(800, 600);

    for i in 0..3 {
        match stream.next().await {
            Some(frame) => println!(
                "frame {i}: {}x{} {} ({} bytes)",
                frame.width,
                frame.height,
                frame.format,
                frame.len()
            ),
            None => {
                println!("stream ended early");
                break;
            }
        }
    }

    shutdown_token.cancel();
    Ok(())
}
