//! Per-frame pipeline cost, split by stage.
//!
//! ```sh
//! cargo run --release -p prysm-processor --example bench -- --threads 3 --frames 120
//! ```
//!
//! `--threads N` gives the sampler N pool threads (1: no pool, the calling
//! thread alone); without it the processor chooses, as in the hosts.
//! `--frames N` frames are timed per table column, after a warm-up.
//! `--density N` sets the sample density (default: the hosts' 128).
//! Frames are gradients, moving patches and noise, different every frame
//! (except the static case), so no stage is skipped or cached.

use prysm_core::{
    BrightnessPercent, ColorCorrection, ColorFocusPercent, Config, DarkFloorPercent, Frame,
    PixelFormat, SampleDensity, SaturationBoostPercent, SpatialReachPercent, YuvRange,
    YuvStandardMatrix,
};
use prysm_processor::{PrysmProcessor, Stage};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

const STAGES: [(Stage, &str); 6] = [
    (Stage::BlackBars, "black bars"),
    (Stage::CellAverages, "cell averages"),
    (Stage::Projection, "projection"),
    (Stage::ColorFocus, "colour focus"),
    (Stage::Smoothing, "smoothing"),
    (Stage::Output, "output"),
];
const FOCUS: [u8; 3] = [0, 10, 100];
/// Distinct frames per case, reused in turn; frames are shared, not copied.
const DISTINCT_FRAMES: u32 = 16;
const WARM_UP_FRAMES: u32 = 10;

struct Options {
    threads: Option<NonZeroUsize>,
    frames: u32,
    density: SampleDensity,
}

fn options() -> Result<Options, String> {
    let mut options = Options {
        threads: None,
        frames: 120,
        density: SampleDensity::default(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|&value| value > 0)
                .ok_or(format!("{arg} needs a positive number"))
        };
        match arg.as_str() {
            "--threads" => options.threads = NonZeroUsize::new(value()? as usize),
            "--frames" => options.frames = value()?,
            "--density" => options.density = SampleDensity(value()? as usize),
            _ => {
                return Err(format!(
                    "unknown argument {arg}; use --threads N --frames N --density N"
                ));
            }
        }
    }
    Ok(options)
}

/// A small, seeded xorshift: the same noise on every machine.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0.to_le_bytes()[0]
    }
}

/// Picture content at `(x, y)` of a `width` x `height` frame number `seed`,
/// as 8-bit `[Y, U, V]` over the full range: diagonal gradients, two patches
/// of colour that move with `seed`, and noise.
fn content(x: u32, y: u32, width: u32, height: u32, seed: u32, noise: &mut Noise) -> [u8; 3] {
    let (u, v) = (x * 256 / width, y * 256 / height);
    let moving = seed * 7 % width;
    let in_patch = |cx: u32, cy: u32| x.abs_diff(cx) < width / 8 && y.abs_diff(cy) < height / 6;
    let (luma, cb, cr) = if in_patch(moving, height / 3) {
        (180, 60, 200) // warm patch
    } else if in_patch(width - 1 - moving, height * 2 / 3) {
        (90, 200, 70) // cool patch
    } else {
        ((u + v) / 2, u, 255 - v)
    };
    let mut jitter =
        |level: u32| u8::try_from((level + u32::from(noise.next() & 15)).min(255)).unwrap();
    [jitter(luma), jitter(cb), jitter(cr)]
}

/// Full-range code `level` (0..=255) as a limited-range code (`low`..=`high`).
fn limited(level: u8, low: u32, high: u32) -> u8 {
    u8::try_from(low + u32::from(level) * (high - low) / 255).unwrap()
}

fn nv12_frame(width: u32, height: u32, seed: u32) -> Frame {
    let mut noise = Noise(0x2545_F491 ^ seed.wrapping_mul(0x9E37_79B9));
    let (w, h) = (width as usize, height as usize);
    let mut data = vec![0u8; w * h * 3 / 2];
    let (luma, chroma) = data.split_at_mut(w * h);
    for y in 0..height {
        for x in 0..width {
            let [level, cb, cr] = content(x, y, width, height, seed, &mut noise);
            luma[y as usize * w + x as usize] = limited(level, 16, 235);
            if x % 2 == 0 && y % 2 == 0 {
                let index = (y as usize / 2) * w + x as usize;
                chroma[index] = limited(cb, 16, 240);
                chroma[index + 1] = limited(cr, 16, 240);
            }
        }
    }
    let format = PixelFormat::NV12 {
        range: YuvRange::Limited,
        matrix: YuvStandardMatrix::Bt709,
    };
    Frame::new(data, width, height, format).unwrap()
}

fn yuyv_frame(width: u32, height: u32, seed: u32) -> Frame {
    let mut noise = Noise(0x2545_F491 ^ seed.wrapping_mul(0x9E37_79B9));
    let mut data = Vec::with_capacity((width * height * 2) as usize);
    for y in 0..height {
        for x in (0..width).step_by(2) {
            let [y0, cb, cr] = content(x, y, width, height, seed, &mut noise);
            let [y1, ..] = content(x + 1, y, width, height, seed, &mut noise);
            data.extend([y0, cb, y1, cr]);
        }
    }
    let format = PixelFormat::YUYV {
        range: YuvRange::Full,
        matrix: YuvStandardMatrix::Bt601,
    };
    Frame::new(data, width, height, format).unwrap()
}

struct Case {
    label: &'static str,
    frames: Vec<Frame>,
}

