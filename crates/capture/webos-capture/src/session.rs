//! Coordinate connection negotiation, frame capture, transmission, and pacing.
use crate::{
    capture::{Capture, UiCapture},
    config::{CaptureOptions, Compression, Config, Source},
    encoder::{Encoder, H264_BITRATE},
    events::Events,
    native::Video,
    transport::Sender,
};
use anyhow::Result;
use prysm_metrics::{PipelineEvent, Stage};
#[cfg(test)]
use prysm_webos_protocol::worker::VideoAvailability;
use prysm_webos_protocol::{ControlMessage, worker::VideoUpdate};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// One negotiated receiver connection and its native capture resources.
pub struct Session {
    capture: Capture,
    sender: Sender,
    state: State,
    compression: Compression,
}

enum State {
    Waiting(ControlMessage),
    Capturing(CaptureOptions),
}

impl Session {
    /// Connect before acquiring any video or UI resources.
    pub fn connect(config: &Config) -> Result<Self> {
        let (sender, message) = Sender::connect(config.receiver.socket_addr())?;
        Ok(Self {
            capture: Capture::new(config),
            sender,
            state: State::Waiting(message),
            compression: config.compression,
        })
    }

    pub fn wait_for_capture(&mut self, config: &Config, stop: &AtomicBool) -> Result<bool> {
        let message = match &self.state {
            State::Waiting(message) => *message,
            State::Capturing(_) => return Ok(true),
        };
        let Some(settings) = self.sender.wait_for_capture(message, stop)? else {
            return Ok(false);
        };
        record_capture_settings(config, settings.fps, [settings.width, settings.height]);
        self.state = State::Capturing(settings);
        Ok(true)
    }

    fn settings(&self) -> Result<CaptureOptions> {
        match self.state {
            State::Capturing(settings) => Ok(settings),
            State::Waiting(_) => anyhow::bail!("receiver has not requested capture"),
        }
    }

    pub fn configure(
        &mut self,
        ui: &mut Option<impl UiCapture>,
        events: &mut Events,
    ) -> Result<()> {
        let settings = self.settings()?;
        self.capture
            .configure(settings.fps, [settings.width, settings.height], ui, events)
    }

    /// Native resources remain owned by the caller until the session is dropped.
    pub fn run(
        &mut self,
        stop: &AtomicBool,
        open_video: &mut impl FnMut(&CaptureOptions) -> Result<Video>,
        capture_ui: &mut Option<impl UiCapture>,
        readiness: &mut impl FnMut() -> Result<VideoUpdate>,
        events: &mut Events,
    ) -> Result<()> {
        let settings = self.settings()?;
        let mut encoder = Encoder::new(self.compression, settings.fps, H264_BITRATE)?;
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
            if let Some(frame) = self.capture.capture(open_video, capture_ui, events)? {
                let result = encoder
                    .encode(frame.header, frame.pixels, frame.content)
                    .and_then(|packet| match packet {
                        Some(packet) => self.sender.send(packet, frame.content),
                        None => Ok(()),
                    });
                if let Err(error) = result {
                    self.sender.disconnect();
                    tracing::warn!(error = %format!("{error:#}"), "frame send failed");
                    return Ok(());
                }
            }
            drop(work);
            pace(&mut next, settings.fps);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn capture_loop(
    config: &Config,
    stop: &AtomicBool,
    mut open_video: impl FnMut(&CaptureOptions) -> Result<Video>,
    mut capture_ui: Option<impl UiCapture>,
) -> Result<()> {
    let mut session = Session::connect(config)?;
    if session.wait_for_capture(config, stop)? {
        session.configure(&mut capture_ui, &mut Events::disabled())?;
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
            &mut Events::disabled(),
        )?;
    }
    Ok(())
}

fn record_capture_settings(config: &Config, fps: u8, [width, height]: [u32; 2]) {
    tracing::info!(fps, width, height, "capture settings negotiated");
    prysm_metrics::capture_info(
        match config.source {
            Source::Video => prysm_metrics::CaptureSource::Video,
            Source::Ui => prysm_metrics::CaptureSource::Ui,
            Source::Both => prysm_metrics::CaptureSource::Both,
        },
        match config.compression {
            Compression::None => prysm_metrics::Codec::None,
            Compression::Lz4 => prysm_metrics::Codec::Lz4,
            Compression::H264 => prysm_metrics::Codec::H264,
        },
        width,
        height,
        fps,
        prysm_webos_protocol::PixelFormat::Rgb.into(),
        if config.compression == Compression::H264 {
            H264_BITRATE
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
