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
use prysm_metrics::{FrameStage, PipelineEvent, Stage, UiCaptureEvent, VideoCaptureEvent};
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
    let _metrics = cli.metrics_listen.and_then(|address| {
        match prysm_metrics::start(address, prysm_metrics::Role::Capturer) {
            Ok(exporter) => {
                eprintln!("Metrics listening on {}", exporter.address());
                Some(exporter)
            }
            Err(error) => {
                eprintln!("Metrics disabled: {error:#}");
                None
            }
        }
    });
    prysm_metrics::capture_info(
        cli.source.into(),
        cli.compression.into(),
        cli.capture.width,
        cli.capture.height,
        cli.capture.fps,
        prysm_webos_protocol::PixelFormat::from(cli.composition_format).into(),
        if cli.compression == cli::Compression::H264 {
            cli.h264_bitrate
        } else {
            0
        },
    );
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
    prysm_metrics::backend_info(
        prysm_metrics::Layer::Ui,
        ui.as_ref()
            .map_or(prysm_metrics::Backend::Disabled, Ui::metrics_backend),
    );
    capture_loop(
        cli,
        stop,
        Video::open,
        ui.as_mut().map(|ui| {
            move |width, height, output: &mut pixels::Bgra8Pixels| ui.capture(width, height, output)
        }),
    )
}

