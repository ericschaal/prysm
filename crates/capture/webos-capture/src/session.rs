//! Coordinate connection negotiation, frame capture, transmission, and pacing.
use crate::{
    capture::{self, Capture},
    cli::{CaptureOptions, Cli, Compression},
    native::Video,
    pixels::Bgra8Pixels,
    transport::Sender,
};
use anyhow::Result;
use prysm_metrics::{PipelineEvent, Stage};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub fn run(cli: &Cli, stop: &AtomicBool) -> Result<()> {
    let mut ui = capture::open_ui(cli)?;
    capture_loop(
        cli,
        stop,
        Video::open,
        ui.as_mut().map(|ui| {
            move |width, height, output: &mut Bgra8Pixels| ui.capture(width, height, output)
        }),
    )
}

pub(crate) fn capture_loop(
    cli: &Cli,
    stop: &AtomicBool,
    mut open_video: impl FnMut(&CaptureOptions) -> Result<Video>,
    mut capture_ui: Option<impl FnMut(u32, u32, &mut Bgra8Pixels) -> Result<()>>,
) -> Result<()> {
    let mut capture = Capture::new(cli);
    let mut sender = Sender::new(cli.receiver, cli.compression);
    sender.h264_bitrate = cli.h264_bitrate;
    let mut next = Instant::now();
    tracing::info!(
        width = cli.capture.width,
        height = cli.capture.height,
        receiver = %cli.receiver,
        compression = ?cli.compression,
        composition_format = ?cli.composition_format,
        "waiting for receiver capture settings"
    );
    while !stop.load(Ordering::Relaxed) {
        let was_connected = sender.connected();
        match sender.connect() {
            Ok(true) => {}
            Ok(false) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "capture handshake failed");
                capture.release_video();
                continue;
            }
        }
        if !was_connected {
            capture.configure_rate(sender.fps);
            next = Instant::now();
            record_capture_settings(cli, sender.fps);
        }
        let work = prysm_metrics::timer(Stage::CaptureIteration);
        if let Some(frame) = capture.capture(&mut open_video, &mut capture_ui)?
            && let Err(error) = sender.send(frame.header, frame.pixels, frame.content)
        {
            tracing::warn!(error = %format!("{error:#}"), "frame send failed");
            capture.release_video();
        }
        drop(work);
        pace(&mut next, sender.fps);
    }
    Ok(())
}

fn record_capture_settings(cli: &Cli, fps: u8) {
    tracing::info!(fps, "capture settings negotiated");
    prysm_metrics::capture_info(
        cli.source.into(),
        cli.compression.into(),
        cli.capture.width,
        cli.capture.height,
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
