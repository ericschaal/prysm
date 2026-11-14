//! Rough per-frame pipeline cost: `cargo run --release -p prysm-processor --example bench`

use prysm_core::{ColorFocusPercent, Config, Frame, PixelFormat, SpatialReachPercent};
use prysm_processor::PrysmProcessor;
use std::time::{Duration, Instant};

fn yuyv_frame(width: u32, height: u32, seed: u8) -> Frame {
    let mut data = vec![128u8; (width * height * 2) as usize];
    for (i, byte) in data.iter_mut().enumerate().step_by(2) {
        *byte = ((i / 7).to_le_bytes()[0]).wrapping_add(seed); // luma varies per frame
    }
    Frame::new(
        data,
        width,
        height,
        PixelFormat::YUYV {
            range: prysm_core::YuvRange::Full,
            matrix: prysm_core::YuvStandardMatrix::Bt601,
        },
    )
    .unwrap()
}

fn bench(label: &str, width: u32, height: u32, vary: bool) {
    let per_frame = per_frame(width, height, vary, &Config::default());
    println!("{label}: {per_frame:?}/frame");
}

fn per_frame(width: u32, height: u32, vary: bool, config: &Config) -> Duration {
    let mut processor = PrysmProcessor::new(config);
    let frames: Vec<Frame> = (0..120)
        .map(|i| yuyv_frame(width, height, if vary { i } else { 0 }))
        .collect();

    // Warm up
    let clock = Instant::now();
    processor.process_frame_at(frames[0].clone(), clock);

    let start = Instant::now();
    for (frame, step) in frames.iter().zip(1..) {
        let now = clock + Duration::from_secs_f64(f64::from(step) / 30.0);
        std::hint::black_box(processor.process_frame_at(frame.clone(), now));
    }
    start.elapsed() / u32::try_from(frames.len()).unwrap()
}

/// Colour focus 0 against 100 at 640x360: each the fastest of five
/// interleaved runs, so a busy moment does not land on one side only.
fn bench_color_focus(reach: u8) {
    let plain = Config {
        spatial_reach_percent: SpatialReachPercent(reach),
        ..Config::default()
    };
    let focused = Config {
        color_focus_percent: ColorFocusPercent(100),
        ..plain
    };
    let (mut off, mut on) = (Duration::MAX, Duration::MAX);
    for _ in 0..5 {
        off = off.min(per_frame(640, 360, true, &plain));
        on = on.min(per_frame(640, 360, true, &focused));
    }
    let cost = on.saturating_sub(off);
    println!(
        "640x360  edge depth {reach:>3} %: colour focus 0 {off:?}, 100 {on:?}, cost {cost:?}/frame"
    );
}

/// Isolate the reusable SIMD conversion from the rest of the pipeline.
fn bench_full_decode() {
    let frame = yuyv_frame(1920, 1080, 0);
    let mut rgb = Vec::new();
    let start = Instant::now();
    let iterations = 120;
    for _ in 0..iterations {
        prysm_core::yuyv::yuyv_to_rgb_into(&frame, &mut rgb).unwrap();
        std::hint::black_box(&rgb);
    }
    let per_frame = start.elapsed() / iterations;
    println!("1920x1080 full decode (reference): {per_frame:?}/frame");
}

fn main() {
    bench("640x360  changing frames", 640, 360, true);
    bench("640x360  static frames   ", 640, 360, false);
    bench("1920x1080 changing frames", 1920, 1080, true);
    bench_full_decode();
    // Colour focus costs the same at any focus above 0; the default edge
    // depth, and the deepest, where its near field is largest.
    bench_color_focus(10);
    bench_color_focus(100);
}