impl Case {
    fn new(label: &'static str, changing: bool, frame: impl Fn(u32) -> Frame) -> Self {
        let distinct = if changing { DISTINCT_FRAMES } else { 1 };
        Self {
            label,
            frames: (0..distinct).map(frame).collect(),
        }
    }
}

/// Mean time per frame of each stage, and of the whole call.
#[derive(Default)]
struct Timing {
    stages: [Duration; STAGES.len()],
    total: Duration,
}

fn measure(case: &Case, config: &Config, options: &Options) -> Timing {
    let mut processor = match options.threads {
        Some(threads) => PrysmProcessor::with_threads(config, threads),
        None => PrysmProcessor::new(config),
    };
    let clock = Instant::now();
    let frames = case.frames.iter().cycle().zip(0..);
    let mut timing = Timing::default();
    for (frame, index) in frames.take((WARM_UP_FRAMES + options.frames) as usize) {
        let now = clock + Duration::from_secs_f64(f64::from(index) / 30.0);
        let mut stages = [Duration::ZERO; STAGES.len()];
        let start = Instant::now();
        let mut last = start;
        let colors = processor.process_frame_staged(frame.clone(), now, |stage| {
            let time = Instant::now();
            if let Some(slot) = STAGES.iter().position(|&(known, _)| known == stage) {
                stages[slot] += time - last;
            }
            last = time;
        });
        let total = start.elapsed();
        std::hint::black_box(colors);
        if index >= WARM_UP_FRAMES {
            timing.total += total;
            for (sum, stage) in timing.stages.iter_mut().zip(stages) {
                *sum += stage;
            }
        }
    }
    timing.total /= options.frames;
    for stage in &mut timing.stages {
        *stage /= options.frames;
    }
    timing
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn print_row(label: &str, cells: impl Iterator<Item = Option<(Duration, Duration)>>) {
    let mut line = format!("  {label:<14}");
    for cell in cells {
        line += &match cell {
            Some((time, total)) if !time.is_zero() => format!(
                "{:>9.3} {:>5.1} %",
                milliseconds(time),
                100.0 * time.as_secs_f64() / total.as_secs_f64().max(f64::MIN_POSITIVE)
            ),
            _ => format!("{:>9} {:>5}  ", "-", ""),
        };
    }
    println!("{line}");
}

fn report(case: &Case, base: &Config, options: &Options) {
    let timings: Vec<Timing> = FOCUS
        .iter()
        .map(|&focus| {
            let config = Config {
                color_focus_percent: ColorFocusPercent(focus),
                ..*base
            };
            measure(case, &config, options)
        })
        .collect();
    println!("{}", case.label);
    let header: String = FOCUS
        .iter()
        .map(|focus| format!("{:>17}", format!("focus {focus} %")))
        .collect();
    println!("  {:<14}{header}", "ms/frame, %");
    for (slot, (_, name)) in STAGES.iter().enumerate() {
        print_row(
            name,
            timings.iter().map(|t| Some((t.stages[slot], t.total))),
        );
    }
    print_row(
        "other",
        timings.iter().map(|t| {
            let staged: Duration = t.stages.iter().sum();
            Some((t.total.saturating_sub(staged), t.total))
        }),
    );
    print_row("total", timings.iter().map(|t| Some((t.total, t.total))));
    println!();
}

fn main() {
    let options = match options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let threads = options
        .threads
        .map_or_else(|| "automatic".to_owned(), |threads| threads.to_string());
    println!(
        "prysm-processor bench: sampler threads {threads}, {cores} cores, {} frames per column after {WARM_UP_FRAMES} warm-up",
        options.frames
    );
    // Every stage on, with the output stage away from neutral so it runs;
    // otherwise the hosts' defaults (edge depth 10 %, smoothing 0.1 s).
    let base = Config {
        saturation_boost_percent: SaturationBoostPercent(20),
        dark_floor_percent: DarkFloorPercent(2),
        correction: ColorCorrection::new(10, -5, 5).expect("a valid correction"),
        brightness_percent: BrightnessPercent(90),
        sample_density: options.density,
        ..Config::default()
    };
    println!(
        "Settings: black bars on, density {}, edge depth {} %, smoothing {} s, boost 20 %, floor 2 %, calibration on, brightness 90 %",
        base.sample_density.0, base.spatial_reach_percent.0, base.smoothing_seconds.0
    );
    println!("'-': the stage did not run. 'other': the call outside the stages.\n");

    let cases = [
        Case::new("NV12 limited BT.709 640x360, changing", true, |seed| {
            nv12_frame(640, 360, seed)
        }),
        Case::new("NV12 limited BT.709 1280x720, changing", true, |seed| {
            nv12_frame(1280, 720, seed)
        }),
        Case::new("YUYV full BT.601 640x360, changing", true, |seed| {
            yuyv_frame(640, 360, seed)
        }),
        Case::new("YUYV full BT.601 640x360, static", false, |seed| {
            yuyv_frame(640, 360, seed)
        }),
        Case::new("YUYV full BT.601 1920x1080, changing", true, |seed| {
            yuyv_frame(1920, 1080, seed)
        }),
    ];
    for case in &cases {
        report(case, &base, &options);
    }
    // Colour focus costs most at the deepest edge, where its near field is largest.
    let deep = Config {
        spatial_reach_percent: SpatialReachPercent(100),
        ..base
    };
    let deep_case = Case::new("NV12 640x360, changing, edge depth 100 %", true, |seed| {
        nv12_frame(640, 360, seed)
    });
    report(&deep_case, &deep, &options);
}
