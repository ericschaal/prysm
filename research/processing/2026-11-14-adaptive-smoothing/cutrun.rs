//! Reads raw RGB24 640x360 frames on stdin; prints, per frame, the cut
//! strength between the unsmoothed LED colours of this frame and the last.
use prysm_core::{Config, Frame, PixelFormat, SmoothingSeconds};
use prysm_processor::{PrysmProcessor, cut_strength};
use std::io::Read;
use std::time::{Duration, Instant};

fn main() {
    let fps: f64 = std::env::args().nth(1).unwrap().parse().unwrap();
    let config = Config { smoothing_seconds: SmoothingSeconds(0.0), ..Config::default() };
    let mut processor = PrysmProcessor::new(&config);
    let start = Instant::now();
    let mut stdin = std::io::stdin().lock();
    let mut previous = None;
    let mut index = 0u32;
    loop {
        let mut data = vec![0u8; 640 * 360 * 3];
        if stdin.read_exact(&mut data).is_err() {
            break;
        }
        let frame = Frame::new(data, 640, 360, PixelFormat::RGB24).unwrap();
        let now = start + Duration::from_secs_f64(f64::from(index) / fps);
        let colors = processor.process_frame_at(frame, now);
        let strength = match &previous {
            Some(before) => {
                let same = |a: &prysm_core::EdgeColors, b: &prysm_core::EdgeColors| {
                    a.top.len() == b.top.len() && a.left.len() == b.left.len()
                };
                if same(before, &colors) { cut_strength(before, &colors) } else { -1.0 }
            }
            None => 0.0,
        };
        println!("{index} {strength:.3}");
        previous = Some(colors);
        index += 1;
    }
}
