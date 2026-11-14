//! Drives the real `PrysmProcessor` with synthetic, deterministic pictures and
//! timestamps and prints the tables of the adaptive-smoothing study as Markdown.
//!
//! Usage: smoothrun > results.md
//!
//! Pictures are 320x180 RGB24 (smoothing does not depend on the capture size),
//! black-bar removal is off (so no crop decision shifts a transition), the
//! other settings are the defaults. The 162 LEDs are read as the WLED output
//! reads them: top 51, right 31, bottom 49, left 31, `color_at(i, count)`.
//! Every distance is Oklab, in just-noticeable steps (JND = 0.02).

use prysm_core::{
    BrightnessSmoothing, Config, Frame, LinearColor, Oklab, PixelFormat, SmoothingResponsePercent,
    SmoothingSeconds,
};
use prysm_processor::PrysmProcessor;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

const WIDTH: usize = 320;
const HEIGHT: usize = 180;
const LEDS: [usize; 4] = [51, 31, 49, 31];
const JND: f32 = Oklab::JUST_NOTICEABLE;

type Rgb = [u8; 3];
type Picture = Vec<Rgb>;

// ── Pictures ────────────────────────────────────────────────────────────────

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [0, 1, 2].map(|i| (f32::from(a[i]) * (1.0 - t) + f32::from(b[i]) * t).round() as u8)
}

fn picture(color: impl Fn(f32, f32) -> Rgb) -> Picture {
    let mut pixels = Vec::with_capacity(WIDTH * HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.push(color(x as f32 / WIDTH as f32, y as f32 / HEIGHT as f32));
        }
    }
    pixels
}

/// Warm sunset: blue sky over an orange horizon, dark ground, a sun at the right.
fn sunset() -> Picture {
    picture(|x, y| {
        let (dx, dy) = (x - 0.78, (y - 0.48) * 9.0 / 16.0);
        if dx * dx + dy * dy < 0.006 {
            [255, 225, 150]
        } else if y < 0.55 {
            mix([40, 60, 140], [235, 140, 60], y / 0.55)
        } else {
            mix([90, 55, 35], [45, 30, 20], (y - 0.55) / 0.45)
        }
    })
}

/// Cool forest: greens, with a cyan lake at the bottom left.
fn forest() -> Picture {
    picture(|x, y| {
        if x < 0.4 && y > 0.6 {
            [40, 140, 160]
        } else {
            mix([70, 130, 80], [20, 60, 30], y)
        }
    })
}

/// A dim room at night with one lamp.
fn night() -> Picture {
    picture(|x, y| {
        if (0.15..0.3).contains(&x) && (0.3..0.6).contains(&y) {
            [70, 50, 28]
        } else {
            mix([22, 20, 28], [14, 12, 16], y)
        }
    })
}

/// An explosion filling the screen, brightest in the middle.
fn flash() -> Picture {
    picture(|x, y| {
        let r = ((x - 0.5).powi(2) + ((y - 0.5) * 9.0 / 16.0).powi(2)).sqrt();
        mix([255, 240, 200], [230, 120, 40], (r / 0.5).min(1.0))
    })
}

/// A wide, smooth hue panorama seen through a window at `offset` pixels.
fn panorama(offset: f32) -> Picture {
    picture(|x, y| {
        let hue = ((x * WIDTH as f32 + offset) / (2.0 * WIDTH as f32)).rem_euclid(1.0);
        let wave = |phase: f32| 0.5 + 0.5 * (std::f32::consts::TAU * (hue + phase)).cos();
        let level = 60.0 + 120.0 * (1.0 - 0.3 * y);
        [wave(0.0), wave(1.0 / 3.0), wave(2.0 / 3.0)].map(|w| (level * (0.35 + 0.65 * w)) as u8)
    })
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

/// Grain as compression and film noise reach the LEDs: one offset of up to
/// `amplitude` code values per `block` x `block` pixels, mostly in luma (the
/// same offset on R, G and B) with a third of it in chroma. Per-pixel noise
/// would average away inside each LED's region and show nothing; larger
/// blocks stand for the blotchy noise of flat areas in compressed video.
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

// ── Sequences ───────────────────────────────────────────────────────────────

/// Frames at a fixed rate; `at(t)` is the picture at time `t` seconds.
struct Sequence {
    frames: Vec<Frame>,
    fps: u32,
}

impl Sequence {
    fn new(fps: u32, seconds: f32, mut at: impl FnMut(f32) -> Picture) -> Self {
        let count = (seconds * fps as f32).round() as usize;
        let frames = (0..count)
            .map(|i| frame(&at(i as f32 / fps as f32)))
            .collect();
        Self { frames, fps }
    }

    fn time(&self, index: usize) -> f64 {
        index as f64 / f64::from(self.fps)
    }

    fn index(&self, seconds: f64) -> usize {
        (seconds * f64::from(self.fps)).round() as usize
    }
}

/// The LEDs of each frame, in Oklab.
fn run(config: &Config, sequence: &Sequence) -> Vec<Vec<Oklab>> {
    let mut processor = PrysmProcessor::new(config);
    let origin = Instant::now();
    sequence
        .frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let now = origin + Duration::from_secs_f64(sequence.time(index));
            let edges = processor.process_frame_at(frame.clone(), now);
            [&edges.top, &edges.right, &edges.bottom, &edges.left]
                .into_iter()
                .zip(LEDS)
                .flat_map(|(strip, count)| {
                    (0..count).map(move |i| strip.color_at(i, count).expect("index in range"))
                })
                .map(|color: LinearColor| Oklab::from(color))
                .collect()
        })
        .collect()
}

