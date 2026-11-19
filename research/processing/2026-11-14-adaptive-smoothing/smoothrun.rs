//! Drives the real `PrysmProcessor` with synthetic, deterministic pictures and
//! timestamps and prints the tables of the smoothing study (fixed rate, scene
//! cuts, brightness ramp) as Markdown.
//!
//! Usage: smoothrun > results.md
//!
//! Pictures are 320x180 RGB24 (smoothing does not depend on the capture size),
//! black-bar removal is off (so no crop decision shifts a transition), the
//! other settings are the defaults. The 162 LEDs are read as the WLED output
//! reads them: top 51, right 31, bottom 49, left 31, `color_at(i, count)`.
//! Every distance is Oklab, in just-noticeable steps (JND = 0.02).

use prysm_core::{
    BrightnessRampSeconds, Config, CutResponsePercent, EdgeColors, Frame, LinearColor, Oklab,
    PixelFormat, SmoothingSeconds,
};
use prysm_processor::{PrysmProcessor, cut_strength};
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

/// The forest with a bright red block, a fifth of the screen wide, at `x`
/// (0 to 1) along the top edge: motion that changes one edge only.
fn local_motion(x: f32) -> Picture {
    let base = forest();
    let (left, right) = (x * 0.8, x * 0.8 + 0.2);
    base.into_iter()
        .enumerate()
        .map(|(index, pixel)| {
            let (px, py) = (
                (index % WIDTH) as f32 / WIDTH as f32,
                (index / WIDTH) as f32 / HEIGHT as f32,
            );
            if (left..right).contains(&px) && py < 0.15 {
                [230, 40, 30]
            } else {
                pixel
            }
        })
        .collect()
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

/// The processor's output for each frame.
fn edges(config: &Config, sequence: &Sequence) -> Vec<EdgeColors> {
    let mut processor = PrysmProcessor::new(config);
    let origin = Instant::now();
    sequence
        .frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let now = origin + Duration::from_secs_f64(sequence.time(index));
            processor.process_frame_at(frame.clone(), now)
        })
        .collect()
}

