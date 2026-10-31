//! Adapt configured device, file, and webOS inputs to the same fallible frame stream.

use anyhow::Result;
use futures::{StreamExt, stream::BoxStream};
use prysm_capture::{Capturer, Frame, PrysmCapturer, WebosCapturer};
use prysm_core::CaptureResolution;
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
    let input = match input {
        Input::Device {
            device,
            refresh_rate_hz,
        } => {
            let capturer = Capturer::new(Some(device.as_str()), shutdown)?;
            let completed = capturer.completion();
            InputStream {
                frames: capturer
                    .into_stream(width, height, refresh_rate_hz)
                    .map(|frame| frame.map_err(anyhow::Error::from))
                    .boxed(),
                completed: Some(completed),
            }
        }
        Input::File {
            path,
            refresh_rate_hz,
        } => InputStream {
            frames: video_feed(path.as_path(), resolution, refresh_rate_hz, shutdown)?.boxed(),
            completed: None,
        },
        Input::Webos {
            listen,
            refresh_rate_hz,
        } => {
            let capturer = WebosCapturer::new(listen, shutdown)?;
            let completed = capturer.completion();
            InputStream {
                frames: capturer
                    .into_stream(width, height, refresh_rate_hz)
                    .map(|frame| frame.map_err(anyhow::Error::from))
                    .boxed(),
                completed: Some(completed),
            }
        }
    };
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;

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
