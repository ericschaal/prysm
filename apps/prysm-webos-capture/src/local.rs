//! Standalone experiment: LG capture → Prysm processing → WLED DDP.
#![deny(unsafe_code)]
pub mod capture;
pub mod config;
#[allow(unsafe_code)]
pub mod events;
mod lock;
#[allow(unsafe_code)]
pub mod native;
mod pixels;
mod shutdown;

use anyhow::{Context, Result, ensure};
use prysm_contract::{Input, Output, Settings};
use prysm_core::{Frame, FramePool, PixelFormat};
use prysm_processor::PrysmProcessor;
use prysm_renderer_wled::WledRenderer;
use std::{
    fs::File,
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

fn rgb(pool: &mut FramePool, header: prysm_webos_protocol::Header, raw: &[u8]) -> Result<Frame> {
    ensure!(
        header.format == prysm_webos_protocol::PixelFormat::Nv12 && header.raw_len()? == raw.len(),
        "invalid native NV12 frame"
    );
    let (y_plane, uv_plane) = raw.split_at(header.width as usize * header.height as usize);
    let mut frame = pool.acquire(header.width, header.height, PixelFormat::RGB24)?;
    yuv::yuv_nv12_to_rgb(
        &yuv::YuvBiPlanarImage {
            y_plane,
            y_stride: header.width,
            uv_plane,
            uv_stride: header.width,
            width: header.width,
            height: header.height,
        },
        frame.as_mut_slice(),
        header.width * 3,
        yuv::YuvRange::Limited,
        yuv::YuvStandardMatrix::Bt709,
        yuv::YuvConversionMode::Balanced,
    )?;
    Ok(frame.finish())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [path] = args.as_slice() else {
        anyhow::bail!("usage: prysm-webos-local SETTINGS.json")
    };
    let settings: Settings = serde_json::from_reader(File::open(path).context("open settings")?)?;
    settings.validate()?;
    let Some(Input::Webos {
        refresh_rate_hz,
        capture_resolution,
    }) = settings.input
    else {
        anyhow::bail!("settings require a webOS input");
    };
    let output = settings.output.context("missing WLED output")?;
    let Output::Wled { leds, .. } = &output;
    let [width, height] = capture_resolution.dimensions();
    let fps = u8::try_from(refresh_rate_hz.get())?;
    let period = Duration::from_secs(1) / u32::from(fps);
    ensure!(
        cfg!(target_os = "linux") && nix::unistd::Uid::effective().is_root(),
        "TV capture requires webOS root"
    );
    let _lock = lock::capture_lock(Path::new("/tmp/prysm-capture.lock"))?;
    let (stop, _waiter) = shutdown::listen()?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();
    let mut core = settings.processing.to_core();
    core.correction = settings.calibration.correction().to_core();
    let mut processor = PrysmProcessor::new(&core);
    let mut renderer = WledRenderer::new(output.ddp_address()?)?;
    let mut pool = FramePool::default();
    let mut capture = capture::Capture::with_options(
        config::CaptureOptions { width, height, fps },
        config::Source::Both,
    );
    let mut events = events::Events::from_file(File::options().write(true).open("/dev/null")?);
    let mut frames = 0u64;
    let start = Instant::now();
    let mut report = start;
    while !stop.load(Ordering::Relaxed) {
        let tick = Instant::now();
        if let Some(frame) = capture.capture(
            &mut |opts| native::Video::open(opts, config::Source::Both),
            &mut events,
        )? {
            let rgb = rgb(&mut pool, frame.header, frame.pixels)?;
            let edges = processor.process_frame(rgb);
            renderer.render_edges(&edges, leds.counts())?;
            frames += 1;
        }
        if report.elapsed() >= Duration::from_secs(5) {
            tracing::info!(
                frames,
                elapsed_ms = start.elapsed().as_millis(),
                "local DDP output"
            );
            report = Instant::now();
        }
        std::thread::sleep(period.saturating_sub(tick.elapsed()));
    }
    tracing::info!(
        frames,
        elapsed_ms = start.elapsed().as_millis(),
        "local capture stopped"
    );
    ensure!(frames > 0, "no frames rendered");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_levels_become_black_and_white_and_truncated_frames_fail() {
        let header = pixels::header(2, 2, prysm_webos_protocol::PixelFormat::Nv12).unwrap();
        let mut pool = FramePool::default();
        let frame = rgb(&mut pool, header, &[16, 235, 235, 16, 128, 128]).unwrap();
        assert_eq!(
            frame.as_slice(),
            [0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0]
        );
        assert!(rgb(&mut pool, header, &[16; 5]).is_err());
    }
}
