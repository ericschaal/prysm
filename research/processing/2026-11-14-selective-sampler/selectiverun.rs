//! Drives the real `PrysmProcessor` with synthetic, deterministic pictures and
//! prints the flicker and effect tables of the selective-sampler study (colour
//! focus) as Markdown.
//!
//! Usage: selectiverun > tables.md
//!
//! Pictures are 640x360 RGB24, the size the accuracy tables use; black-bar
//! removal is off and the other settings are the defaults unless a table says
//! otherwise. The 162 LEDs are read as the WLED output reads them: top 51,
//! right 31, bottom 49, left 31, `color_at(i, count)`. Every distance is
//! Oklab, in just-noticeable steps (JND = 0.02).

use prysm_core::{
    ColorFocusPercent, Config, EdgeColors, Frame, LinearColor, Oklab, PixelFormat,
    SmoothingSeconds,
};
use prysm_processor::PrysmProcessor;
use std::time::{Duration, Instant};

const WIDTH: usize = 640;
const HEIGHT: usize = 360;
const LEDS: [usize; 4] = [51, 31, 49, 31];
const JND: f32 = Oklab::JUST_NOTICEABLE;
const FPS: u32 = 30;
/// Frames per grain sequence: two seconds of changes at 30 fps.
const FRAMES: usize = 61;

type Rgb = [u8; 3];
type Picture = Vec<Rgb>;

// ── Pictures ────────────────────────────────────────────────────────────────

fn picture(color: impl Fn(usize, usize) -> Rgb) -> Picture {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .map(|(x, y)| color(x, y))
        .collect()
}

const ORANGE: Rgb = [230, 80, 20];
const BLUE: Rgb = [20, 90, 230];
const GREEN: Rgb = [40, 200, 60];
const MAGENTA: Rgb = [220, 30, 200];

/// Orange left, blue right: the boundary is next to the middle LEDs of the
/// top and bottom strips.
fn opposing() -> Picture {
    picture(|x, _| if x < WIDTH / 2 { ORANGE } else { BLUE })
}

/// A magenta square at the top and a green disc at the left, on mid grey.
fn objects_on_grey() -> Picture {
    picture(|x, y| {
        let (dx, dy) = (x as f32 - 40.0, y as f32 - 180.0);
        if (280..360).contains(&x) && y < 60 {
            MAGENTA
        } else if dx * dx + dy * dy < 40.0 * 40.0 {
            GREEN
        } else {
            [128, 128, 128]
        }
    })
}

/// Which LEDs sit beside the picture's colour boundary: (strip, position
/// along it from 0 to 1).
type Beside = fn(usize, f32) -> bool;

fn beside_halves(strip: usize, position: f32) -> bool {
    (strip == 0 || strip == 2) && (0.4..0.6).contains(&position)
}

fn beside_objects(strip: usize, position: f32) -> bool {
    (strip == 0 && (0.4..0.6).contains(&position)) || (strip == 3 && (0.35..0.65).contains(&position))
}

/// SplitMix64: a fixed seed gives the same grain on every run.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in -amplitude..=amplitude.
    fn offset(&mut self, amplitude: i16) -> i16 {
        (self.next() % (2 * amplitude as u64 + 1)) as i16 - amplitude
    }
}

/// The adaptive-smoothing study's grain: one offset of up to `amplitude` code
/// values per `block` x `block` pixels, mostly luma with a third in chroma.
fn grainy(base: &Picture, seed: &mut Seeded, amplitude: i16, block: usize) -> Picture {
    let blocks_x = WIDTH.div_ceil(block);
    let offsets: Vec<[i16; 3]> = (0..blocks_x * HEIGHT.div_ceil(block))
        .map(|_| {
            let luma = seed.offset(amplitude);
            let chroma = [seed.offset(1), seed.offset(1), seed.offset(1)];
            [0, 1, 2].map(|i| luma + chroma[i] * amplitude / 3)
        })
        .collect();
    base.iter()
        .enumerate()
        .map(|(index, pixel)| {
            let (x, y) = (index % WIDTH, index / WIDTH);
            let offset = offsets[(y / block) * blocks_x + x / block];
            [0, 1, 2].map(|i| (i16::from(pixel[i]) + offset[i]).clamp(0, 255) as u8)
        })
        .collect()
}

fn frame(picture: &Picture) -> Frame {
    let data = picture.iter().flatten().copied().collect();
    Frame::new(data, WIDTH as u32, HEIGHT as u32, PixelFormat::RGB24).expect("RGB24 size")
}

// ── Running ─────────────────────────────────────────────────────────────────

fn config(focus: u8, smoothing: f32) -> Config {
    Config {
        color_focus_percent: ColorFocusPercent(focus),
        smoothing_seconds: SmoothingSeconds(smoothing),
        remove_black_bars: false,
        ..Config::default()
    }
}

/// Each LED as (strip, position along it, colour).
fn leds(edges: &EdgeColors) -> Vec<(usize, f32, LinearColor)> {
    [&edges.top, &edges.right, &edges.bottom, &edges.left]
        .into_iter()
        .zip(LEDS)
        .enumerate()
        .flat_map(|(strip, (colors, count))| {
            (0..count).map(move |i| {
                let position = (i as f32 + 0.5) / count as f32;
                (strip, position, colors.color_at(i, count).expect("index in range"))
            })
        })
        .collect()
}

