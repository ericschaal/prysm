#![cfg(target_os = "linux")]

//! Use a dedicated v4l2loopback device with no other readers or writers:
//! `sudo modprobe v4l2loopback video_nr=42 exclusive_caps=1`
//! `PRYSM_TEST_VIDEO_DEVICE=/dev/video42 cargo test -p prysm-capture --test linux_recovery -- --ignored`

use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use prysm_capture::{PrysmCapturer, V4lCapturer};
use std::io::Write;
use std::time::Duration;
use tokio::time::{interval, timeout};
use tokio_util::sync::CancellationToken;
use v4l::control::{Control, Value};
use v4l::video::Output;
use v4l::{Device, Format, FourCC};

#[tokio::test]
#[ignore = "requires a dedicated v4l2loopback device in PRYSM_TEST_VIDEO_DEVICE"]
async fn stalled_producer_recovers_and_capture_cancels() -> Result<()> {
    let path = std::env::var("PRYSM_TEST_VIDEO_DEVICE")
        .context("Set PRYSM_TEST_VIDEO_DEVICE to a dedicated v4l2loopback device")?;
    let mut producer = Device::with_path(&path)?;
    ensure!(
        producer.query_caps()?.driver == "v4l2 loopback",
        "Expected a v4l2loopback device"
    );
    for control in producer.query_controls()? {
        if matches!(control.name.as_str(), "sustain_framerate" | "timeout") {
            producer.set_control(Control {
                id: control.id,
                value: Value::Integer(0),
            })?;
        }
    }
    let format = producer.set_format(&Format::new(64, 48, FourCC::new(b"YUYV")))?;
    ensure!(
        format.width == 64 && format.height == 48 && format.fourcc == FourCC::new(b"YUYV"),
        "Loopback device must accept 64x48 YUYV"
    );
    producer.write_all(&[16, 128, 16, 128].repeat(64 * 48 / 2))?;

    let shutdown = CancellationToken::new();
    let _cancel_on_exit = shutdown.clone().drop_guard();
    let mut frames = V4lCapturer::new(Some(&path), shutdown.clone())?.into_stream(64, 48);
    for luma in [16, 235] {
        let pixels = [luma, 128, luma, 128].repeat(64 * 48 / 2);
        timeout(Duration::from_secs(5), async {
            let mut tick = interval(Duration::from_millis(30));
            loop {
                tokio::select! {
                    frame = frames.next() => {
                        let frame = frame.context("Capture closed before the producer's new frame arrived")?;
                        if frame.as_slice() == pixels { return Ok::<_, anyhow::Error>(()); }
                    }
                    _ = tick.tick() => producer.write_all(&pixels)?,
                }
            }
        }).await.context("Capture did not receive the producer's new frame")??;

        // Drain queued frames while the producer pauses through two one-second capture timeouts.
        let ended = timeout(Duration::from_millis(2200), async {
            while frames.next().await.is_some() {}
        })
        .await;
        ensure!(
            ended.is_err(),
            "Capture closed while the producer was paused"
        );
    }
    shutdown.cancel();
    timeout(Duration::from_secs(2), async {
        while frames.next().await.is_some() {}
    })
    .await
    .context("Cancellation did not close the stalled capture")?;
    Ok(())
}
