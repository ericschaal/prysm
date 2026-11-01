//! Host microbenchmark; repeat on the TV before choosing compression.
use anyhow::Result;
use prysm_webos_protocol::{compress, decompress};
use std::{hint::black_box, time::Instant};

#[allow(
    clippy::cast_precision_loss,
    reason = "in-memory frame lengths are small enough for benchmark ratios"
)]
fn main() -> Result<()> {
    println!("NV12 LZ4: size, content, wire/raw %, encode us, decode us (500 frames)");
    for (width, height) in [(320, 180), (640, 360), (1920, 1080)] {
        let length = width * height * 3 / 2;
        for content in ["flat", "gradient", "noise"] {
            let mut random = 1_u32;
            let input: Vec<u8> = (0..length)
                .map(|index| match content {
                    "flat" => 128,
                    "gradient" => u8::try_from((index % width) * 255 / width).unwrap(),
                    _ => {
                        random ^= random << 13;
                        random ^= random >> 17;
                        random ^= random << 5;
                        random.to_le_bytes()[0]
                    }
                })
                .collect();
            let mut scratch = Vec::new();
            let mut decoded = vec![0; length];
            let compressed = compress(&input, &mut scratch)?;
            if let Some(length) = compressed {
                decompress(&scratch[..length], &mut decoded)?;
                assert_eq!(input, decoded);
            }
            let start = Instant::now();
            for _ in 0..500 {
                black_box(compress(black_box(&input), &mut scratch)?);
            }
            let encode = start.elapsed().as_secs_f64() * 1e6 / 500.0;
            let start = Instant::now();
            for _ in 0..500 {
                if let Some(length) = compressed {
                    decompress(black_box(&scratch[..length]), black_box(&mut decoded))?;
                }
            }
            let decode = start.elapsed().as_secs_f64() * 1e6 / 500.0;
            println!(
                "{width}x{height}, {content}, {:.1}%, {encode:.1}, {decode:.1}",
                compressed.unwrap_or(length) as f64 * 100.0 / length as f64
            );
        }
    }
    Ok(())
}
