//! Per-frame cost of the session's work on synthetic NV12 frames, the format
//! of the webOS video tap:
//! `cargo run --release --manifest-path v1/Cargo.toml -p prysm-runtime --example frame_cost [frames]`.
//! On the TV: build it with `--target armv7-unknown-linux-gnueabi --profile webos`
//! (after sourcing `v1/packaging/webos/env.sh`) and run the binary there.
//!
//! - `capture`: copy a strided driver buffer into reused storage, then into a
//!   pooled frame (what `capture-webos` does on its own thread).
//! - `process`: `PrysmProcessor::process_frame_at` with the default settings
//!   (black-bar removal, smoothing), as the session thread runs it; `no bars`
//!   without black-bar removal.
//! - `preview`: one 160x90 `sample_rgb`, run by a surface only while a
//!   preview is watched.
//!
//! Times are per frame.
use prysm_core::{Config, Frame, FramePool, PixelFormat, Viewport, YuvRange, YuvStandardMatrix};
use prysm_processor::PrysmProcessor;
use std::hint::black_box;
use std::time::{Duration, Instant};

const FORMAT: PixelFormat = PixelFormat::NV12 {
    range: YuvRange::Limited,
    matrix: YuvStandardMatrix::Bt709,
};

/// A moving picture with letterbox bars, so black-bar detection does real work.
fn driver_planes(width: usize, height: usize, stride: usize, seed: usize) -> Vec<u8> {
    let bars = height / 8;
    let mut data = vec![16_u8; stride * height * 3 / 2];
    for y in bars..height - bars {
        for x in 0..width {
            data[y * stride + x] = 16 + u8::try_from((x * 3 + y * 5 + seed * 7) % 219).unwrap();
        }
    }
    let uv = stride * height;
    for y in bars / 2..(height - bars) / 2 {
        for x in 0..width {
            data[uv + y * stride + x] = 16 + u8::try_from((x * 11 + y * 3 + seed) % 224).unwrap();
        }
    }
    for y in (0..bars / 2).chain((height - bars) / 2..height / 2) {
        data[uv + y * stride..uv + y * stride + width].fill(128);
    }
    data
}

/// Driver rows into reused storage, then into a pooled frame.
fn capture(
    driver: &[u8],
    (width, height, stride): (usize, usize, usize),
    staging: &mut Vec<u8>,
    pool: &mut FramePool,
) -> Frame {
    staging.clear();
    for row in driver.chunks(stride).take(height * 3 / 2) {
        staging.extend_from_slice(&row[..width]);
    }
    let size = (
        u32::try_from(width).unwrap(),
        u32::try_from(height).unwrap(),
    );
    let mut frame = pool.acquire(size.0, size.1, FORMAT).unwrap();
    frame.as_mut_slice().copy_from_slice(staging);
    frame.finish()
}

/// Mean time of `frames` runs after one warm-up run.
fn time(frames: u32, mut run: impl FnMut(u32)) -> Duration {
    run(0);
    let start = Instant::now();
    for i in 1..=frames {
        run(i);
    }
    start.elapsed() / frames
}

fn main() {
    let count: u32 = std::env::args()
        .nth(1)
        .map_or(120, |n| n.parse().expect("frame count"));
    println!(
        "{:<10} {:>9} {:>9} {:>9} {:>9}",
        "size", "capture", "process", "no bars", "preview"
    );
    for (width, height) in [(640_usize, 360), (960, 540), (1280, 720)] {
        let geometry = (width, height, width.next_multiple_of(256));
        let drivers: Vec<_> = (0..8)
            .map(|seed| driver_planes(width, height, geometry.2, seed))
            .collect();
        let (mut staging, mut frames_pool) = (Vec::new(), FramePool::default());
        let frames: Vec<_> = drivers
            .iter()
            .map(|driver| capture(driver, geometry, &mut staging, &mut FramePool::default()))
            .collect();
        let capture = time(count, |i| {
            let driver = &drivers[i as usize % 8];
            black_box(capture(driver, geometry, &mut staging, &mut frames_pool));
        });
        let clock = Instant::now();
        let process = |config: &Config| {
            let mut processor = PrysmProcessor::new(config);
            time(count, |i| {
                let now = clock + Duration::from_secs_f64(f64::from(i) / 30.0);
                let frame = frames[i as usize % 8].clone();
                black_box(processor.process_frame_at(frame, now));
            })
        };
        let serial = process(&Config::default());
        let no_bars = process(&Config {
            remove_black_bars: false,
            ..Config::default()
        });
        let full = Viewport::full_frame(frames[0].width(), frames[0].height());
        let preview = time(count, |i| {
            black_box(prysm_runtime::sample_rgb(&frames[i as usize % 8], full, 160, 90).unwrap());
        });
        println!(
            "{:<10} {:>9.2?} {:>9.2?} {:>9.2?} {:>9.2?}",
            format!("{width}x{height}"),
            capture,
            serial,
            no_bars,
            preview
        );
    }
}