fn run(config: &Config, frames: &[Frame]) -> Vec<Vec<(usize, f32, LinearColor)>> {
    let mut processor = PrysmProcessor::new(config);
    let origin = Instant::now();
    frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let now = origin + Duration::from_secs_f64(index as f64 / f64::from(FPS));
            leds(&processor.process_frame_at(frame.clone(), now))
        })
        .collect()
}

fn percentile(values: &mut [f32], share: f32) -> f32 {
    values.sort_by(f32::total_cmp);
    let index = ((values.len() - 1) as f32 * share).round() as usize;
    values[index]
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len() as f32
}

fn chroma(color: LinearColor) -> f32 {
    let Oklab { a, b, .. } = Oklab::from_linear(color);
    a.hypot(b)
}

fn steps(a: LinearColor, b: LinearColor) -> f32 {
    Oklab::from_linear(a).distance(Oklab::from_linear(b)) / JND
}

// ── Tables ──────────────────────────────────────────────────────────────────

/// Frame-to-frame LED change after the first second (smoothing settles), all
/// LEDs and those beside the boundary: (mean, p95) each.
fn flicker(leds: &[Vec<(usize, f32, LinearColor)>], beside: Beside) -> [(f32, f32); 2] {
    let (mut all, mut near) = (Vec::new(), Vec::new());
    for pair in leds[FPS as usize..].windows(2) {
        for (&(strip, position, a), &(_, _, b)) in pair[0].iter().zip(&pair[1]) {
            let change = steps(a, b);
            all.push(change);
            if beside(strip, position) {
                near.push(change);
            }
        }
    }
    [all, near].map(|mut changes| (mean(&changes), percentile(&mut changes, 0.95)))
}

fn flicker_tables() {
    let grains = [("fine (8 px)", 8), ("blotchy (32 px)", 32)];
    let pictures: [(&str, Picture, Beside); 2] = [
        ("Opposing halves", opposing(), beside_halves),
        ("Objects on grey", objects_on_grey(), beside_objects),
    ];
    for (smoothing, title) in [(0.0, "unsmoothed"), (0.1, "default smoothing 0.1 s")] {
        println!("### Grain, {title}: frame-to-frame LED change, mean / p95 (steps)\n");
        print!("| Focus |");
        for (name, ..) in &pictures {
            for (grain, _) in grains {
                print!(" {name}, {grain}: all LEDs | beside the boundary |");
            }
        }
        println!();
        println!("| ---: |{}", " --- | --- |".repeat(pictures.len() * grains.len()));
        let mut baseline = Vec::new();
        for focus in [0, 50, 100] {
            print!("| {focus} |");
            let mut cells = Vec::new();
            for (_, base, beside) in &pictures {
                for (_, block) in grains {
                    let mut seed = Seeded(block as u64);
                    let frames: Vec<Frame> =
                        (0..FRAMES).map(|_| frame(&grainy(base, &mut seed, 3, block))).collect();
                    let [all, near] = flicker(&run(&config(focus, smoothing), &frames), *beside);
                    print!(" {:.2} / {:.2} | {:.2} / {:.2} |", all.0, all.1, near.0, near.1);
                    cells.push([all.0, near.0]);
                }
            }
            println!();
            if focus == 0 {
                baseline = cells;
            } else if focus == 100 {
                let ratios: Vec<String> = cells
                    .iter()
                    .zip(&baseline)
                    .flat_map(|(cell, base)| [cell[0] / base[0], cell[1] / base[1]])
                    .map(|ratio| format!("{ratio:.2}x"))
                    .collect();
                println!("| 100 against 0, mean | {} |", ratios.join(" | "));
            }
        }
        println!();
    }
}

fn effect_table() {
    println!("### Effect: Oklab chroma of the LEDs, unsmoothed, no grain\n");
    println!("| Picture | Focus | All LEDs, mean | Beside the boundary, mean | Beside, lowest | Change from focus 0, mean / max (steps) |");
    println!("| --- | ---: | ---: | ---: | ---: | ---: |");
    let pictures: [(&str, Picture, Beside); 2] = [
        ("Opposing halves", opposing(), beside_halves),
        ("Objects on grey", objects_on_grey(), beside_objects),
    ];
    for (name, base, beside) in &pictures {
        let frames = [frame(base)];
        let plain = run(&config(0, 0.0), &frames).remove(0);
        for focus in [0, 50, 100] {
            let leds = run(&config(focus, 0.0), &frames).remove(0);
            let all: Vec<f32> = leds.iter().map(|&(_, _, color)| chroma(color)).collect();
            let mut near: Vec<f32> = leds
                .iter()
                .filter(|&&(strip, position, _)| beside(strip, position))
                .map(|&(_, _, color)| chroma(color))
                .collect();
            let changes: Vec<f32> =
                leds.iter().zip(&plain).map(|(a, b)| steps(a.2, b.2)).collect();
            println!(
                "| {name} | {focus} | {:.3} | {:.3} | {:.3} | {:.2} / {:.2} |",
                mean(&all),
                mean(&near),
                percentile(&mut near, 0.0),
                mean(&changes),
                changes.iter().copied().fold(0.0, f32::max),
            );
        }
    }
    let own = |rgb: Rgb| chroma(LinearColor::from_srgb(prysm_core::Color::new(rgb[0], rgb[1], rgb[2])));
    println!(
        "\nThe pictures' own chroma: orange {:.3}, blue {:.3}, green {:.3}, magenta {:.3}, grey 0.\n",
        own(ORANGE),
        own(BLUE),
        own(GREEN),
        own(MAGENTA)
    );
}

fn main() {
    effect_table();
    flicker_tables();
}
