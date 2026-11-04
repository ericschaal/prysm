//! Adapt configured device, file, and webOS inputs to the same fallible frame stream.

use anyhow::Result;
use futures::{StreamExt, stream::BoxStream};
use prysm_capture::{Capturer, Frame, PrysmCapturer, WebosReceiver};
use tokio_util::sync::CancellationToken;

use crate::video::video_feed;
use prysm_contract::Input;

pub(crate) struct InputStream {
    pub frames: BoxStream<'static, Result<Frame>>,
    pub completed: Option<CancellationToken>,
    pub cleanup: Option<tokio::task::JoinHandle<Result<()>>>,
    pub availability: Option<tokio::sync::watch::Receiver<bool>>,
}

/// Availability belongs to the input; native capture resources belong to its active stream.
#[derive(Clone)]
pub(crate) struct InputSource {
    config: Input,
    receiver: WebosReceiver,
}

impl InputSource {
    pub fn new(config: Input, receiver: WebosReceiver) -> Self {
        Self { config, receiver }
    }

    pub fn into_stream(self, shutdown: CancellationToken) -> Result<InputStream> {
        let input = self.config;
        let resolution = input.capture_resolution().to_core();
        let [width, height] = resolution.dimensions();
        let input = match input {
            Input::Device {
                device,
                refresh_rate_hz,
                ..
            } => {
                let capturer = Capturer::new(Some(device.as_str()), shutdown)?;
                let completed = capturer.completion();
                InputStream {
                    frames: capturer
                        .into_stream(width, height, refresh_rate_hz)
                        .map(|frame| frame.map_err(anyhow::Error::from))
                        .boxed(),
                    completed: Some(completed),
                    cleanup: None,
                    availability: None,
                }
            }
            Input::File {
                path,
                refresh_rate_hz,
                ..
            } => {
                let (frames, cleanup) =
                    video_feed(path.as_path(), resolution, refresh_rate_hz, shutdown)?
                        .into_tracked();
                InputStream {
                    frames,
                    completed: None,
                    cleanup: Some(cleanup),
                    availability: None,
                }
            }
            Input::Webos {
                refresh_rate_hz, ..
            } => {
                let receiver = self.receiver;
                let (frames, completed, availability) =
                    receiver.capture(width, height, refresh_rate_hz, shutdown)?;
                InputStream {
                    frames: frames
                        .map(|frame| frame.map_err(anyhow::Error::from))
                        .boxed(),
                    completed: Some(completed),
                    cleanup: None,
                    availability: Some(availability),
                }
            }
        };
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_input_preserves_startup_failure_context() {
        let input = Input::File {
            capture_resolution: [640, 360].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
            path: String::from("/missing/prysm-input.mp4").try_into().unwrap(),
        };
        let source = InputSource::new(input, WebosReceiver::new().unwrap());
        let error = source.into_stream(CancellationToken::new()).err().unwrap();
        assert_eq!(
            error.to_string(),
            "Cannot open video /missing/prysm-input.mp4"
        );
        assert!(error.downcast_ref::<std::io::Error>().is_some());
    }
}
