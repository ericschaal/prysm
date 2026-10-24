//! Adapt configured device and file inputs to the same fallible frame stream.

use anyhow::Result;
use futures::{StreamExt, stream::BoxStream};
use prysm_capture::{Capturer, Frame, PrysmCapturer};
use prysm_core::CaptureResolution;
use tokio_util::sync::CancellationToken;

use crate::{config::Input, video::video_feed};

impl Input {
    pub(crate) fn into_stream(
        self,
        resolution: CaptureResolution,
        shutdown: CancellationToken,
    ) -> Result<BoxStream<'static, Result<Frame>>> {
        let [width, height] = resolution.dimensions();
        match self {
            Self::Device { device } => Ok(Capturer::new(Some(device.as_str()), shutdown)?
                .into_stream(width, height)
                .map(|frame| frame.map_err(anyhow::Error::from))
                .boxed()),
            Self::File { path } => Ok(video_feed(path.as_path(), resolution, shutdown)?.boxed()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn file_input_preserves_startup_failure_context() {
        let input = Input::File {
            path: PathBuf::from("/missing/prysm-input.mp4")
                .try_into()
                .unwrap(),
        };
        let error = input
            .into_stream(CaptureResolution::default(), CancellationToken::new())
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "Cannot open video /missing/prysm-input.mp4"
        );
        assert!(error.downcast_ref::<std::io::Error>().is_some());
    }
}
