//! Native webOS capture sender. Hardware resources never cross threads.
#![deny(unsafe_code)]
mod cli;
#[allow(unsafe_code)]
mod native;
mod pixels;
mod transport;

use anyhow::{Result, ensure};
use clap::Parser;
use cli::{Cli, Source};
use native::{Ui, Video};
use prysm_webos_protocol::PixelFormat;
use signal_hook::consts::{SIGINT, SIGTERM};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    let cli = Cli::parse();
    ensure!(cfg!(target_os = "linux"), "capture requires webOS Linux");
    ensure!(
        nix::unistd::Uid::effective().is_root(),
        "TV capture requires root"
    );
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    run(&cli, &stop)
}

fn run(cli: &Cli, stop: &AtomicBool) -> Result<()> {
    let options = &cli.capture;
    let mut video = if cli.source.includes_video() {
        Some(Video::open(options)?)
    } else {
        None
    };
    let mut ui = if cli.source.includes_ui() {
        match Ui::open(options) {
            Ok(ui) => Some(ui),
            Err(error) if cli.source == Source::Ui => return Err(error),
            Err(error) => {
                eprintln!("UI unavailable; video only: {error:#}");
                None
            }
        }
    } else {
        None
    };
    let mut sender = transport::Sender::new(cli.receiver, cli.compression);
    let (mut native, mut rgb, mut bgra) = (Vec::new(), Vec::new(), Vec::new());
    let period = Duration::from_secs_f64(1.0 / f64::from(options.fps));
    let mut next = Instant::now();
    let mut retry = next;
    let mut last_frame = next;
    let mut ui_warning = next;
    eprintln!(
        "Capturing {}x{} at {} fps for {} (compression: {:?})",
        options.width, options.height, options.fps, cli.receiver, cli.compression
    );
    while !stop.load(Ordering::Relaxed) {
        if cli.source.includes_video() && video.is_none() && Instant::now() >= retry {
            match Video::open(options) {
                Ok(capture) => {
                    video = Some(capture);
                    last_frame = Instant::now();
                }
                Err(error) => {
                    eprintln!("video restart failed: {error:#}");
                    retry = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        let mut frame = None;
        if let Some(capture) = &mut video {
            let result = capture.capture(&mut native);
            match result {
                Ok(Some(header)) => {
                    last_frame = Instant::now();
                    frame = Some(header);
                }
                Ok(None) if last_frame.elapsed() < Duration::from_secs(2) => {}
                result => {
                    eprintln!("video interrupted; restarting: {result:?}");
                    video = None;
                    retry = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        let mut converted = false;
        if (frame.is_some() || video.is_none())
            && let Some(ui) = &mut ui
        {
            let (width, height) =
                frame.map_or((options.width, options.height), |h| (h.width, h.height));
            match ui.capture(width, height, &mut bgra) {
                Ok(()) => {
                    if let Some(header) = frame {
                        pixels::to_rgb(header, &native, &mut rgb)?;
                        pixels::blend(&mut rgb, &bgra)?;
                    } else {
                        // Keep menus visible while the video backend recovers.
                        rgb.resize(width as usize * height as usize * 3, 0);
                        yuv::bgra_to_rgb(&bgra, width * 4, &mut rgb, width * 3, width, height)?;
                    }
                    frame = Some(pixels::header(width, height, PixelFormat::Rgb)?);
                    converted = true;
                }
                Err(error) if Instant::now() >= ui_warning => {
                    eprintln!("UI frame skipped: {error:#}");
                    ui_warning = Instant::now() + Duration::from_secs(1);
                }
                Err(_) => {}
            }
        }
        if let Some(header) = frame {
            let payload = if converted { &rgb } else { &native };
            if let Err(error) = sender.send(header, payload) {
                eprintln!("{error:#}");
            }
        }
        // Skip missed ticks instead of building up latency or sending bursts.
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    Ok(())
}
