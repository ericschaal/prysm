//! Reads limited-range BT.709 NV12 frames of WIDTH x HEIGHT from stdin and
//! writes the 162 LED colors of each frame for five processors:
//! HEAD and working-tree `prysm-processor`, each without and with smoothing, and
//! a working-tree copy that reads every pixel into its capped grid ("blocks",
//! no smoothing; built by run.sh).
//!
//! Usage: ledrun WIDTH HEIGHT FPS OUT_PREFIX
//! Writes OUT_PREFIX.{head,wt}.{raw,smooth}.bin and OUT_PREFIX.blocks.raw.bin; one record per frame:
//! 162 x RGB u8 (sRGB, as sent to WLED) followed by 162 x RGB f32 LE (linear).
//! LEDs are in strip coordinates: top left-to-right (51), right top-to-bottom (31),
//! bottom left-to-right (49), left top-to-bottom (31).
//! OUT_PREFIX.viewport.txt lists the confirmed viewport of each processor per frame.

use prysm_core::{
    Config, EdgeColors, Frame, PixelFormat, SmoothingSeconds, YuvRange, YuvStandardMatrix,
};
use std::io::{BufWriter, Read, Write};
use std::time::{Duration, Instant};

const LEDS: [usize; 4] = [51, 31, 49, 31];

fn leds(edges: &EdgeColors) -> Vec<prysm_core::LinearColor> {
    let mut out = Vec::with_capacity(162);
    for (strip, count) in [&edges.top, &edges.right, &edges.bottom, &edges.left]
        .into_iter()
        .zip(LEDS)
    {
        for i in 0..count {
            out.push(strip.color_at(i, count).expect("index in range"));
        }
    }
    out
}

fn record(edges: &EdgeColors) -> Vec<u8> {
    let colors = leds(edges);
    let mut bytes = Vec::with_capacity(162 * 15);
    for c in &colors {
        let s = c.to_srgb();
        bytes.extend([s.r, s.g, s.b]);
    }
    for c in &colors {
        for v in [c.r, c.g, c.b] {
            bytes.extend(v.to_le_bytes());
        }
    }
    bytes
}

enum Processor {
    Head(prysm_processor_head::PrysmProcessor),
    Wt(prysm_processor::PrysmProcessor),
    Blocks(prysm_processor_blocks::PrysmProcessor),
}

impl Processor {
    fn run(&mut self, frame: Frame, now: Instant) -> (EdgeColors, String) {
        match self {
            Self::Head(p) => {
                let e = p.process_frame_at(frame, now);
                (e, format!("{:?}", p.viewport()))
            }
            Self::Wt(p) => {
                let e = p.process_frame_at(frame, now);
                (e, format!("{:?}", p.viewport()))
            }
            Self::Blocks(p) => {
                let e = p.process_frame_at(frame, now);
                (e, format!("{:?}", p.viewport()))
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, width, height, fps, prefix] = args.as_slice() else {
        panic!("usage: ledrun WIDTH HEIGHT FPS OUT_PREFIX");
    };
    let (width, height): (u32, u32) = (width.parse().unwrap(), height.parse().unwrap());
    let fps: f64 = fps.parse().unwrap();
    let base = Config {
        smoothing_seconds: SmoothingSeconds(0.0),
        ..Config::default() // density 128, reach 10 %, black bars removed
    };
    let smooth = Config {
        smoothing_seconds: SmoothingSeconds(0.1),
        ..base
    };
    let mut processors = [
        ("head.raw", Processor::Head(prysm_processor_head::PrysmProcessor::new(&base))),
        ("head.smooth", Processor::Head(prysm_processor_head::PrysmProcessor::new(&smooth))),
        ("wt.raw", Processor::Wt(prysm_processor::PrysmProcessor::new(&base))),
        ("wt.smooth", Processor::Wt(prysm_processor::PrysmProcessor::new(&smooth))),
        ("blocks.raw", Processor::Blocks(prysm_processor_blocks::PrysmProcessor::new(&base))),
    ];
    let mut outputs: Vec<_> = processors
        .iter()
        .map(|(name, _)| {
            BufWriter::new(std::fs::File::create(format!("{prefix}.{name}.bin")).unwrap())
        })
        .collect();
    let mut viewports =
        BufWriter::new(std::fs::File::create(format!("{prefix}.viewport.txt")).unwrap());
    let format = PixelFormat::NV12 {
        range: YuvRange::Limited,
        matrix: YuvStandardMatrix::Bt709,
    };
    let size = (width * height * 3 / 2) as usize;
    let mut stdin = std::io::stdin().lock();
    let origin = Instant::now();
    let mut index = 0u64;
    loop {
        let mut data = vec![0; size];
        if stdin.read_exact(&mut data).is_err() {
            break;
        }
        let frame = Frame::new(data, width, height, format).unwrap();
        let now = origin + Duration::from_secs_f64(index as f64 / fps);
        let mut line = format!("{index}");
        for ((_, processor), output) in processors.iter_mut().zip(&mut outputs) {
            let (edges, viewport) = processor.run(frame.clone(), now);
            output.write_all(&record(&edges)).unwrap();
            line.push('\t');
            line.push_str(&viewport);
        }
        writeln!(viewports, "{line}").unwrap();
        index += 1;
    }
    eprintln!("{prefix}: {index} frames");
}