// ── Measures ────────────────────────────────────────────────────────────────

fn percentile(values: &mut [f32], share: f32) -> f32 {
    values.sort_by(f32::total_cmp);
    let index = ((values.len() - 1) as f32 * share).round() as usize;
    values[index]
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len() as f32
}

/// Seconds from the cut frame until each LED is within 10% of its starting
/// distance to the new picture (90% done); median and worst LED. LEDs whose
/// change is under one step are left out.
fn lag(leds: &[Vec<Oklab>], target: &[Oklab], sequence: &Sequence, cut: usize) -> (f32, f32) {
    let mut lags: Vec<f32> = (0..target.len())
        .filter_map(|led| {
            let start = leds[cut - 1][led].distance(target[led]);
            (start >= JND).then(|| {
                let done = (cut..leds.len())
                    .find(|&k| leds[k][led].distance(target[led]) <= 0.1 * start)
                    .unwrap_or(leds.len() - 1);
                (sequence.time(done) - sequence.time(cut)) as f32
            })
        })
        .collect();
    let worst = percentile(&mut lags, 1.0);
    (percentile(&mut lags, 0.5), worst)
}

/// Every LED's change from one frame to the next from `from` on, in steps.
fn changes(leds: &[Vec<Oklab>], from: usize) -> Vec<f32> {
    leds[from..]
        .windows(2)
        .flat_map(|pair| {
            pair[0]
                .iter()
                .zip(&pair[1])
                .map(|(a, b)| a.distance(*b) / JND)
        })
        .collect()
}

/// How far each LED is behind the unsmoothed picture in `frames`, in steps.
fn behind(leds: &[Vec<Oklab>], raw: &[Vec<Oklab>], frames: std::ops::Range<usize>) -> Vec<f32> {
    frames
        .flat_map(|k| {
            leds[k]
                .iter()
                .zip(&raw[k])
                .map(|(a, b)| a.distance(*b) / JND)
        })
        .collect()
}

// ── Study ───────────────────────────────────────────────────────────────────

struct Setting {
    name: String,
    config: Config,
}

fn setting(seconds: f32, response: u8, brightness: BrightnessSmoothing) -> Setting {
    let name = match brightness {
        BrightnessSmoothing::WithColor => "with colour",
        BrightnessSmoothing::Faster => "faster",
        BrightnessSmoothing::Limited => "limited",
    };
    Setting {
        name: format!("{seconds} s, response {response} %, {name}"),
        config: Config {
            smoothing_seconds: SmoothingSeconds(seconds),
            smoothing_response_percent: SmoothingResponsePercent(response),
            brightness_smoothing: brightness,
            remove_black_bars: false,
            ..Config::default()
        },
    }
}

fn settings() -> Vec<Setting> {
    let mut all = vec![
        Setting {
            name: "off (raw)".into(),
            ..setting(0.0, 0, BrightnessSmoothing::WithColor)
        },
        setting(0.1, 0, BrightnessSmoothing::WithColor),
    ];
    for brightness in BrightnessSmoothing::ALL {
        for response in [0, 25, 50, 100] {
            all.push(setting(0.5, response, brightness));
        }
    }
    all
}

