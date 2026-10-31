//! `FFmpeg` file input shared by the egui and WLED outputs.

use std::path::Path;
use std::process::Stdio;
use std::{io, num::NonZeroU32};

use anyhow::{Context, Result};
use futures::{Stream, StreamExt};
use prysm_capture::{Frame, PixelFormat};
use prysm_core::CaptureResolution;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// Decode a video once at up to the requested rate into sRGB frames at the requested resolution.
///
/// Requires `FFmpeg` 9+ on PATH. Successful EOF closes the stream; decoding
/// failures are yielded with their original error context. Cancellation stops playback.
pub fn video_feed(
    path: &Path,
    resolution: CaptureResolution,
    refresh_rate_hz: NonZeroU32,
    shutdown: CancellationToken,
) -> Result<impl Stream<Item = Result<Frame>> + Send + 'static + use<>> {
    let [width, height] = resolution.dimensions();
    std::fs::File::open(path).with_context(|| format!("Cannot open video {}", path.display()))?;
    // FFmpeg 9+ maps HDR to the sRGB colors expected by the processor and egui.
    let filter = format!(
        "fps='min(source_fps,{refresh_rate_hz})',scale={width}:{height}:flags=area:intent=perceptual:\
         out_primaries=bt709:out_transfer=srgb:out_range=full,realtime"
    );
    let mut child = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-xerror"])
        .arg("-i")
        .arg(path)
        .args(["-map", "0:v:0", "-vf", &filter])
        .args(["-fps_mode", "passthrough", "-pix_fmt", "rgb24"])
        // Pace decoded frames by PTS; rawvideo frame threading would buffer them again.
        .args(["-threads:v", "1"])
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

    let frames =
        futures::stream::try_unfold((child, stdout), move |(mut child, mut stdout)| async move {
            if let Some(frame) = read_frame(&mut stdout, resolution)
                .await
                .context("Failed to read FFmpeg frame")?
            {
                return Ok(Some((frame, (child, stdout))));
            }
            let status = child.wait().await.context("Failed to wait for FFmpeg")?;
            anyhow::ensure!(status.success(), "FFmpeg exited with {status}");
            tracing::info!("Video playback complete");
            Ok(None)
        });
    Ok(frames.take_until(shutdown.cancelled_owned()))
}

async fn read_frame(
    reader: &mut (impl AsyncRead + Unpin),
    resolution: CaptureResolution,
) -> io::Result<Option<Frame>> {
    let [width, height] = resolution.dimensions();
    let mut data = vec![0; resolution.rgb24_bytes()];
    if reader.read(&mut data[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut data[1..]).await?;
    Ok(Some(Frame::new(data, width, height, PixelFormat::RGB24)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_complete_frames_and_clean_eof() {
        let mut bytes = vec![42; 96];
        bytes.extend(vec![123; 96]);
        let mut reader = bytes.as_slice();
        for expected in [42, 123] {
            let frame = read_frame(&mut reader, CaptureResolution::new(8, 4).unwrap())
                .await
                .unwrap()
                .unwrap();
            assert_eq!((frame.width, frame.height), (8, 4));
            assert_eq!(frame.format, PixelFormat::RGB24);
            assert!(frame.as_slice().iter().all(|&byte| byte == expected));
        }
        assert!(
            read_frame(&mut reader, CaptureResolution::new(8, 4).unwrap())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn truncated_frame_is_an_error() {
        let bytes = vec![0; CaptureResolution::default().rgb24_bytes() - 1];
        let error = read_frame(&mut bytes.as_slice(), CaptureResolution::default())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg 9+ on PATH"]
    async fn file_stream_preserves_ordered_eof_and_decoder_failure() {
        let directory =
            std::env::temp_dir().join(format!("prysm-video-stream-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("three-frames.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=16x16:rate=10",
            ])
            .args(["-frames:v", "3", "-c:v", "ffv1"])
            .arg(&path)
            .status()
            .await
            .unwrap();
        assert!(status.success());
        let shutdown = CancellationToken::new();
        let stream = video_feed(
            &path,
            CaptureResolution::new(32, 18).unwrap(),
            30.try_into().unwrap(),
            shutdown.clone(),
        )
        .unwrap();
        futures::pin_mut!(stream);
        let mut count = 0;
        let mut first_frame_at = None;
        while let Some(frame) =
            tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
                .await
                .unwrap()
        {
            let frame = frame.unwrap();
            assert_eq!((frame.width, frame.height), (32, 18));
            assert_eq!(frame.len(), 32 * 18 * 3);
            first_frame_at.get_or_insert_with(std::time::Instant::now);
            count += 1;
        }
        assert_eq!(count, 3);
        let playback_span = first_frame_at.unwrap().elapsed();
        assert!(!shutdown.is_cancelled());
        let capped = video_feed(
            &path,
            CaptureResolution::new(32, 18).unwrap(),
            5.try_into().unwrap(),
            CancellationToken::new(),
        )
        .unwrap();
        futures::pin_mut!(capped);
        let mut capped_count = 0;
        while let Some(frame) =
            tokio::time::timeout(std::time::Duration::from_secs(10), capped.next())
                .await
                .unwrap()
        {
            frame.unwrap();
            capped_count += 1;
        }
        assert_eq!(capped_count, 2, "FFmpeg must cap the 10 Hz source at 5 Hz");
        let cancellation = CancellationToken::new();
        let playback = video_feed(
            &path,
            CaptureResolution::new(32, 18).unwrap(),
            30.try_into().unwrap(),
            cancellation.clone(),
        )
        .unwrap();
        futures::pin_mut!(playback);
        tokio::time::timeout(std::time::Duration::from_secs(10), playback.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        cancellation.cancel();
        assert!(playback.next().await.is_none());
        let invalid = directory.join("invalid.mkv");
        std::fs::write(&invalid, b"invalid video data").unwrap();
        let stream = video_feed(
            &invalid,
            CaptureResolution::new(32, 18).unwrap(),
            30.try_into().unwrap(),
            shutdown.clone(),
        )
        .unwrap();
        futures::pin_mut!(stream);
        let error = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("FFmpeg exited"));
        assert!(stream.next().await.is_none());
        assert!(!shutdown.is_cancelled());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(invalid).unwrap();
        std::fs::remove_dir(directory).unwrap();
        assert!(
            playback_span >= std::time::Duration::from_millis(150),
            "10 fps frames arrived in a burst instead of spanning 200 ms"
        );
    }
}
