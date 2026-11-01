//! Coordinate connection negotiation, frame capture, transmission, and pacing.
use crate::{
    capture::{Capture, UiCapture},
    cli::{CaptureOptions, Cli, Compression},
    native::Video,
    transport::Sender,
};
use anyhow::Result;
use prysm_metrics::{PipelineEvent, Stage};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// One negotiated receiver connection and its native capture resources.
pub struct Session {
    capture: Capture,
    sender: Sender,
}

impl Session {
    /// Connect before acquiring any video or UI resources.
    pub fn connect(cli: &Cli) -> Result<Self> {
        let mut sender = Sender::new(cli.receiver, cli.compression);
        sender.h264_bitrate = cli.h264_bitrate;
        sender.connect()?;

        Ok(Self {
            capture: Capture::new(cli),
            sender,
        })
    }

    pub fn wait_for_capture(&mut self, cli: &Cli, stop: &AtomicBool) -> Result<bool> {
        let capturing = self.sender.wait_for_capture(stop)?;
        if capturing {
            record_capture_settings(cli, self.sender.fps, self.sender.capture_resolution);
        }
        Ok(capturing)
    }

    pub fn configure(&mut self, ui: &mut Option<impl UiCapture>) -> Result<()> {
        self.capture
            .configure(self.sender.fps, self.sender.capture_resolution, ui)
    }

    /// Native resources remain owned by the caller until the session is dropped.
    pub fn run(
        &mut self,
        stop: &AtomicBool,
        open_video: &mut impl FnMut(&CaptureOptions) -> Result<Video>,
        capture_ui: &mut Option<impl UiCapture>,
    ) -> Result<()> {
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            if let Err(error) = self.sender.check_connection() {
                tracing::info!(error = %format!("{error:#}"), "receiver disconnected");
                return Ok(());
            }
            let work = prysm_metrics::timer(Stage::CaptureIteration);
            if let Some(frame) = self.capture.capture(open_video, capture_ui)?
                && let Err(error) = self.sender.send(frame.header, frame.pixels, frame.content)
            {
                tracing::warn!(error = %format!("{error:#}"), "frame send failed");
                return Ok(());
            }
            drop(work);
            pace(&mut next, self.sender.fps);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn capture_loop(
    cli: &Cli,
    stop: &AtomicBool,
    mut open_video: impl FnMut(&CaptureOptions) -> Result<Video>,
    mut capture_ui: Option<impl UiCapture>,
) -> Result<()> {
    let mut session = Session::connect(cli)?;
    if session.wait_for_capture(cli, stop)? {
        session.configure(&mut capture_ui)?;
        session.run(stop, &mut open_video, &mut capture_ui)?;
    }
    Ok(())
}

fn record_capture_settings(cli: &Cli, fps: u8, [width, height]: [u32; 2]) {
    tracing::info!(fps, width, height, "capture settings negotiated");
    prysm_metrics::capture_info(
        cli.source.into(),
        cli.compression.into(),
        width,
        height,
        fps,
        prysm_webos_protocol::PixelFormat::from(cli.composition_format).into(),
        if cli.compression == Compression::H264 {
            cli.h264_bitrate
        } else {
            0
        },
    );
}

fn pace(next: &mut Instant, fps: u8) {
    // Skip missed ticks instead of building up latency or sending bursts.
    *next += Duration::from_secs_f64(1.0 / f64::from(fps));
    let now = Instant::now();
    if *next > now {
        std::thread::sleep(*next - now);
    } else {
        prysm_metrics::event(PipelineEvent::MissedTick);
        *next = now;
    }
}

#[cfg(test)]
mod tests;
