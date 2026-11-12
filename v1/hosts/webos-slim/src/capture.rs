//! The TV capture, shaped by the receiver's request.
use crate::link::Flow;
use prysm_capture_remote::StreamRequest;
use prysm_capture_webos::WebosSettings;
use prysm_ports::{FrameSource, Pull, Result};
use std::sync::Arc;
use std::time::Duration;

/// The capture settings a request asks for; its encoding is the sender's.
#[must_use]
pub fn settings(request: &StreamRequest) -> WebosSettings {
    WebosSettings {
        width: request.width,
        height: request.height,
        fps: request.fps,
        include_ui: request.include_ui,
    }
}

/// A [`FrameSource`] that captures at the size, rate and menu blending the
/// receiver wants ([`Flow::wanted`]): nothing before the first request,
/// closed and opened again when a request changes them, and closed while the
/// receiver has paused the stream (its lighting is off) until it resumes or
/// a receiver connects again. It keeps running while a receiver that did not
/// pause is away, so a dropped connection does not restart the TV capture.
/// With nothing open, `next_frame` waits out its timeout and reports idle,
/// which keeps the uplink reconnecting and reading the receiver's resume.
pub struct RequestedCapture<O> {
    flow: Arc<Flow>,
    open: O,
    current: Option<(WebosSettings, Box<dyn FrameSource>)>,
}

impl<O> RequestedCapture<O>
where
    O: FnMut(WebosSettings) -> Result<Box<dyn FrameSource>> + Send,
{
    pub fn new(flow: Arc<Flow>, open: O) -> Self {
        Self {
            flow,
            open,
            current: None,
        }
    }
}

impl<O> FrameSource for RequestedCapture<O>
where
    O: FnMut(WebosSettings) -> Result<Box<dyn FrameSource>> + Send,
{
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        let wanted = self.flow.wanted().map(|request| settings(&request));
        if self.current.as_ref().map(|(open, _)| *open) != wanted {
            if let Some((_, old)) = self.current.take() {
                old.close()?;
            }
            if let Some(wanted) = wanted {
                self.current = Some((wanted, (self.open)(wanted)?));
            }
        }
        match &mut self.current {
            Some((_, source)) => source.next_frame(timeout),
            None => {
                std::thread::sleep(timeout);
                Ok(Pull::Idle)
            }
        }
    }

    fn close(self: Box<Self>) -> Result<()> {
        self.current.map_or(Ok(()), |(_, source)| source.close())
    }
}