fn capture_loop(
    cli: &Cli,
    stop: &AtomicBool,
    mut open_video: impl FnMut(&CaptureOptions) -> Result<Video>,
    mut capture_ui: Option<impl FnMut(u32, u32, &mut pixels::Bgra8Pixels) -> Result<()>>,
) -> Result<()> {
    let options = &cli.capture;
    prysm_metrics::backend_info(
        prysm_metrics::Layer::Video,
        prysm_metrics::Backend::Disabled,
    );
    let mut video = None;
    let mut sender = transport::Sender::new(cli.receiver, cli.compression);
    sender.h264_bitrate = cli.h264_bitrate;
    sender.fps = cli.capture.fps;
    let mut native = Vec::new();
    let mut composition = pixels::CompositionBuffer::default();
    let mut bgra = pixels::Bgra8Pixels::default();
    let period = Duration::from_secs_f64(1.0 / f64::from(options.fps));
    let mut next = Instant::now();
    let mut retry = next;
    let mut ui_warning = next;
    eprintln!(
        "Capturing {}x{} at {} fps for {} (compression: {:?}, composition format: {:?})",
        options.width,
        options.height,
        options.fps,
        cli.receiver,
        cli.compression,
        cli.composition_format
    );
    while !stop.load(Ordering::Relaxed) {
        let work = prysm_metrics::timer(Stage::CaptureIteration);
        if cli.source.includes_video() && video.is_none() && Instant::now() >= retry {
            prysm_metrics::event(VideoCaptureEvent::InitAttempt);
            match open_video(options) {
                Ok(capture) => {
                    prysm_metrics::backend_info(
                        prysm_metrics::Layer::Video,
                        capture.metrics_backend(),
                    );
                    video = Some(capture);
                }
                Err(error) => {
                    prysm_metrics::event(VideoCaptureEvent::InitFailed);
                    eprintln!("video unavailable; retrying: {error:#}");
                    retry = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        let mut frame = None;
        if let Some(capture) = &mut video {
            let capture_time = prysm_metrics::timer(Stage::VideoAcquire);
            let result = capture.capture(&mut native);
            drop(capture_time);
            match result {
                Ok(Some(header)) => {
                    prysm_metrics::frame(FrameStage::VideoCaptured, header.format.into());
                    frame = Some(header);
                }
                Ok(None) => prysm_metrics::event(VideoCaptureEvent::Duplicate),
                Err(error) => {
                    prysm_metrics::event(VideoCaptureEvent::AcquireFailed);
                    eprintln!("video interrupted; restarting: {error:#}");
                    video = None;
                    prysm_metrics::backend_info(
                        prysm_metrics::Layer::Video,
                        prysm_metrics::Backend::Disabled,
                    );
                    retry = Instant::now() + Duration::from_secs(1);
                }
            }
        }
        let mut payload = native.as_slice();
        let mut content = prysm_metrics::Content::Video;
        if sender.ready()
            && let Some(capture_ui) = &mut capture_ui
        {
            let (width, height) =
                frame.map_or((options.width, options.height), |h| (h.width, h.height));
            let ui_time = prysm_metrics::timer(Stage::UiAcquire);
            let result = capture_ui(width, height, &mut bgra);
            drop(ui_time);
            if result.is_err() {
                prysm_metrics::event(UiCaptureEvent::AcquireFailed);
            }
            match result {
                Ok(()) => {
                    prysm_metrics::frame(FrameStage::UiCaptured, (&bgra).into());
                    content = if frame.is_some() {
                        prysm_metrics::Content::Blended
                    } else {
                        prysm_metrics::Content::Ui
                    };
                    let composed = composition.compose(
                        width,
                        height,
                        frame.map(|header| (header, native.as_slice())),
                        &bgra,
                        cli.composition_format,
                    )?;
                    if content == prysm_metrics::Content::Blended {
                        prysm_metrics::frame(FrameStage::Composited, composed.header.format.into());
                    }
                    frame = Some(composed.header);
                    payload = composed.pixels;
                }
                Err(error) if Instant::now() >= ui_warning => {
                    eprintln!("UI frame skipped: {error:#}");
                    ui_warning = Instant::now() + Duration::from_secs(1);
                }
                Err(_) => {}
            }
        }
        if let Some(header) = frame {
            if let Err(error) = sender.send(header, payload, content) {
                eprintln!("{error:#}");
            }
        }
        drop(work);
        // Skip missed ticks instead of building up latency or sending bursts.
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            prysm_metrics::event(PipelineEvent::MissedTick);
            next = now;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_webos_protocol::{HEADER_LEN, Header, PixelFormat};
    use std::{cell::Cell, io::Read, net::TcpListener};

    #[test]
    fn nv12_composition_reaches_the_socket_with_raw_or_lz4_payload() {
        for compression in ["none", "lz4"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let cli = Cli::try_parse_from([
                "capture",
                &listener.local_addr().unwrap().to_string(),
                "--source",
                "ui",
                "--width",
                "16",
                "--height",
                "8",
                "--composition-format",
                "nv12",
                "--compression",
                compression,
            ])
            .unwrap();
            let stop = AtomicBool::new(false);
            capture_loop(
                &cli,
                &stop,
                |_| panic!("UI-only opened video"),
                Some(|_, _, bgra: &mut pixels::Bgra8Pixels| {
                    *bgra.bytes_mut() = [0, 0, 255, 255].repeat(16 * 8);
                    stop.store(true, Ordering::Relaxed);
                    Ok(())
                }),
            )
            .unwrap();
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut bytes = [0; HEADER_LEN];
            socket.read_exact(&mut bytes).unwrap();
            let header = Header::decode(&bytes).unwrap();
            assert_eq!(header.format, PixelFormat::Nv12);
            assert_eq!(header.raw_len().unwrap(), 16 * 8 * 3 / 2);
            assert_eq!(
                header.codec == prysm_webos_protocol::Codec::Lz4,
                compression == "lz4"
            );
            let mut payload = vec![0; header.payload_len];
            socket.read_exact(&mut payload).unwrap();
            let raw = if header.codec == prysm_webos_protocol::Codec::Lz4 {
                let mut decoded = vec![0; header.raw_len().unwrap()];
                prysm_webos_protocol::decompress(&payload, &mut decoded).unwrap();
                decoded
            } else {
                payload
            };
            let mut rgb = Vec::new();
            pixels::to_rgb(header, &raw, &mut rgb).unwrap();
            assert!(
                rgb.chunks_exact(3)
                    .all(|p| p[0] >= 250 && p[1] <= 4 && p[2] <= 4)
            );
        }
    }

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
            Some(|_, _, bgra: &mut pixels::Bgra8Pixels| {
                ui_frames.set(ui_frames.get() + 1);
                *bgra.bytes_mut() = [0, 0, 255, 255].repeat(4);
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
                Some(|_, _, bgra: &mut pixels::Bgra8Pixels| {
                    ui_frames += 1;
                    *bgra.bytes_mut() = [0, 0, 255, 255].repeat(4);
                    Ok(())
                }),
            )
            .unwrap();
        });
        assert_eq!(ui_frames, 1);
    }
}
