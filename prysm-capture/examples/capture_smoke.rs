use anyhow::Context;
use futures::{Stream, StreamExt};
use prysm_capture::{Capturer, Frame, PrysmCapturer};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    let shutdown_token = CancellationToken::new();
    let capturer = Capturer::new(None, shutdown_token.clone())?;
    let stream = capturer.into_stream(800, 600);

    let result = check_capture(stream).await;
    shutdown_token.cancel();
    result
}

async fn check_capture(mut stream: impl Stream<Item = Frame> + Unpin) -> anyhow::Result<()> {
    for i in 0..3 {
        let frame = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .context("Timed out waiting for a capture frame")?
            .context("Capture stream ended before three frames arrived")?;
        println!(
            "frame {i}: {}x{} {} ({} bytes)",
            frame.width,
            frame.height,
            frame.format,
            frame.len()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn three_frames_succeed() {
        let frames = futures::stream::iter(vec![Frame::dummy(2, 1); 3]);
        assert!(check_capture(frames).await.is_ok());
    }

    #[tokio::test]
    async fn early_end_is_an_error() {
        for count in 0..3 {
            let frames = futures::stream::iter(vec![Frame::dummy(2, 1); count]);
            assert!(
                check_capture(frames).await.is_err(),
                "only {count} frames arrived"
            );
        }
    }

    #[tokio::test]
    async fn stalled_capture_is_an_error() {
        let result = tokio::time::timeout(
            Duration::from_secs(6),
            check_capture(futures::stream::pending()),
        )
        .await
        .expect("the smoke check must stop waiting on its own");
        assert!(result.unwrap_err().to_string().contains("Timed out"));
    }
}