fn main() {
    let settings = settings();
    let raw_config = &settings[0].config;
    let mut out = String::new();
    for fps in [30, 60] {
        let cut_at = 1.0;
        let cut = Sequence::new(fps, 3.0, |t| if t < cut_at { sunset() } else { forest() });
        let bang = Sequence::new(fps, 3.0, |t| if t < cut_at { night() } else { flash() });
        // Three code values of grain, fine (8 px blocks, 16 px at 640x360)
        // and blotchy (32 px), on the sunset and on the dark room.
        let grain = |base: Picture, seed: u64, block: usize| {
            let mut seed = Seeded(seed);
            Sequence::new(fps, 5.0, move |_| grainy(&base, &mut seed, 3, block))
        };
        let fine = grain(sunset(), 0x5EED, 8);
        let blotchy = grain(sunset(), 0xB10C, 32);
        let dark = grain(night(), 0xDA4C, 32);
        let (a, b) = (sunset(), forest());
        let fade = Sequence::new(fps, 4.0, |t| {
            let share = ((t - 0.5) / 3.0).clamp(0.0, 1.0);
            a.iter().zip(&b).map(|(x, y)| mix(*x, *y, share)).collect()
        });
        // Half a screen width per second.
        let pan = Sequence::new(fps, 4.0, |t| panorama(t * WIDTH as f32 / 2.0));

        let raw = |sequence: &Sequence| run(raw_config, sequence);
        let (cut_raw, bang_raw, fade_raw, pan_raw) = (raw(&cut), raw(&bang), raw(&fade), raw(&pan));
        let settle = |sequence: &Sequence| sequence.index(1.0);

        let mut transitions = String::new();
        let mut grains = String::new();
        for Setting { name, config } in &settings {
            let cut_leds = run(config, &cut);
            let (cut_median, cut_worst) = lag(
                &cut_leds,
                cut_raw.last().unwrap(),
                &cut,
                cut.index(cut_at.into()),
            );
            let bang_leds = run(config, &bang);
            let (bang_median, bang_worst) = lag(
                &bang_leds,
                bang_raw.last().unwrap(),
                &bang,
                bang.index(cut_at.into()),
            );
            let fade_leds = run(config, &fade);
            let fading = fade.index(0.5)..fade.index(3.5);
            let mut fade_behind = behind(&fade_leds, &fade_raw, fading.clone());
            let mut fade_steps = changes(&fade_leds[..fading.end], fading.start);
            let pan_leds = run(config, &pan);
            let mut pan_behind = behind(&pan_leds, &pan_raw, settle(&pan)..pan.frames.len());
            let mut pan_steps = changes(&pan_leds, settle(&pan));
            let ms = |seconds: f32| seconds * 1000.0;
            writeln!(
                transitions,
                "| {name} | {:.0} / {:.0} | {:.0} / {:.0} | {:.2} / {:.2} | {:.2} | {:.2} / {:.2} | {:.2} |",
                ms(cut_median),
                ms(cut_worst),
                ms(bang_median),
                ms(bang_worst),
                mean(&fade_behind),
                percentile(&mut fade_behind, 1.0),
                percentile(&mut fade_steps, 1.0),
                mean(&pan_behind),
                percentile(&mut pan_behind, 1.0),
                percentile(&mut pan_steps, 1.0),
            )
            .unwrap();
            write!(grains, "| {name} |").unwrap();
            for sequence in [&fine, &blotchy, &dark] {
                let mut steps = changes(&run(config, sequence), settle(sequence));
                write!(
                    grains,
                    " {:.2} / {:.2} |",
                    mean(&steps),
                    percentile(&mut steps, 0.95)
                )
                .unwrap();
            }
            writeln!(grains).unwrap();
        }
        writeln!(out, "### Transitions, {fps} fps\n").unwrap();
        writeln!(
            out,
            "| Setting | Cut: 90 % done, median / worst LED (ms) | Flash: 90 % done, median / worst (ms) | Fade: behind, mean / max (steps) | Fade: largest step | Pan: behind, mean / max (steps) | Pan: largest step |"
        )
        .unwrap();
        writeln!(out, "| --- | --- | --- | --- | --- | --- | --- |").unwrap();
        out.push_str(&transitions);
        writeln!(
            out,
            "\n### Grain, {fps} fps: frame-to-frame LED change, mean / p95 (steps)\n"
        )
        .unwrap();
        writeln!(
            out,
            "| Setting | Bright, fine | Bright, blotchy | Dark, blotchy |"
        )
        .unwrap();
        writeln!(out, "| --- | --- | --- | --- |").unwrap();
        out.push_str(&grains);
        writeln!(out).unwrap();
    }
    print!("{out}");
}
