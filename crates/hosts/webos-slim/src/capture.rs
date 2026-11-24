//! The TV capture, shaped by the receiver's request.
use crate::link::Flow;
use prysm_capture_remote::{Offer, RemoteSettings};
use prysm_capture_webos::WebosSettings;
use prysm_h264::Backend;
use prysm_ports::StreamEncoding;
use prysm_ports::{FrameSource, Mode, NoPicture, Pull, Result};
use prysm_webos_link::worker::StopHandle;
use std::sync::Arc;
use std::time::Duration;

/// What this TV streams from `capture`'s modes: raw as captured, and in
/// each H.264 encoder this build has, what both capture and encoder take.
#[must_use]
pub fn offer(capture: &[Mode]) -> Offer {
    Offer::new(StreamEncoding::ALL.into_iter().filter_map(|encoding| {
        let modes = match Backend::of(encoding) {
            None => capture.to_vec(),
            Some(backend) if backend.available() => capture
                .iter()
                .filter_map(|mode| mode.and(&backend.limits()))
                .collect(),
            Some(_) => return None,
        };
        Some((encoding, modes))
    }))
}

/// The capture settings a request asks for; its encoding is the sender's.
#[must_use]
pub fn settings(request: &RemoteSettings) -> WebosSettings {
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
/// receiver has stopped the stream (its lighting is off), while its request
/// is refused (the encoder failed) or while Prysm speaks another version,
/// until a receiver takes a connection again, or for good once another
/// sender replaced this one. It keeps running while a receiver that did not say so is away, so a
/// dropped connection does not restart the TV capture. With nothing open,
/// `next_frame` waits out its timeout and reports idle, which keeps the
/// uplink reconnecting and reading the receiver's messages. Once the worker
/// is asked to stop (`stop` has a reason) nothing is opened any more, so a
/// stop costs at most one release: of the capture being replaced, or of the
/// open one.
pub struct RequestedCapture<O> {
    flow: Arc<Flow>,
    stop: StopHandle,
    open: O,
    current: Option<(WebosSettings, Box<dyn FrameSource>)>,
}

impl<O> RequestedCapture<O>
where
    O: FnMut(WebosSettings) -> Result<Box<dyn FrameSource>> + Send,
{
    pub fn new(flow: Arc<Flow>, stop: StopHandle, open: O) -> Self {
        Self {
            flow,
            stop,
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
            // Read after the close, which a stop can arrive during.
            let stopping = self.stop.reason().is_some();
            if let Some(wanted) = wanted.filter(|_| !stopping) {
                self.current = Some((wanted, (self.open)(wanted)?));
            }
        }
        match &mut self.current {
            Some((_, source)) => {
                let pull = source.next_frame(timeout);
                self.flow.pulled(pull.as_ref().ok());
                pull
            }
            // Waits like any idle pull, so a stop cannot make it spin.
            None => {
                self.flow.pulled(None);
                std::thread::sleep(timeout);
                Ok(Pull::NoPicture(NoPicture::Quiet))
            }
        }
    }

    fn close(self: Box<Self>) -> Result<()> {
        self.current.map_or(Ok(()), |(_, source)| source.close())
    }
}
