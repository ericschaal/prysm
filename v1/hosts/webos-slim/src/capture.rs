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
/// receiver last requested ([`Flow::requested`]): nothing before the first
/// request, and closed and opened again when a request changes them. It
/// keeps running while the receiver is away, so lighting going off and on
/// at the desktop does not restart the TV capture.
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
        if let Some(wanted) = self.flow.requested().map(|request| settings(&request))
            && self
                .current
                .as_ref()
                .is_none_or(|(open, _)| *open != wanted)
        {
            if let Some((_, old)) = self.current.take() {
                old.close()?;
            }
            self.current = Some((wanted, (self.open)(wanted)?));
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
