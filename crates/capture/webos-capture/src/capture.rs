//! Acquire video/UI layers and assemble frames using reusable pixel buffers.
use crate::{
    cli::{CaptureOptions, Cli, CompositionFormat, Source},
    native::{Ui, Video},
    pixels::{Bgra8Pixels, CompositionBuffer},
};
use anyhow::Result;
use prysm_metrics::{Content, FrameStage, Stage, UiCaptureEvent, VideoCaptureEvent};
use prysm_webos_protocol::Header;
use std::time::{Duration, Instant};

pub struct CapturedFrame<'a> {
    pub header: Header,
    pub pixels: &'a [u8],
    pub content: Content,
}

fn open_ui(source: Source, options: &CaptureOptions) -> Result<Option<Ui>> {
    let ui = if source.includes_ui() {
        match Ui::open(options) {
            Ok(ui) => Some(ui),
            Err(error) if source == Source::Ui => return Err(error),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "UI unavailable; video only");
                None
            }
        }
    } else {
        None
    };
    prysm_metrics::backend_info(
        prysm_metrics::Layer::Ui,
        ui.as_ref()
            .map_or(prysm_metrics::Backend::Disabled, Ui::metrics_backend),
    );
    Ok(ui)
}

/// A UI source can recreate size-dependent resources before acquiring frames.
pub trait UiCapture {
    fn configure(&mut self, _options: &CaptureOptions) -> Result<()> {
        Ok(())
    }
    fn capture(&mut self, width: u32, height: u32, output: &mut Bgra8Pixels) -> Result<bool>;
}

impl<F: FnMut(u32, u32, &mut Bgra8Pixels) -> Result<()>> UiCapture for F {
    fn capture(&mut self, width: u32, height: u32, output: &mut Bgra8Pixels) -> Result<bool> {
        self(width, height, output).map(|()| true)
    }
}

pub struct NativeUi {
    source: Source,
    ui: Option<Ui>,
}

impl NativeUi {
    pub fn new(source: Source) -> Self {
        Self { source, ui: None }
    }
}

impl UiCapture for NativeUi {
    fn configure(&mut self, options: &CaptureOptions) -> Result<()> {
        self.ui = None;
        self.ui = open_ui(self.source, options)?;
        Ok(())
    }

    fn capture(&mut self, width: u32, height: u32, output: &mut Bgra8Pixels) -> Result<bool> {
        match self.ui.as_mut() {
            Some(ui) => ui.capture(width, height, output).map(|()| true),
            None => Ok(false),
        }
    }
}

pub struct Capture {
    options: CaptureOptions,
    source: Source,
    composition_format: CompositionFormat,
    video: Option<Video>,
    video_retry: Instant,
    native: Vec<u8>,
    composition: CompositionBuffer,
    bgra: Bgra8Pixels,
    ui_warning: Instant,
    configured: bool,
}

impl Capture {
    pub fn new(cli: &Cli) -> Self {
        prysm_metrics::backend_info(
            prysm_metrics::Layer::Video,
            prysm_metrics::Backend::Disabled,
        );
        Self {
            options: cli.capture.clone(),
            source: cli.source,
            composition_format: cli.composition_format,
            video: None,
            video_retry: Instant::now(),
            native: Vec::new(),
            composition: CompositionBuffer::default(),
            bgra: Bgra8Pixels::default(),
            ui_warning: Instant::now(),
            configured: false,
        }
    }

    pub fn configure(
        &mut self,
        fps: u8,
        resolution: [u32; 2],
        ui: &mut Option<impl UiCapture>,
    ) -> Result<()> {
        let resized = [self.options.width, self.options.height] != resolution;
        if self.options.fps != fps || resized {
            self.release_video();
            self.options.fps = fps;
            [self.options.width, self.options.height] = resolution;
            self.video_retry = Instant::now();
        }
        if !self.configured || resized {
            if let Some(ui) = ui {
                ui.configure(&self.options)?;
            }
            self.configured = true;
        }
        Ok(())
    }

    pub fn release_video(&mut self) {
        self.video = None;
    }

    pub fn capture(
        &mut self,
        open_video: &mut impl FnMut(&CaptureOptions) -> Result<Video>,
        capture_ui: &mut Option<impl UiCapture>,
    ) -> Result<Option<CapturedFrame<'_>>> {
        let video = if self.source.includes_video() {
            self.capture_video(open_video)
        } else {
            None
        };
        if let Some(capture_ui) = capture_ui {
            let (width, height) = video.map_or((self.options.width, self.options.height), |h| {
                (h.width, h.height)
            });
            let ui_time = prysm_metrics::timer(Stage::UiAcquire);
            let result = capture_ui.capture(width, height, &mut self.bgra);
            drop(ui_time);
            if result.is_err() {
                prysm_metrics::event(UiCaptureEvent::AcquireFailed);
            }
            match result {
                Ok(true) => {
                    prysm_metrics::frame(FrameStage::UiCaptured, (&self.bgra).into());
                    let content = if video.is_some() {
                        Content::Blended
                    } else {
                        Content::Ui
                    };
                    let composed = self.composition.compose(
                        width,
                        height,
                        video.map(|header| (header, self.native.as_slice())),
                        &self.bgra,
                        self.composition_format,
                    )?;
                    if content == Content::Blended {
                        prysm_metrics::frame(FrameStage::Composited, composed.header.format.into());
                    }
                    return Ok(Some(CapturedFrame {
                        header: composed.header,
                        pixels: composed.pixels,
                        content,
                    }));
                }
                Err(error) if Instant::now() >= self.ui_warning => {
                    tracing::warn!(error = %format!("{error:#}"), "UI frame skipped");
                    self.ui_warning = Instant::now() + Duration::from_secs(1);
                }
                Ok(false) | Err(_) => {}
            }
        }
        Ok(video.map(|header| CapturedFrame {
            header,
            pixels: &self.native,
            content: Content::Video,
        }))
    }

    fn capture_video(
        &mut self,
        open_video: &mut impl FnMut(&CaptureOptions) -> Result<Video>,
    ) -> Option<Header> {
        if self.video.is_none() && Instant::now() >= self.video_retry {
            prysm_metrics::event(VideoCaptureEvent::InitAttempt);
            match open_video(&self.options) {
                Ok(capture) => {
                    prysm_metrics::backend_info(
                        prysm_metrics::Layer::Video,
                        capture.metrics_backend(),
                    );
                    self.video = Some(capture);
                }
                Err(error) => {
                    prysm_metrics::event(VideoCaptureEvent::InitFailed);
                    tracing::warn!(error = %format!("{error:#}"), "video unavailable; retrying");
                    self.video_retry = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        let capture = self.video.as_mut()?;
        let capture_time = prysm_metrics::timer(Stage::VideoAcquire);
        let result = capture.capture(&mut self.native);
        drop(capture_time);
        match result {
            Ok(Some(header)) => {
                prysm_metrics::frame(FrameStage::VideoCaptured, header.format.into());
                Some(header)
            }
            Ok(None) => {
                prysm_metrics::event(VideoCaptureEvent::Duplicate);
                None
            }
            Err(error) => {
                prysm_metrics::event(VideoCaptureEvent::AcquireFailed);
                tracing::warn!(error = %format!("{error:#}"), "video interrupted; restarting");
                self.release_video();
                prysm_metrics::backend_info(
                    prysm_metrics::Layer::Video,
                    prysm_metrics::Backend::Disabled,
                );
                self.video_retry = Instant::now() + Duration::from_secs(1);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