/// The LEDs of each frame, in Oklab.
fn run(config: &Config, sequence: &Sequence) -> Vec<Vec<Oklab>> {
    edges(config, sequence)
        .iter()
        .map(|edges| {
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

/// The cut detector on the unsmoothed input, frame to frame: how many frames
/// it fires on (strength above 0), and its largest strength.
fn detections(raw_config: &Config, sequence: &Sequence) -> (usize, f32) {
    let input = edges(raw_config, sequence);
    let strengths: Vec<f32> = input
        .windows(2)
        .map(|pair| cut_strength(&pair[0], &pair[1]))
        .collect();
    let fired = strengths.iter().filter(|&&strength| strength > 0.0).count();
    (fired, strengths.into_iter().fold(0.0, f32::max))
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

/// A one-frame flash at `at`: how much of the way to the flash the median
/// LED shows on its frame (percent, in Oklab), and seconds after it until
/// every LED is back within one step of the picture before.
fn flash_shown(leds: &[Vec<Oklab>], flash: &[Oklab], sequence: &Sequence, at: usize) -> (f32, f32) {
    let before = &leds[at - 1];
    let mut shown: Vec<f32> = (0..flash.len())
        .filter_map(|led| {
            let way = before[led].distance(flash[led]);
            (way >= JND).then(|| 100.0 * (1.0 - leds[at][led].distance(flash[led]) / way))
        })
        .collect();
    let back = (at + 1..leds.len())
        .find(|&k| (0..flash.len()).all(|led| leds[k][led].distance(before[led]) < JND))
        .unwrap_or(leds.len() - 1);
    (
        percentile(&mut shown, 0.5),
        (sequence.time(back) - sequence.time(at)) as f32,
    )
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

fn setting(seconds: f32, cut: u8, ramp: f32) -> Setting {
    let ramp_name = if ramp > 0.0 {
        format!(", ramp {ramp} s")
    } else {
        String::new()
    };
    Setting {
        name: format!("{seconds} s, cuts {cut} %{ramp_name}"),
        config: Config {
            smoothing_seconds: SmoothingSeconds(seconds),
            cut_response_percent: CutResponsePercent(cut),
            brightness_ramp_seconds: BrightnessRampSeconds(ramp),
            remove_black_bars: false,
            ..Config::default()
        },
    }
}

fn settings() -> Vec<Setting> {
    let mut all = vec![
        Setting {
            name: "off (raw)".into(),
            ..setting(0.0, 0, 0.0)
        },
        Setting {
            name: "**today: 0.1 s, cuts 0 %**".into(),
            ..setting(0.1, 0, 0.0)
        },
    ];
    for ramp in [0.0, 0.5, 1.0] {
        for cut in [0, 50, 100] {
            all.push(setting(0.5, cut, ramp));
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
        let bright = Sequence::new(fps, 3.0, |t| if t < cut_at { night() } else { flash() });
        let flash_frame = (cut_at * fps as f32).round() as usize;
        let mut index = 0;
        let one_frame = Sequence::new(fps, 3.0, |_| {
            index += 1;
            if index - 1 == flash_frame {
                flash()
            } else {
                night()
            }
        });
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
        // Across the top edge and back in 4 s.
        let local = Sequence::new(fps, 4.0, |t| local_motion(1.0 - (1.0 - t / 2.0).abs()));

        let raw = |sequence: &Sequence| run(raw_config, sequence);
        let (cut_raw, bright_raw, one_frame_raw) = (raw(&cut), raw(&bright), raw(&one_frame));
        let (fade_raw, pan_raw, local_raw) = (raw(&fade), raw(&pan), raw(&local));
        let settle = |sequence: &Sequence| sequence.index(1.0);
        let ms = |seconds: f32| seconds * 1000.0;

        let mut transitions = String::new();
        let mut motion = String::new();
        let mut grains = String::new();
        for Setting { name, config } in &settings {
            let at = cut.index(cut_at.into());
            let (cut_median, cut_worst) =
                lag(&run(config, &cut), cut_raw.last().unwrap(), &cut, at);
            let (bright_median, bright_worst) = lag(
                &run(config, &bright),
                bright_raw.last().unwrap(),
                &bright,
                at,
            );
            let (shown, back) = flash_shown(
                &run(config, &one_frame),
                &one_frame_raw[flash_frame],
                &one_frame,
                flash_frame,
            );
            writeln!(
                transitions,
                "| {name} | {:.0} / {:.0} | {:.0} / {:.0} | {:.0} / {:.0} |",
                ms(cut_median),
                ms(cut_worst),
                ms(bright_median),
                ms(bright_worst),
                shown,
                ms(back),
            )
            .unwrap();

            let fade_leds = run(config, &fade);
            let fading = fade.index(0.5)..fade.index(3.5);
            let mut fade_behind = behind(&fade_leds, &fade_raw, fading.clone());
            let mut fade_steps = changes(&fade_leds[..fading.end], fading.start);
            let pan_leds = run(config, &pan);
            let mut pan_behind = behind(&pan_leds, &pan_raw, settle(&pan)..pan.frames.len());
            let mut pan_steps = changes(&pan_leds, settle(&pan));
            let local_leds = run(config, &local);
            let mut local_behind = behind(&local_leds, &local_raw, 0..local.frames.len());
            writeln!(
                motion,
                "| {name} | {:.2} / {:.2} | {:.2} | {:.2} / {:.2} | {:.2} | {:.2} / {:.2} |",
                mean(&fade_behind),
                percentile(&mut fade_behind, 1.0),
                percentile(&mut fade_steps, 1.0),
                mean(&pan_behind),
                percentile(&mut pan_behind, 1.0),
                percentile(&mut pan_steps, 1.0),
                mean(&local_behind),
                percentile(&mut local_behind, 1.0),
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

        let mut detector = String::new();
        for (name, sequence, expected) in [
            ("Cut", &cut, "1"),
            ("Bright cut", &bright, "1"),
            ("One-frame flash", &one_frame, "2"),
            ("Pan", &pan, "0"),
            ("Fade", &fade, "0"),
            ("Grain, bright fine", &fine, "0"),
            ("Grain, bright blotchy", &blotchy, "0"),
            ("Grain, dark blotchy", &dark, "0"),
            ("Local motion", &local, "0"),
        ] {
            let (fired, strongest) = detections(raw_config, sequence);
            writeln!(
                detector,
                "| {name} | {} | {expected} | {fired} | {strongest:.2} |",
                sequence.frames.len() - 1
            )
            .unwrap();
        }

        writeln!(out, "### Transitions, {fps} fps\n").unwrap();
        writeln!(
            out,
            "| Setting | Cut: 90 % done, median / worst LED (ms) | Bright cut: 90 % done, median / worst (ms) | One-frame flash: shown (% of the way, median) / back within 1 step (ms) |"
        )
        .unwrap();
        writeln!(out, "| --- | --- | --- | --- |").unwrap();
        out.push_str(&transitions);
        writeln!(
            out,
            "\n### Motion, {fps} fps: behind the unsmoothed LEDs (steps)\n"
        )
        .unwrap();
        writeln!(
            out,
            "| Setting | Fade: behind, mean / max | Fade: largest step | Pan: behind, mean / max | Pan: largest step | Local motion: behind, mean / max |"
        )
        .unwrap();
        writeln!(out, "| --- | --- | --- | --- | --- | --- |").unwrap();
        out.push_str(&motion);
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
        writeln!(
            out,
            "\n### Cut detector, {fps} fps: frames it fires on (the same for every setting)\n"
        )
        .unwrap();
        writeln!(
            out,
            "| Sequence | Frame changes | Cuts in it | Detector fired | Largest strength |"
        )
        .unwrap();
        writeln!(out, "| --- | --- | --- | --- | --- |").unwrap();
        out.push_str(&detector);
        writeln!(out).unwrap();
    }
    print!("{out}");
}
