//! On-TV worker: LG capture → Prysm processing → WLED DDP.
#![deny(unsafe_code)]
pub mod capture;
pub mod config;
pub mod control;
#[allow(unsafe_code)]
pub mod events;
mod lock;
#[cfg(feature = "embedded-prysm")]
mod metrics;
#[allow(unsafe_code)]
pub mod native;
#[cfg(all(target_arch = "arm", feature = "webos-neon"))]
#[allow(unsafe_code)]
mod nv12;
mod pixels;
#[cfg(feature = "embedded-prysm")]
mod portal;
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
    #[cfg(all(target_arch = "arm", feature = "webos-neon"))]
    if header.width.is_multiple_of(8) {
        nv12::convert(
            y_plane,
            uv_plane,
            frame.as_mut_slice(),
            header.width as usize,
        );
        return Ok(frame.finish());
    }
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

/// Settings for the standalone profiling runner.
struct Job {
    capture: config::CaptureOptions,
    source: config::Source,
    processing: prysm_core::Config,
    output: Output,
}

impl Job {
    fn from_file(path: &Path) -> Result<Self> {
        let settings: Settings =
            serde_json::from_reader(File::open(path).context("open settings")?)?;
        settings.validate()?;
        let Some(Input::Webos {
            refresh_rate_hz,
            capture_resolution,
        }) = settings.input
        else {
            anyhow::bail!("settings require a webOS input");
        };
        let [width, height] = capture_resolution.dimensions();
        let mut processing = settings.processing.to_core();
        processing.correction = settings.calibration.correction().to_core();
        Ok(Self {
            capture: config::CaptureOptions {
                width,
                height,
                fps: u8::try_from(refresh_rate_hz.get())?,
            },
            source: config::Source::Both,
            processing,
            output: settings.output.context("missing WLED output")?,
        })
    }
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(feature = "embedded-prysm")]
    if let [mode, path] = args.as_slice()
        && mode == "--supervised"
    {
        return portal::serve(Path::new(path));
    }
    let [arg] = args.as_slice() else {
        anyhow::bail!("usage: prysm-webos-local SETTINGS.json")
    };
    let mut events = events::Events::from_file(File::options().write(true).open("/dev/null")?);
    run(&Job::from_file(Path::new(arg))?, &mut events)
}

fn report<T>(result: Result<T>, events: &mut events::Events) -> Result<T> {
    if let Err(error) = &result {
        events.send(&events::Event::Failed {
            message: format!("{error:#}"),
        })?;
    }
    result
}

fn run(job: &Job, events: &mut events::Events) -> Result<()> {
    if !cfg!(target_os = "linux") || !nix::unistd::Uid::effective().is_root() {
        return report(
            Err(anyhow::anyhow!("TV capture requires webOS root")),
            events,
        );
    }
    let _lock = report(
        lock::capture_lock(Path::new("/tmp/prysm-capture.lock")),
        events,
    )?;
    let (stop, _waiter) = report(shutdown::listen(), events)?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();
    let mut pipeline = report(Pipeline::new(job), events)?;
    // Publish the original failure before any potentially blocking vendor teardown.
    let result = report(pipeline.run(job, events, &stop), events);
    let release = pipeline.capture.release_video(events);
    result.and_then(|()| report(release, events))
}

struct Pipeline {
    capture: capture::Capture<native::Video>,
    processor: PrysmProcessor,
    renderer: WledRenderer,
    leds: [usize; 4],
    pool: FramePool,
}

impl Pipeline {
    fn new(job: &Job) -> Result<Self> {
        let Output::Wled { leds, .. } = &job.output;
        Ok(Self {
            capture: capture::Capture::with_options(job.capture, job.source),
            processor: PrysmProcessor::new(&job.processing),
            renderer: WledRenderer::new(job.output.ddp_address()?)?,
            leds: leds.counts(),
            pool: FramePool::default(),
        })
    }

    fn render(&mut self, source: config::Source, events: &mut events::Events) -> Result<bool> {
        let Some(frame) = self
            .capture
            .capture(&mut |opts| native::Video::open(opts, source), events)?
        else {
            return Ok(false);
        };
        let rgb = rgb(&mut self.pool, frame.header, frame.pixels)?;
        let edges = self.processor.process_frame(rgb);
        self.renderer.render_edges(&edges, self.leds)?;
        Ok(true)
    }

    fn run(
        &mut self,
        job: &Job,
        events: &mut events::Events,
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<()> {
        let mut frames = 0u64;
        let mut published = false;
        let start = Instant::now();
        let mut report = start;
        let mut next_capture = start;
        let period = Duration::from_secs(1) / u32::from(job.capture.fps);
        while !stop.load(Ordering::Relaxed) {
            if self.render(job.source, events)? {
                frames += 1;
                if !published {
                    events.send(&events::Event::Capturing)?;
                    published = true;
                }
            } else if published && !self.capture.has_video() {
                events.send(&events::Event::CaptureIdle)?;
                published = false;
            }
            if report.elapsed() >= Duration::from_secs(5) {
                tracing::info!(
                    frames,
                    elapsed_ms = start.elapsed().as_millis(),
                    "local DDP output"
                );
                report = Instant::now();
            }
            let now = Instant::now();
            next_capture += period;
            if now.saturating_duration_since(next_capture) >= period {
                next_capture = now;
            }
            std::thread::sleep(next_capture.saturating_duration_since(now));
        }
        tracing::info!(
            frames,
            elapsed_ms = start.elapsed().as_millis(),
            "local capture stopped"
        );
        ensure!(frames > 0, "no frames rendered");
        Ok(())
    }
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
