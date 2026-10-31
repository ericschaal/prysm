//! Adapt configured device, file, and webOS inputs to the same fallible frame stream.

use anyhow::Result;
use futures::{Stream, StreamExt, stream::BoxStream};
use prysm_capture::{Capturer, Frame, PrysmCapturer, WebosCapturer};
use prysm_core::CaptureResolution;
use std::{num::NonZeroU32, time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::video::video_feed;
use prysm_contract::Input;

pub(crate) struct InputStream {
    pub frames: BoxStream<'static, Result<Frame>>,
    pub completed: Option<CancellationToken>,
}

pub(crate) fn into_stream(
    input: Input,
    resolution: CaptureResolution,
    shutdown: CancellationToken,
) -> Result<InputStream> {
    let [width, height] = resolution.dimensions();
    let refresh_rate_hz = match &input {
        Input::Device {
            refresh_rate_hz, ..
        }
        | Input::File {
            refresh_rate_hz, ..
        }
        | Input::Webos {
            refresh_rate_hz, ..
        } => *refresh_rate_hz,
    };
    let mut input = match input {
        Input::Device { device, .. } => {
            let capturer = Capturer::new(Some(device.as_str()), shutdown)?;
            let completed = capturer.completion();
            InputStream {
                frames: capturer
                    .into_stream(width, height)
                    .map(|frame| frame.map_err(anyhow::Error::from))
                    .boxed(),
                completed: Some(completed),
            }
        }
        Input::File { path, .. } => InputStream {
            frames: video_feed(path.as_path(), resolution, shutdown)?.boxed(),
            completed: None,
        },
        Input::Webos { listen, .. } => {
            let capturer = WebosCapturer::new(listen, shutdown)?;
            let completed = capturer.completion();
            InputStream {
                frames: capturer
                    .into_stream(width, height)
                    .map(|frame| frame.map_err(anyhow::Error::from))
                    .boxed(),
                completed: Some(completed),
            }
        }
    };
    input.frames = limit_rate(input.frames, refresh_rate_hz).boxed();
    Ok(input)
}

/// Drop excess input frames before processing; errors and EOF pass through immediately.
fn limit_rate(
    frames: impl Stream<Item = Result<Frame>>,
    refresh_rate_hz: NonZeroU32,
) -> impl Stream<Item = Result<Frame>> {
    let interval =
        Duration::from_nanos(1_000_000_000u64.div_ceil(u64::from(refresh_rate_hz.get())));
    let mut last_frame: Option<Instant> = None;
    frames.filter_map(move |frame| {
        let now = Instant::now();
        let deliver =
            frame.is_err() || last_frame.is_none_or(|last| now.duration_since(last) >= interval);
        if deliver && frame.is_ok() {
            last_frame = Some(now);
        }
        futures::future::ready(deliver.then_some(frame))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn caps_frames_but_preserves_errors_and_eof() {
        let frames = futures::stream::iter([
            Ok(Frame::fill(1, 2, 1, prysm_capture::PixelFormat::RGB24)),
            Ok(Frame::fill(2, 2, 1, prysm_capture::PixelFormat::RGB24)),
            Err(anyhow::anyhow!("capture failed")),
        ]);
        let frames = limit_rate(frames, 1.try_into().unwrap());
        futures::pin_mut!(frames);
        assert_eq!(frames.next().await.unwrap().unwrap().as_slice(), &[1; 6]);
        assert_eq!(
            frames.next().await.unwrap().unwrap_err().to_string(),
            "capture failed"
        );
        assert!(frames.next().await.is_none());
    }

    #[tokio::test]
    async fn delivers_new_frames_after_the_interval_and_cancels_while_limited() {
        let shutdown = CancellationToken::new();
        let frames = futures::stream::unfold(0u8, |value| async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((
                Ok(Frame::fill(value, 2, 1, prysm_capture::PixelFormat::RGB24)),
                value.wrapping_add(1),
            ))
        })
        .take_until(shutdown.clone().cancelled_owned());
        let frames = limit_rate(frames, 10.try_into().unwrap());
        futures::pin_mut!(frames);
        let started = Instant::now();
        assert_eq!(frames.next().await.unwrap().unwrap().as_slice(), &[0; 6]);
        let next = frames.next().await.unwrap().unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(next.as_slice()[0] > 0);
        shutdown.cancel();
        assert!(frames.next().await.is_none());
    }

    #[test]
    fn file_input_preserves_startup_failure_context() {
        let input = Input::File {
            refresh_rate_hz: 30.try_into().unwrap(),
            path: String::from("/missing/prysm-input.mp4").try_into().unwrap(),
        };
        let error = into_stream(
            input,
            CaptureResolution::default(),
            CancellationToken::new(),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.to_string(),
            "Cannot open video /missing/prysm-input.mp4"
        );
        assert!(error.downcast_ref::<std::io::Error>().is_some());
    }
}
