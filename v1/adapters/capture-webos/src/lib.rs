//! LG webOS TV video capture as a blocking [`FrameSource`].
//!
//! - [`WebosCapture`]: the factory registered under kind `"webos"`.
//! - [`WebosSettings`]: size, rate and UI blending, with typed validation.
//! - [`VideoSignal`]: the host feeds Luna `videooutput` readiness and context
//!   changes here; the source releases the video tap while video is
//!   unavailable.
//! - [`WebosFrames`]: the open source. A private native thread owns the LG
//!   tap; watchdogs, retries and release certification live here.
//!
//! ## TV UI without video
//!
//! With `include_ui` on, menus come blended into the video tap while video is
//! available or unknown. While video is unavailable (Home, apps without
//! video, settings over no input) the same native thread instead opens the
//! graphics plane through HAL GAL (`libhalgal`, `/dev/gfx`) at a small fixed
//! size (the configured size clamped to 256–320 × 144–180) and converts it to
//! the same limited-range BT.709 NV12. The two taps never run together: a
//! change of video availability releases one before the other opens. If the
//! UI tap fails to open or to deliver its first frame, it is logged once as
//! unsupported and the source idles without video, as with `include_ui` off.
//! A transparent plane is delivered as black.
//!
//! All `unsafe` FFI is in `prysm-capture-webos-sys`.
mod backend;
mod settings;
mod source;
mod ui;
mod video;

pub use settings::WebosSettings;
pub use source::WebosFrames;
pub use video::{Readiness, Video, VideoSignal};

use prysm_ports::{FrameSource, Result, SourceFactory, ValidationError};

/// Opens the TV's video capture.
#[derive(Debug, Clone, Default)]
pub struct WebosCapture {
    video: VideoSignal,
}

impl WebosCapture {
    /// Sources opened by this factory follow `video`. Without a feed
    /// ([`Default`]), video is [`Video::Unknown`] and capture is attempted,
    /// bounded by the preparation limit.
    #[must_use]
    pub fn new(video: VideoSignal) -> Self {
        Self { video }
    }

    /// Validate, take the device lock and start the native thread. Does not
    /// wait for video; frames follow once the driver is ready.
    ///
    /// # Errors
    /// Invalid settings, a non-TV platform, no root, or another capturer
    /// holding the device.
    pub fn start(&self, settings: &WebosSettings) -> Result<WebosFrames> {
        settings.validate()?;
        WebosFrames::start(
            backend::Lg::new()?,
            *settings,
            self.video.clone(),
            source::Limits::TV,
        )
    }
}

impl SourceFactory for WebosCapture {
    const KIND: &'static str = "webos";
    type Settings = WebosSettings;

    fn validate(&self, settings: &WebosSettings) -> Result<(), ValidationError> {
        settings.validate()
    }

    fn open(&self, settings: &WebosSettings) -> Result<Box<dyn FrameSource>> {
        Ok(Box::new(self.start(settings)?))
    }
}

#[cfg(test)]
mod tests;
