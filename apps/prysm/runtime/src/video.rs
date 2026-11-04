//! `FFmpeg` file input shared by WLED output and browser preview.

use std::path::Path;
use std::process::Stdio;
use std::{io, num::NonZeroU32};

use anyhow::{Context, Result};
use futures::{Stream, StreamExt};
use prysm_core::CaptureResolution;
use prysm_core::{Frame, FramePool, PixelFormat};
use std::{
    pin::Pin,
    task::{Context as TaskContext, Poll},
};
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
) -> Result<VideoFeed> {
    let [width, height] = resolution.dimensions();
    std::fs::File::open(path).with_context(|| format!("Cannot open video {}", path.display()))?;
    // FFmpeg 9+ maps HDR to the sRGB colors expected by the processor and browser preview.
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

    let (exited, exit) = tokio::sync::oneshot::channel();
    let child_shutdown = shutdown.child_token();
    let stop_child = child_shutdown.clone();
    let cleanup = tokio::spawn(async move {
        let status = tokio::select! {
            status = child.wait() => status.context("Failed to wait for FFmpeg"),
            () = stop_child.cancelled() => {
                // Observe reaping after cancellation; kill_on_drop alone is not a barrier.
                child.start_kill().context("Failed to terminate FFmpeg")?;
                child.wait().await.context("Failed to reap FFmpeg")
            }
        };
        match status {
            Ok(status) => {
                let _ = exited.send(Ok(status));
                Ok(())
            }
            Err(error) => {
                let message = format!("{error:#}");
                let _ = exited.send(Err(anyhow::anyhow!(message)));
                Err(error)
            }
        }
    });
    let cancel = child_shutdown.drop_guard();
    let frames = futures::stream::try_unfold(
        (
            stdout,
            exit,
            cancel,
            FramePool::default(),
            prysm_metrics::TraceStats::new("file_read"),
        ),
        move |(mut stdout, exit, cancel, mut pool, mut stats)| async move {
            let started = stats.start();
            if let Some(frame) = read_frame(&mut stdout, resolution, &mut pool)
                .await
                .inspect_err(|_| {
                    stats.failure();
                })
                .context("Failed to read FFmpeg frame")?
            {
                stats.record(started, frame.len(), Some(pool.stats()));
                return Ok(Some((frame, (stdout, exit, cancel, pool, stats))));
            }
            let status = exit.await.context("FFmpeg monitor ended unexpectedly")??;
            anyhow::ensure!(status.success(), "FFmpeg exited with {status}");
            tracing::info!("Video playback complete");
            Ok(None)
        },
    );
    Ok(VideoFeed {
        frames: frames.take_until(shutdown.cancelled_owned()).boxed(),
        cleanup,
    })
}

/// File frames plus the retained child-reaping task. Application composition owns both.
pub struct VideoFeed {
    frames: futures::stream::BoxStream<'static, Result<Frame>>,
    cleanup: tokio::task::JoinHandle<Result<()>>,
}
impl VideoFeed {
    pub(crate) fn into_tracked(
        self,
    ) -> (
        futures::stream::BoxStream<'static, Result<Frame>>,
        tokio::task::JoinHandle<Result<()>>,
    ) {
        (self.frames, self.cleanup)
    }
    /// Stop playback and wait until `FFmpeg` has been terminated and reaped.
    pub async fn finish(self) -> Result<()> {
        drop(self.frames);
        self.cleanup.await.context("FFmpeg cleanup task panicked")?
    }
}
impl Stream for VideoFeed {
    type Item = Result<Frame>;
    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.frames.as_mut().poll_next(context)
    }
}

async fn read_frame(
    reader: &mut (impl AsyncRead + Unpin),
    resolution: CaptureResolution,
    pool: &mut FramePool,
) -> io::Result<Option<Frame>> {
    let [width, height] = resolution.dimensions();
    let mut frame = pool
        .acquire(width, height, PixelFormat::RGB24)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let data = frame.as_mut_slice();
    if reader.read(&mut data[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut data[1..]).await?;
    Ok(Some(frame.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_reads_reuse_released_frames_and_preserve_retained_frames() {
        let resolution = CaptureResolution::new(8, 4).unwrap();
        let mut pool = FramePool::default();
        let bytes: Vec<_> = [1, 2, 3, 4]
            .into_iter()
            .flat_map(|value| [value; 96])
            .collect();
        let mut reader = bytes.as_slice();
        let first = read_frame(&mut reader, resolution, &mut pool)
            .await
            .unwrap()
            .unwrap();
        let pixels = first.as_slice().as_ptr();
        let second = read_frame(&mut reader, resolution, &mut pool)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(second.as_slice().as_ptr(), pixels);
        drop(first);
        let third = read_frame(&mut reader, resolution, &mut pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(third.as_slice().as_ptr(), pixels);
        assert_eq!(second.as_slice(), &[2; 96]);
        assert_eq!(third.as_slice(), &[3; 96]);
        drop(third);
        assert!(
            read_frame(&mut &[99; 95][..], resolution, &mut pool)
                .await
                .is_err()
        );
        let fourth = read_frame(&mut reader, resolution, &mut pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fourth.as_slice().as_ptr(), pixels);
        assert_eq!(fourth.as_slice(), &[4; 96]);
    }

    #[tokio::test]
    async fn reads_complete_frames_and_clean_eof() {
        let mut bytes = vec![42; 96];
        bytes.extend(vec![123; 96]);
        let mut reader = bytes.as_slice();
        for expected in [42, 123] {
            let frame = read_frame(
                &mut reader,
                CaptureResolution::new(8, 4).unwrap(),
                &mut FramePool::default(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!((frame.width(), frame.height()), (8, 4));
            assert_eq!(frame.format(), PixelFormat::RGB24);
            assert!(frame.as_slice().iter().all(|&byte| byte == expected));
        }
        assert!(
            read_frame(
                &mut reader,
                CaptureResolution::new(8, 4).unwrap(),
                &mut FramePool::default()
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    #[tokio::test]
    async fn truncated_frame_is_an_error() {
        let bytes = vec![0; CaptureResolution::default().rgb24_bytes() - 1];
        let error = read_frame(
            &mut bytes.as_slice(),
            CaptureResolution::default(),
            &mut FramePool::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg 9+ on PATH"]
    #[allow(
        clippy::too_many_lines,
        reason = "exercise ordered EOF, rate limiting, cancellation and decoder failure against one FFmpeg fixture"
    )]
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
            assert_eq!((frame.width(), frame.height()), (32, 18));
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
        let mut playback = video_feed(
            &path,
            CaptureResolution::new(32, 18).unwrap(),
            30.try_into().unwrap(),
            cancellation.clone(),
        )
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), playback.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        cancellation.cancel();
        assert!(playback.next().await.is_none());
        playback
            .finish()
            .await
            .expect("cancellation must terminate and reap FFmpeg");
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
