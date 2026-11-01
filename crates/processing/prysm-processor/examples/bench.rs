//! Rough per-frame pipeline cost: `cargo run --release -p prysm-processor --example bench`

use prysm_core::{Config, Frame, PixelFormat};
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
    let config = Config::default();
    let mut processor = PrysmProcessor::new(&config);
    let frames: Vec<Frame> = (0..120)
        .map(|i| yuyv_frame(width, height, if vary { i } else { 0 }))
        .collect();

    // Warm up
    let clock = Instant::now();
    processor.process_frame_at(frames[0].clone(), clock);

    let start = Instant::now();
    for (frame, step) in frames.iter().zip(1..) {
        let now = clock + Duration::from_secs_f64(f64::from(step) / 30.0);
        processor.process_frame_at(frame.clone(), now);
    }
    let per_frame = start.elapsed() / u32::try_from(frames.len()).unwrap();
    println!("{label}: {per_frame:?}/frame");
}

/// Reference: what the old pipeline paid per frame just to decode 1080p
/// to an RGB buffer before sampling anything.
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
}
