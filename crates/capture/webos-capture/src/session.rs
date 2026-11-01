//! Coordinate connection negotiation, frame capture, transmission, and pacing.
use crate::{
    capture::{Capture, UiCapture},
    cli::{CaptureOptions, Cli, Compression, Source},
    native::Video,
    transport::Sender,
};
use anyhow::Result;
use prysm_metrics::{PipelineEvent, Stage};
#[cfg(test)]
use prysm_webos_protocol::worker::VideoAvailability;
use prysm_webos_protocol::worker::VideoUpdate;
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
        let mut sender = Sender::new(cli.receiver.socket_addr(), cli.compression);
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

    pub fn configure(
        &mut self,
        ui: &mut Option<impl UiCapture>,
        events: &mut crate::native::events::Events,
    ) -> Result<()> {
        self.capture
            .configure(self.sender.fps, self.sender.capture_resolution, ui, events)
    }

    /// Native resources remain owned by the caller until the session is dropped.
    pub fn run(
        &mut self,
        stop: &AtomicBool,
        open_video: &mut impl FnMut(&CaptureOptions) -> Result<Video>,
        capture_ui: &mut Option<impl UiCapture>,
        readiness: &mut impl FnMut() -> Result<VideoUpdate>,
        events: &mut crate::native::events::Events,
    ) -> Result<()> {
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            if let Err(error) = self.sender.check_connection() {
                tracing::info!(error = %format!("{error:#}"), "receiver disconnected");
                return Ok(());
            }
            let update = readiness()?;
            if update.reset {
                self.capture.release_video(events)?;
            }
            self.capture
                .set_video_available(update.availability.allows_capture());
            let work = prysm_metrics::timer(Stage::CaptureIteration);
            if let Some(frame) = self.capture.capture(open_video, capture_ui, events)?
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
        session.configure(
            &mut capture_ui,
            &mut crate::native::events::Events::disabled(),
        )?;
        session.run(
            stop,
            &mut open_video,
            &mut capture_ui,
            &mut || {
                Ok(VideoUpdate {
                    availability: VideoAvailability::Unknown,
                    reset: false,
                })
            },
            &mut crate::native::events::Events::disabled(),
        )?;
    }
    Ok(())
}

fn record_capture_settings(cli: &Cli, fps: u8, [width, height]: [u32; 2]) {
    tracing::info!(fps, width, height, "capture settings negotiated");
    prysm_metrics::capture_info(
        match cli.source {
            Source::Video => prysm_metrics::CaptureSource::Video,
            Source::Ui => prysm_metrics::CaptureSource::Ui,
            Source::Both => prysm_metrics::CaptureSource::Both,
        },
        match cli.compression {
            Compression::None => prysm_metrics::Codec::None,
            Compression::Lz4 => prysm_metrics::Codec::Lz4,
            Compression::H264 => prysm_metrics::Codec::H264,
        },
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
