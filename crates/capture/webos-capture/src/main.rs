//! Native webOS capture sender. Hardware resources never cross threads.
#![deny(unsafe_code)]
mod cli;
#[allow(unsafe_code)]
mod native;
mod pixels;
mod transport;

use anyhow::{Result, ensure};
use clap::Parser;
use cli::{CaptureOptions, Cli, Source};
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
    capture_loop(
        cli,
        stop,
        Video::open,
        ui.as_mut()
            .map(|ui| move |width, height, output: &mut Vec<u8>| ui.capture(width, height, output)),
    )
}

fn capture_loop(
    cli: &Cli,
    stop: &AtomicBool,
    mut open_video: impl FnMut(&CaptureOptions) -> Result<Video>,
    mut capture_ui: Option<impl FnMut(u32, u32, &mut Vec<u8>) -> Result<()>>,
) -> Result<()> {
    let options = &cli.capture;
    let mut video = None;
    let mut sender = transport::Sender::new(cli.receiver, cli.compression);
    let (mut native, mut rgb, mut bgra) = (Vec::new(), Vec::new(), Vec::new());
    let period = Duration::from_secs_f64(1.0 / f64::from(options.fps));
    let mut next = Instant::now();
    let mut retry = next;
    let mut ui_warning = next;
    eprintln!(
        "Capturing {}x{} at {} fps for {} (compression: {:?})",
        options.width, options.height, options.fps, cli.receiver, cli.compression
    );
    while !stop.load(Ordering::Relaxed) {
        if cli.source.includes_video() && video.is_none() && Instant::now() >= retry {
            match open_video(options) {
                Ok(capture) => video = Some(capture),
                Err(error) => {
                    eprintln!("video unavailable; retrying: {error:#}");
                    retry = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        let mut frame = None;
        if let Some(capture) = &mut video {
            match capture.capture(&mut native) {
                Ok(header) => frame = header,
                Err(error) => {
                    eprintln!("video interrupted; restarting: {error:#}");
                    video = None;
                    retry = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        let mut converted = false;
        if sender.ready()
            && let Some(capture_ui) = &mut capture_ui
        {
            let (width, height) =
                frame.map_or((options.width, options.height), |h| (h.width, h.height));
            match capture_ui(width, height, &mut bgra) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_webos_protocol::{HEADER_LEN, Header};
    use std::{cell::Cell, io::Read, net::TcpListener};

    #[test]
    fn startup_retries_while_ui_frames_continue() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cli = Cli::try_parse_from([
            "capture",
            &listener.local_addr().unwrap().to_string(),
            "--width",
            "2",
            "--height",
            "2",
        ])
        .unwrap();
        let stop = AtomicBool::new(false);
        let mut attempts = 0;
        let ui_frames = Cell::new(0);
        let start = Instant::now();
        capture_loop(
            &cli,
            &stop,
            |_| {
                attempts += 1;
                if attempts == 2 {
                    assert!(ui_frames.get() > 0);
                    stop.store(true, Ordering::Relaxed);
                }
                anyhow::bail!("driver not ready")
            },
            Some(|_, _, bgra: &mut Vec<u8>| {
                ui_frames.set(ui_frames.get() + 1);
                *bgra = [0, 0, 255, 255].repeat(4);
                Ok(())
            }),
        )
        .unwrap();
        assert_eq!(attempts, 2);
        assert!(start.elapsed() >= Duration::from_secs(1));
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut bytes = [0; HEADER_LEN];
        socket.read_exact(&mut bytes).unwrap();
        let header = Header::decode(&bytes).unwrap();
        assert_eq!(header.format, PixelFormat::Rgb);
        let mut pixels = vec![0; header.payload_len];
        socket.read_exact(&mut pixels).unwrap();
        assert_eq!(pixels, [255, 0, 0].repeat(4));
    }

    #[test]
    fn network_backoff_skips_ui_capture_and_composition() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cli = Cli::try_parse_from([
            "capture",
            &listener.local_addr().unwrap().to_string(),
            "--source",
            "ui",
            "--width",
            "2",
            "--height",
            "2",
        ])
        .unwrap();
        drop(listener);
        let stop = AtomicBool::new(false);
        let mut ui_frames = 0;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(250));
                stop.store(true, Ordering::Relaxed);
            });
            capture_loop(
                &cli,
                &stop,
                |_| panic!("UI-only opened video"),
                Some(|_, _, bgra: &mut Vec<u8>| {
                    ui_frames += 1;
                    *bgra = [0, 0, 255, 255].repeat(4);
                    Ok(())
                }),
            )
            .unwrap();
        });
        assert_eq!(ui_frames, 1);
    }
}
