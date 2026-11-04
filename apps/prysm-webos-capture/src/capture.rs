//! Own one native capture tap and reuse its packed frame storage.
use crate::{
    config::{CaptureOptions, Config, Source},
    events::{Event, Events},
    native::Video,
};
use anyhow::Result;
use prysm_metrics::{Content, FrameStage, Stage, VideoCaptureEvent};
use prysm_webos_protocol::Header;
use std::time::{Duration, Instant};

pub struct CapturedFrame<'a> {
    pub header: Header,
    pub pixels: &'a [u8],
    pub content: Content,
}

pub trait FrameCapture {
    fn capture(&mut self, output: &mut Vec<u8>, events: &mut Events) -> Result<Option<Header>>;
}

impl FrameCapture for Video {
    fn capture(&mut self, output: &mut Vec<u8>, events: &mut Events) -> Result<Option<Header>> {
        Self::capture(self, output, events)
    }
}

#[cfg(test)]
impl<F: FnMut(&mut Vec<u8>) -> Result<Option<Header>>> FrameCapture for F {
    fn capture(&mut self, output: &mut Vec<u8>, _: &mut Events) -> Result<Option<Header>> {
        self(output)
    }
}

pub struct Capture<V: FrameCapture> {
    options: CaptureOptions,
    source: Source,
    video: Option<V>,
    video_available: bool,
    video_retry: Instant,
    native: Vec<u8>,
}

impl<V: FrameCapture> Capture<V> {
    #[must_use]
    pub fn new(config: &Config) -> Self {
        Self::with_options(config.capture, config.source)
    }

    #[must_use]
    pub fn with_options(options: CaptureOptions, source: Source) -> Self {
        prysm_metrics::backend_info(prysm_metrics::Backend::Disabled);
        Self {
            options,
            source,
            video: None,
            video_available: true,
            video_retry: Instant::now(),
            native: Vec::new(),
        }
    }

    pub fn configure(&mut self, fps: u8, resolution: [u32; 2], events: &mut Events) -> Result<()> {
        if self.options.fps != fps || [self.options.width, self.options.height] != resolution {
            self.release_video(events)?;
            self.options.fps = fps;
            [self.options.width, self.options.height] = resolution;
            self.video_retry = Instant::now();
        }
        Ok(())
    }

    pub fn set_video_available(&mut self, available: bool) {
        if available != self.video_available {
            tracing::info!(available, "video source availability changed");
        }
        if available && !self.video_available {
            self.video_retry = Instant::now();
        }
        self.video_available = available;
    }

    #[must_use]
    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }

    pub fn release_video(&mut self, events: &mut Events) -> Result<()> {
        if self.video.is_some() {
            events.send(&Event::VideoResetting)?;
            self.video = None;
            events.send(&Event::VideoReset)?;
        }
        Ok(())
    }

    pub fn capture(
        &mut self,
        open_video: &mut impl FnMut(&CaptureOptions) -> Result<V>,
        events: &mut Events,
    ) -> Result<Option<CapturedFrame<'_>>> {
        // The native tap cannot initialize without an active video source.
        if !self.video_available {
            return Ok(None);
        }
        let header = self.capture_video(open_video, events)?;
        Ok(header.map(|header| CapturedFrame {
            header,
            pixels: &self.native,
            content: match self.source {
                Source::Both => Content::Blended,
                Source::Video => Content::Video,
            },
        }))
    }

    fn capture_video(
        &mut self,
        open_video: &mut impl FnMut(&CaptureOptions) -> Result<V>,
        events: &mut Events,
    ) -> Result<Option<Header>> {
        if !self.has_video() && Instant::now() >= self.video_retry {
            prysm_metrics::event(VideoCaptureEvent::InitAttempt);
            events.send(&Event::VideoResetting)?;
            let result = open_video(&self.options);
            events.send(&Event::VideoReset)?;
            match result {
                Ok(capture) => {
                    prysm_metrics::backend_info(match self.source {
                        Source::Both => prysm_metrics::Backend::VtcaptureBlended,
                        Source::Video => prysm_metrics::Backend::Vtcapture,
                    });
                    self.video = Some(capture);
                }
                Err(error) if error.is::<crate::native::FatalVideo>() => return Err(error),
                Err(error) => {
                    prysm_metrics::event(VideoCaptureEvent::InitFailed);
                    tracing::warn!(error = %format!("{error:#}"), "video unavailable; retrying");
                    self.video_retry = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        let Some(capture) = self.video.as_mut() else {
            return Ok(None);
        };
        let capture_time = prysm_metrics::timer(Stage::VideoAcquire);
        let result = capture.capture(&mut self.native, events);
        drop(capture_time);
        Ok(match result {
            Ok(Some(header)) => {
                prysm_metrics::frame(FrameStage::VideoCaptured, header.format.into());
                Some(header)
            }
            Ok(None) => {
                prysm_metrics::event(VideoCaptureEvent::Duplicate);
                None
            }
            // Keep ownership until main has reported failure; native Drop can block.
            Err(error) if error.is::<crate::native::FatalVideo>() => return Err(error),
            Err(error) => {
                prysm_metrics::event(VideoCaptureEvent::AcquireFailed);
                tracing::warn!(error = %format!("{error:#}"), "video interrupted; restarting");
                self.release_video(events)?;
                prysm_metrics::backend_info(prysm_metrics::Backend::Disabled);
                self.video_retry = Instant::now() + Duration::from_secs(1);
                None
            }
        })
    }
}

#[cfg(test)]
mod tests;
