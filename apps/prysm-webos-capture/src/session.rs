//! Coordinate connection negotiation, frame capture, transmission, and pacing.
use crate::{
    capture::{Capture, FrameCapture},
    config::{CaptureOptions, Compression, Config, Source},
    encoder::{Encoder, H264_BITRATE},
    events::Events,
    transport::Sender,
};
use anyhow::Result;
use prysm_metrics::{PipelineEvent, Stage};
use prysm_webos_protocol::{ControlMessage, worker::VideoUpdate};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// One negotiated receiver connection and its native capture resources.
pub struct Session<V: FrameCapture> {
    capture: Capture<V>,
    sender: Sender,
    state: State,
    compression: Compression,
}

enum State {
    Waiting(ControlMessage),
    Capturing(CaptureOptions),
}

impl<V: FrameCapture> Session<V> {
    /// Connect before acquiring native capture resources.
    pub fn connect(config: &Config) -> Result<Self> {
        let (sender, message) = Sender::connect(&config.instance_url)?;
        Ok(Self {
            capture: Capture::new(config),
            sender,
            state: State::Waiting(message),
            compression: config.compression,
        })
    }

    pub fn busy(&self) -> bool {
        matches!(self.state, State::Waiting(ControlMessage::Busy))
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

    pub fn configure(&mut self, events: &mut Events) -> Result<()> {
        let settings = self.settings()?;
        self.capture
            .configure(settings.fps, [settings.width, settings.height], events)
    }

    /// Native resources remain owned by the caller until the session is dropped.
    pub fn run(
        &mut self,
        stop: &AtomicBool,
        open_video: &mut impl FnMut(&CaptureOptions) -> Result<V>,
        readiness: &mut impl FnMut() -> Result<VideoUpdate>,
        events: &mut Events,
    ) -> Result<()> {
        let settings = self.settings()?;
        let mut encoder = Encoder::new(self.compression, settings.fps, H264_BITRATE)?;
        let result = (|| {
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
                if let Some(frame) = self.capture.capture(open_video, events)? {
                    let hardware = encoder.hardware_active();
                    if hardware {
                        events.send(&crate::events::Event::EncoderBusy)?;
                    }
                    let packet = encoder.encode(frame.header, frame.pixels, frame.content);
                    if hardware {
                        events.send(&crate::events::Event::EncoderIdle)?;
                    }
                    let result = packet.and_then(|packet| match packet {
                        Some(packet) => self.sender.send(packet, packet.content),
                        None => Ok(()),
                    });
                    if let Err(error) = result {
                        self.sender.disconnect();
                        tracing::warn!(error = %format!("{error:#}"), "frame send failed");
                        if error
                            .downcast_ref::<crate::encoder::HardwareFailed>()
                            .is_some()
                        {
                            return Err(error);
                        }
                        return Ok(());
                    }
                }
                drop(work);
                pace(&mut next, settings.fps);
            }
            Ok(())
        })();
        self.sender.disconnect();
        let hardware = encoder.hardware_active();
        if hardware {
            events.send(&crate::events::Event::EncoderBusy)?;
        }
        drop(encoder);
        if hardware {
            events.send(&crate::events::Event::EncoderIdle)?;
        }
        result
    }
}

#[cfg(test)]
pub(crate) fn capture_loop<V: FrameCapture>(
    config: &Config,
    stop: &AtomicBool,
    mut open_video: impl FnMut(&CaptureOptions) -> Result<V>,
) -> Result<()> {
    let mut session = Session::connect(config)?;
    if session.wait_for_capture(config, stop)? {
        session.configure(&mut Events::disabled())?;
        session.run(
            stop,
            &mut open_video,
            &mut || {
                Ok(VideoUpdate {
                    availability: prysm_webos_protocol::worker::VideoAvailability::Unknown,
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
            Source::Both => prysm_metrics::CaptureSource::Both,
        },
        match config.compression {
            Compression::None => prysm_metrics::Codec::None,
            Compression::Lz4 => prysm_metrics::Codec::Lz4,
            Compression::H264Software | Compression::H264Hardware => prysm_metrics::Codec::H264,
        },
        width,
        height,
        fps,
        if matches!(
            config.compression,
            Compression::H264Software | Compression::H264Hardware
        ) {
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
