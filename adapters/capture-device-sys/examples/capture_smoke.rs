use anyhow::Context;
use futures::{Stream, StreamExt};
use prysm_capture_device_sys::{CaptureError, DeviceCapture, Frame};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    let capture = DeviceCapture::open(None, 800, 600, 30.try_into().unwrap())?;
    let mut worker = capture.worker;
    let frames = futures::stream::unfold(capture.frames, |mut frames| async {
        loop {
            frames.changed().await.ok()?;
            let frame = frames.borrow_and_update().clone();
            if let Some(frame) = frame {
                return Some((Ok(frame), frames));
            }
        }
    });
    let result = check_capture(frames).await;
    worker.cancel();
    if let Some(error) = worker.join().await?.failure {
        return Err(error.into());
    }
    result
}

async fn check_capture(
    stream: impl Stream<Item = Result<Frame, CaptureError>>,
) -> anyhow::Result<()> {
    futures::pin_mut!(stream);
    for i in 0..3 {
        let frame = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .context("Timed out waiting for a capture frame")?
            .context("Capture stream ended before three frames arrived")??;
        println!(
            "frame {i}: {}x{} {} ({} bytes)",
            frame.width(),
            frame.height(),
            frame.format(),
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
        let frames = futures::stream::iter((0..3).map(|_| Ok(Frame::dummy(2, 1).unwrap())));
        assert!(check_capture(frames).await.is_ok());
    }

    #[tokio::test]
    async fn early_end_is_an_error() {
        for count in 0..3 {
            let frames = futures::stream::iter((0..count).map(|_| Ok(Frame::dummy(2, 1).unwrap())));
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
