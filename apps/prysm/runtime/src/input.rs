//! Adapt configured device, file, and webOS inputs to the same fallible frame stream.

use anyhow::Result;
#[cfg(feature = "host-input")]
use futures::StreamExt;
use futures::stream::BoxStream;
#[cfg(feature = "host-input")]
use prysm_capture::{Capturer, PrysmCapturer, WebosReceiver};
use prysm_core::Frame;
use tokio_util::sync::CancellationToken;

#[cfg(feature = "host-input")]
use crate::video::video_feed;
use prysm_contract::Input;

pub struct InputStream {
    pub(crate) frames: BoxStream<'static, Result<Frame>>,
    pub(crate) completed: Option<CancellationToken>,
    pub(crate) cleanup: Option<tokio::task::JoinHandle<Result<()>>>,
    pub(crate) availability: Option<tokio::sync::watch::Receiver<bool>>,
}

impl InputStream {
    /// Couple native frames to their owner's join handle and availability signal.
    #[must_use]
    pub fn native(
        frames: BoxStream<'static, Result<Frame>>,
        cleanup: tokio::task::JoinHandle<Result<()>>,
        availability: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            frames,
            completed: None,
            cleanup: Some(cleanup),
            availability: Some(availability),
        }
    }
}

/// The native adapter owns its capture thread until the stream's cleanup handle completes.
pub trait NativeCapture: Send + Sync {
    fn open(
        &self,
        resolution: prysm_contract::CaptureResolution,
        rate: std::num::NonZeroU32,
        stop: CancellationToken,
    ) -> Result<InputStream>;
}

#[derive(Clone)]
pub(crate) enum Inputs {
    #[cfg(feature = "host-input")]
    Host(WebosReceiver),
    Native(std::sync::Arc<dyn NativeCapture>),
}
#[cfg(feature = "host-input")]
impl From<WebosReceiver> for Inputs {
    fn from(value: WebosReceiver) -> Self {
        Self::Host(value)
    }
}
#[cfg(all(test, feature = "host-input"))]
impl Inputs {
    pub fn host(&self) -> &WebosReceiver {
        match self {
            Self::Host(receiver) => receiver,
            Self::Native(_) => panic!("expected host fixture"),
        }
    }
}

/// Availability belongs to the input; native capture resources belong to its active stream.
#[derive(Clone)]
pub(crate) struct InputSource {
    config: Input,
    inputs: Inputs,
}

impl InputSource {
    pub fn new(config: Input, inputs: impl Into<Inputs>) -> Self {
        Self {
            config,
            inputs: inputs.into(),
        }
    }

    pub fn into_stream(self, shutdown: CancellationToken) -> Result<InputStream> {
        let input = self.config;
        let resolution = input.capture_resolution();
        match self.inputs {
            Inputs::Native(native) => {
                let Input::Webos {
                    refresh_rate_hz, ..
                } = input
                else {
                    anyhow::bail!("embedded Prysm only supports native webOS capture")
                };
                native.open(resolution, refresh_rate_hz, shutdown)
            }
            #[cfg(feature = "host-input")]
            Inputs::Host(receiver) => {
                let resolution = resolution.to_core();
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
    }
}

#[cfg(all(test, feature = "host-input"))]
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
