//! `FFmpeg` video-file input shared by the desktop and WLED binaries.

use std::io;
use std::path::Path;
use std::process::Stdio;

use crate::{CAPTURE_HEIGHT, CAPTURE_WIDTH};
use anyhow::{Context, Result};
use futures::{Stream, StreamExt};
use prysm_capture::{Frame, PixelFormat};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// Local Philips HDR test clip used when no video path is supplied.
pub const DEFAULT_VIDEO: &str =
    "/Users/eschaal/Ambilight 4K HDR Test by Philips [e1QD2dBkESE].f701.mp4";
const FRAME_BYTES: usize = CAPTURE_WIDTH as usize * CAPTURE_HEIGHT as usize * 3;

/// Decode a video once at its source rate into 640×360 sRGB frames.
///
/// Requires `FFmpeg` 9+ on PATH. Successful EOF cancels `shutdown`; decoding
/// failures are logged and close the stream without cancelling it.
pub fn video_feed(
    path: &Path,
    shutdown: CancellationToken,
) -> Result<impl Stream<Item = Frame> + Send + 'static + use<>> {
    std::fs::File::open(path).with_context(|| format!("Cannot open video {}", path.display()))?;
    // FFmpeg 9+ maps HDR to the sRGB colors expected by the processor and preview.
    let filter = format!(
        "scale={CAPTURE_WIDTH}:{CAPTURE_HEIGHT}:flags=area:intent=perceptual:\
         out_primaries=bt709:out_transfer=srgb:out_range=full"
    );
    let mut child = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-xerror"])
        .args(["-re", "-readrate_initial_burst", "0", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-vf", &filter])
        .args(["-fps_mode", "passthrough", "-pix_fmt", "rgb24"])
        .args(["-f", "rawvideo", "pipe:1"])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Cannot start FFmpeg; install FFmpeg 9+ and make it available on PATH")?;
    let stdout = child
        .stdout
        .take()
        .context("FFmpeg stdout is unavailable")?;
    tracing::info!(path = %path.display(), "Playing video");

    let frames = futures::stream::try_unfold(
        (child, stdout, shutdown.clone()),
        |(mut child, mut stdout, shutdown)| async move {
            if let Some(frame) = read_frame(&mut stdout).await? {
                return Ok(Some((frame, (child, stdout, shutdown))));
            }
            let status = child.wait().await.context("Failed to wait for FFmpeg")?;
            anyhow::ensure!(status.success(), "FFmpeg exited with {status}");
            tracing::info!("Video playback complete");
            shutdown.cancel();
            Ok(None)
        },
    );
    Ok(frames.take_until(shutdown.cancelled_owned()).filter_map(
        |result: Result<Frame>| async move {
            result
                .inspect_err(|error| tracing::error!(%error, "Video playback failed"))
                .ok()
        },
    ))
}

async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Frame>> {
    let mut data = vec![0; FRAME_BYTES];
    if reader.read(&mut data[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut data[1..]).await?;
    Ok(Some(Frame::new(
        data,
        CAPTURE_WIDTH,
        CAPTURE_HEIGHT,
        PixelFormat::RGB24,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_complete_frames_and_clean_eof() {
        let mut bytes = vec![42; FRAME_BYTES];
        bytes.extend(vec![123; FRAME_BYTES]);
        let mut reader = bytes.as_slice();
        for expected in [42, 123] {
            let frame = read_frame(&mut reader).await.unwrap().unwrap();
            assert_eq!((frame.width, frame.height), (CAPTURE_WIDTH, CAPTURE_HEIGHT));
            assert_eq!(frame.format, PixelFormat::RGB24);
            assert!(frame.as_slice().iter().all(|&byte| byte == expected));
        }
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn truncated_frame_is_an_error() {
        let bytes = vec![0; FRAME_BYTES - 1];
        let error = read_frame(&mut bytes.as_slice()).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg 9+ and the local default video"]
    async fn plays_default_video_and_stops_on_cancellation() {
        let shutdown = CancellationToken::new();
        let feed = video_feed(Path::new(DEFAULT_VIDEO), shutdown.clone()).unwrap();
        futures::pin_mut!(feed);
        for _ in 0..3 {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(10), feed.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(frame.len(), FRAME_BYTES);
        }
        assert!(!shutdown.is_cancelled());
        shutdown.cancel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), feed.next())
                .await
                .unwrap()
                .is_none()
        );
    }
}
