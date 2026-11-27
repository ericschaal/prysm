use prysm_core::{
    BrightnessRampSeconds, ColorStrip, Config, EdgeColors, LinearColor, Oklab, SmoothingSeconds,
};
use std::cmp::Ordering;
use std::time::Instant;

/// A sample has changed between two inputs when it moves more than this:
/// three just-noticeable steps, above grain and above what a pan or a fade
/// moves a sample in one frame (both well under one step).
const CHANGED: f32 = 3.0 * Oklab::JUST_NOTICEABLE;

/// A sample darker than this Oklab lightness in both inputs is left out of
/// the count, so that black bars or a black border do not outvote a cut.
/// About sRGB code value 3 of 255.
const DARK: f32 = 0.1;

/// Up to this share of changed lit samples, the change is motion, not a cut:
/// a block moving along one edge changes at most that edge, about a third.
const MOTION_SHARE: f32 = 0.4;

/// From this share of changed lit samples on, the change is a full cut: a cut
/// often keeps a few samples alike (dark ground, a sky), so not all change.
const CUT_SHARE: f32 = 0.7;

/// Temporal smoothing node
///
/// Two stages, each a blend in linear light:
///
/// 1. The colour the lights should have. Each sample moves from its wanted
///    colour toward the new input by `1 - e^(-ln 20 · elapsed / smoothing
///    seconds)`, independent of the frame rate. A scene cut, detected from how
///    many lit samples of the input changed visibly since the previous input,
///    first takes `cut response · cut strength` of the way at once.
/// 2. The colour the lights show. Each sample follows its wanted colour, as
///    far as the brightness ramp lets perceived lightness change in the time
///    elapsed; without a ramp it is the wanted colour.
///
/// Every result is a mix of two colours that were themselves inputs or mixes
/// of inputs, so the lights never overshoot and never leave the inputs' range.
#[derive(Debug, Clone)]
pub struct TemporalSmoother {
    smoothing: SmoothingSeconds,
    ramp: BrightnessRampSeconds,
    cut_response: f32,
    history: Option<History>,
}

impl TemporalSmoother {
    /// `None` when the config leaves the input unchanged: no smoothing and no
    /// brightness ramp.
    pub fn new(config: &Config) -> Option<Self> {
        (config.smoothing_seconds.is_enabled() || config.brightness_ramp_seconds.is_enabled()).then(
            || Self {
                smoothing: config.smoothing_seconds,
                ramp: config.brightness_ramp_seconds,
                cut_response: config.cut_response_percent.as_ratio(),
                history: None,
            },
        )
    }

    /// The same history with the new knobs; `None` when the node is now off.
    pub fn reconfigured(self, config: &Config) -> Option<Self> {
        Some(Self {
            history: self.history,
            ..Self::new(config)?
        })
    }

    /// Forgets the history: the next input is shown as it is.
    pub fn reset(&mut self) {
        self.history = None;
    }

    pub fn process(&mut self, input: EdgeColors, now: Instant) -> EdgeColors {
        let Some(history) = &mut self.history else {
            self.history = Some(History {
                wanted: input.clone(),
                shown: input.clone(),
                input: input.clone(),
                time: now,
            });
            return input;
        };
        history.adopt_sample_counts(&input);
        let elapsed = now.saturating_duration_since(history.time);
        // Repeated or earlier times do not advance smoothing. They do not
        // replace the previous input either, so a cut that arrives at such a
        // time is still seen on the next frame.
        if elapsed.is_zero() {
            return history.shown.clone();
        }
        let elapsed = elapsed.as_secs_f64();
        let at_once = if self.cut_response > 0.0 {
            self.cut_response * cut_strength(&history.input, &input)
        } else {
            0.0
        };
        let share = with_cut(ordinary_share(self.smoothing, elapsed), at_once);
        let wanted = history.wanted.blend(&input, share);
        let shown = if self.ramp.is_enabled() {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "an infinite step deliberately leaves brightness uncapped"
            )]
            let step = (elapsed / f64::from(self.ramp.0)) as f32;
            history.shown.combine(&wanted, |shown, wanted| {
                let share = ramp_cap(shown, wanted, step);
                // Reaching the wanted colour is that colour, not a rounded blend.
                if share >= 1.0 {
                    wanted
                } else {
                    shown.blend(&wanted, share)
                }
            })
        } else {
            wanted.clone()
        };

        *history = History {
            wanted,
            shown: shown.clone(),
            input,
            time: now,
        };
        shown
    }
}

/// What the lights should show and do show, and the input that last moved them.
#[derive(Debug, Clone)]
struct History {
    /// Smoothed and cut-following, before the brightness ramp.
    wanted: EdgeColors,
    /// What the lights show: `wanted`, held back by the ramp.
    shown: EdgeColors,
    /// The reference a new input is compared with to detect a cut.
    input: EdgeColors,
    time: Instant,
}

impl History {
    /// Keeps LED positions across crop changes: the wanted and shown colours
    /// and the previous input are resampled to the new sample counts, so their
    /// colours stay where they were on the edge, and a crop alone is no cut.
    fn adopt_sample_counts(&mut self, input: &EdgeColors) {
        resample_like(&mut self.wanted, input);
        resample_like(&mut self.shown, input);
        resample_like(&mut self.input, input);
    }
}

fn resample_like(colors: &mut EdgeColors, counts: &EdgeColors) {
    for (old, new) in [
        (&mut colors.top, &counts.top),
        (&mut colors.right, &counts.right),
        (&mut colors.bottom, &counts.bottom),
        (&mut colors.left, &counts.left),
    ] {
        if old.len() != new.len() {
            *old = ColorStrip::new(old.resample(new.len())).expect("sample count is nonzero");
        }
    }
}

/// Share of an ordinary change taken after `elapsed` seconds; 95% after the
/// configured duration and the whole change when smoothing is off.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a share in 0..=1 fits f32; f64 keeps the exponent precise"
)]
fn ordinary_share(smoothing: SmoothingSeconds, elapsed: f64) -> f32 {
    if smoothing.is_enabled() {
        // exp_m1 keeps small shares when frames arrive close together.
        -(-20.0_f64.ln() * elapsed / f64::from(smoothing.0)).exp_m1() as f32
    } else {
        1.0
    }
}

/// How much of a scene cut lies between two consecutive inputs, 0 to 1: the
/// share of lit samples that changed visibly, eased from motion (0) to a full
/// cut (1). No lit sample is no cut.
///
/// Compares the inputs sample by sample; give both the same sample counts,
/// as consecutive inputs at one crop have.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "sample counts are far below 2^24, where f32 holds every integer"
)]
pub fn cut_strength(previous: &EdgeColors, input: &EdgeColors) -> f32 {
    let (mut lit, mut changed) = (0_usize, 0_usize);
    for (before, after) in sample_pairs(previous, input) {
        let (before, after) = (Oklab::from(before), Oklab::from(after));
        if before.l.max(after.l) > DARK {
            lit += 1;
            changed += usize::from(before.distance(after) > CHANGED);
        }
    }
    if lit == 0 {
        return 0.0;
    }
    smoothstep(MOTION_SHARE, CUT_SHARE, changed as f32 / lit as f32)
}

/// Each sample of `a` with the sample at the same place in `b`.
fn sample_pairs<'a>(
    a: &'a EdgeColors,
    b: &'a EdgeColors,
) -> impl Iterator<Item = (LinearColor, LinearColor)> + 'a {
    let strips =
        |colors: &'a EdgeColors| [&colors.top, &colors.right, &colors.bottom, &colors.left];
    strips(a)
        .into_iter()
        .zip(strips(b))
        .flat_map(|(a, b)| a.samples().iter().copied().zip(b.samples().iter().copied()))
}

/// 0 up to `low`, 1 from `high`, and an S-curve between (`3t² - 2t³`), so the
/// strength has neither jumps nor kinks as a change grows into a cut.
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `share` after a cut: first `at_once` of the way, then `share` of the rest,
/// `1 - (1 - at_once) · (1 - share)`. Without a cut it is exactly `share`.
fn with_cut(share: f32, at_once: f32) -> f32 {
    if at_once > 0.0 {
        1.0 - (1.0 - at_once) * (1.0 - share)
    } else {
        share
    }
}

/// The largest share of the way from `shown` to `target` (the wanted colour)
/// that changes perceived lightness by at most `step`.
///
/// Lightness is the cube root of luminance `Y`, and `Y` is linear along the
/// blend, so the share is closed-form: the allowed luminance
/// `Y' = (∛Y_shown ± step)³` (toward the target, at least 0), reached at
/// `(Y' - Y_shown) / (Y_target - Y_shown)`. Equal luminance needs no cap.
fn ramp_cap(shown: LinearColor, target: LinearColor, step: f32) -> f32 {
    let (from, to) = (shown.luminance(), target.luminance());
    let lightness = from.max(0.0).cbrt();
    let allowed = match to.partial_cmp(&from) {
        Some(Ordering::Greater) => (lightness + step).powi(3),
        Some(Ordering::Less) => (lightness - step).max(0.0).powi(3),
        Some(Ordering::Equal) | None => return 1.0,
    };
    ((allowed - from) / (to - from)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{BrightnessRampSeconds, CutResponsePercent, SmoothingSeconds};
    use std::time::Duration;

    /// Samples per edge in the test pictures: 40 in all.
    const PER_EDGE: usize = 10;
    const FRAME: f64 = 1.0 / 30.0;
    const WHITE: LinearColor = LinearColor {
        r: 1.0,
        g: 1.0,
        b: 1.0,
    };
    const GRAY: LinearColor = LinearColor {
        r: 0.2,
        g: 0.2,
        b: 0.2,
    };
    const RED: LinearColor = LinearColor {
        r: 0.8,
        g: 0.05,
        b: 0.02,
    };

    fn config(seconds: f32, cut: u8, ramp: f32) -> Config {
        Config {
            smoothing_seconds: SmoothingSeconds(seconds),
            cut_response_percent: CutResponsePercent(cut),
            brightness_ramp_seconds: BrightnessRampSeconds(ramp),
            ..Config::default()
        }
    }

    fn smoother(seconds: f32, cut: u8, ramp: f32) -> TemporalSmoother {
        TemporalSmoother::new(&config(seconds, cut, ramp)).unwrap()
    }

    /// Edges from `PER_EDGE` samples per edge, top, right, bottom, left.
    fn picture(samples: &[LinearColor]) -> EdgeColors {
        let mut strips = samples
            .chunks(samples.len() / 4)
            .map(|chunk| ColorStrip::new(chunk.to_vec()).unwrap());
        let mut next = || strips.next().unwrap();
        EdgeColors::new(next(), next(), next(), next())
    }

    fn uniform(color: LinearColor) -> EdgeColors {
        picture(&[color; 4 * PER_EDGE])
    }

    fn samples(colors: &EdgeColors) -> Vec<LinearColor> {
        sample_pairs(colors, colors)
            .map(|(color, _)| color)
            .collect()
    }

    fn lightness(color: LinearColor) -> f32 {
        color.luminance().max(0.0).cbrt()
    }

    /// Feeds `pictures` at `times` seconds after a start; returns each output.
    fn run(
        smoother: &mut TemporalSmoother,
        pictures: &[EdgeColors],
        times: &[f64],
    ) -> Vec<EdgeColors> {
        let start = Instant::now();
        pictures
            .iter()
            .zip(times)
            .map(|(picture, &time)| {
                smoother.process(picture.clone(), start + Duration::from_secs_f64(time))
            })
            .collect()
    }

    /// `first`, then `then` on every following frame at 30 fps, for `seconds`.
    fn change(
        smoother: &mut TemporalSmoother,
        first: &EdgeColors,
        then: &EdgeColors,
        seconds: f64,
    ) -> Vec<EdgeColors> {
        let count = frame_count(seconds);
        let pictures: Vec<_> = std::iter::once(first.clone())
            .chain(std::iter::repeat_n(then.clone(), count))
            .collect();
        let times: Vec<_> = (0..=count).map(time_of).collect();
        run(smoother, &pictures, &times)
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test durations are a few seconds"
    )]
    fn frame_count(seconds: f64) -> usize {
        (seconds / FRAME).round() as usize
    }

    fn time_of(frame: usize) -> f64 {
        frame as f64 * FRAME
    }

    /// SplitMix64: seeded, so every run checks the same colours.
    struct Seeded(u64);

    impl Seeded {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Uniform in 0..=1.
        #[allow(
            clippy::cast_precision_loss,
            reason = "24 random bits are exact in f32"
        )]
        fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / ((1_u64 << 24) - 1) as f32
        }

        fn color(&mut self) -> LinearColor {
            LinearColor::new(self.unit(), self.unit(), self.unit())
        }

        fn scaled(&mut self, scale: f32) -> LinearColor {
            self.color() * scale
        }

        #[allow(
            clippy::cast_possible_truncation,
            reason = "the modulus is a small slice length"
        )]
        fn pick<T: Copy>(&mut self, from: &[T]) -> T {
            from[(self.next() % from.len() as u64) as usize]
        }

        fn picture(&mut self, color: impl FnMut(&mut Self) -> LinearColor) -> EdgeColors {
            let mut color = color;
            let samples: Vec<_> = (0..4 * PER_EDGE).map(|_| color(self)).collect();
            picture(&samples)
        }
    }

    #[test]
    fn no_smoothing_and_no_ramp_builds_no_node() {
        for off in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(TemporalSmoother::new(&config(off, 100, off)).is_none());
            assert!(
                smoother(0.5, 0, 0.0)
                    .reconfigured(&config(off, 50, off))
                    .is_none()
            );
            assert!(TemporalSmoother::new(&config(off, 0, 0.5)).is_some());
            assert!(TemporalSmoother::new(&config(0.5, 0, off)).is_some());
        }
    }

    #[test]
    fn response_zero_and_no_ramp_are_the_fixed_rate_bit_for_bit() {
        let fixed = |previous: &EdgeColors, input: &EdgeColors, elapsed: f64, seconds: f32| {
            let ratio = -(-20.0_f64.ln() * elapsed / f64::from(seconds)).exp_m1();
            previous.blend(input, ratio as f32)
        };
        let mut seed = Seeded(0xF1ED);
        for seconds in [0.05, 0.1, 0.5, 2.0] {
            for ramp in [0.0, -1.0, f32::NAN] {
                let mut smoother = smoother(seconds, 0, ramp);
                let start = Instant::now();
                let first = seed.picture(Seeded::color);
                let mut expected = smoother.process(first, start);
                let (mut time, mut before) = (0.0, start);
                for index in 0..40 {
                    time += [FRAME, 1.0 / 60.0, 0.2, 0.004][index % 4];
                    let at = start + Duration::from_secs_f64(time);
                    let input = seed.picture(Seeded::color);
                    let actual = smoother.process(input.clone(), at);
                    let elapsed = at.duration_since(before).as_secs_f64();
                    expected = fixed(&expected, &input, elapsed, seconds);
                    before = at;
                    assert_eq!(actual, expected, "{seconds} s, ramp {ramp}, frame {index}");
                }
            }
        }
    }

    /// A named sequence of test pictures: the picture of each frame.
    type Sequence = (&'static str, fn(&mut Seeded, usize) -> EdgeColors);

    #[test]
    fn emitted_light_stays_between_shown_and_wanted_for_every_knob() {
        // `blend` rounds each product: allow two units in the last place of 1.
        const ROUNDING: f32 = 2.0 * f32::EPSILON;
        let sequences: [Sequence; 4] = [
            ("cut every frame", |seed, _| seed.picture(Seeded::color)),
            ("alternating flashes", |seed, frame| {
                let level = if frame % 2 == 0 { 0.02 } else { 1.0 };
                seed.picture(|seed| seed.scaled(level))
            }),
            ("black and white", |_, frame| {
                uniform(if frame % 3 == 0 {
                    WHITE
                } else {
                    LinearColor::black()
                })
            }),
            ("motion", |seed, frame| {
                let mut samples = vec![GRAY; 4 * PER_EDGE];
                samples[frame % (4 * PER_EDGE)] = seed.color();
                picture(&samples)
            }),
        ];
        let mut seed = Seeded(0xC0_4E5);
        for seconds in [0.0, 0.05, 0.5, 2.0] {
            for cut in [0, 50, 100] {
                for ramp in [0.0, 0.25, 2.0] {
                    if TemporalSmoother::new(&config(seconds, cut, ramp)).is_none() {
                        continue;
                    }
                    for (name, sequence) in sequences {
                        let mut smoother = smoother(seconds, cut, ramp);
                        let start = Instant::now();
                        let mut time = 0.0;
                        let mut shown = smoother.process(sequence(&mut seed, 0), start);
                        for frame in 1..150 {
                            time += seed.pick(&[0.0, 0.001, 1.0 / 120.0, FRAME, 0.1, 0.5]);
                            let target = sequence(&mut seed, frame);
                            let at = start + Duration::from_secs_f64(time);
                            let next = smoother.process(target, at);
                            let label = format!(
                                "{seconds} s, cut {cut}, ramp {ramp}, {name}, frame {frame}"
                            );
                            let wanted = smoother.history.as_ref().unwrap().wanted.clone();
                            for ((before, after), goal) in
                                sample_pairs(&shown, &next).zip(samples(&wanted))
                            {
                                for (value, a, b) in [
                                    (after.r, before.r, goal.r),
                                    (after.g, before.g, goal.g),
                                    (after.b, before.b, goal.b),
                                ] {
                                    assert!(
                                        value >= a.min(b) - ROUNDING
                                            && value <= a.max(b) + ROUNDING,
                                        "{label}: {value} outside {a}..{b}"
                                    );
                                    assert!((0.0..=1.0 + ROUNDING).contains(&value), "{label}");
                                }
                            }
                            shown = next;
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_cut_lands_with_full_response_and_eases_without() {
        let mut seed = Seeded(0xC07);
        let before = seed.picture(|seed| seed.scaled(0.5));
        let after = seed.picture(|seed| seed.scaled(0.5));
        assert_eq!(cut_strength(&before, &after), 1.0);

        let mut full = smoother(0.5, 100, 0.0);
        assert_eq!(change(&mut full, &before, &after, FRAME)[1], after);

        // The elapsed time as the smoother sees it, in whole nanoseconds.
        let elapsed = Duration::from_secs_f64(FRAME).as_secs_f64();
        let ordinary = ordinary_share(SmoothingSeconds(0.5), elapsed);
        let mut none = smoother(0.5, 0, 0.0);
        assert_eq!(
            change(&mut none, &before, &after, FRAME)[1],
            before.blend(&after, ordinary)
        );

        let mut half = smoother(0.5, 50, 0.0);
        let share = 1.0 - 0.5 * (1.0 - ordinary);
        assert_eq!(
            change(&mut half, &before, &after, FRAME)[1],
            before.blend(&after, share)
        );
    }

    /// Outputs at cut response 100 and 0, which agree when nothing is a cut.
    fn with_and_without_cuts(pictures: &[EdgeColors]) -> (Vec<EdgeColors>, Vec<EdgeColors>) {
        let times: Vec<_> = (0..pictures.len()).map(time_of).collect();
        (
            run(&mut smoother(0.5, 100, 0.0), pictures, &times),
            run(&mut smoother(0.5, 0, 0.0), pictures, &times),
        )
    }

    #[test]
    fn motion_on_a_few_samples_is_no_cut() {
        let mut seed = Seeded(0x307);
        let still = seed.picture(|seed| seed.scaled(0.6));
        let mut pictures = vec![still.clone()];
        // A bright block moving along the top edge: 4 of 40 samples change a lot.
        for start in 0..PER_EDGE - 3 {
            let mut samples = samples(&still);
            samples[start..start + 4].fill(WHITE);
            pictures.push(picture(&samples));
        }
        for pair in pictures.windows(2) {
            assert_eq!(cut_strength(&pair[0], &pair[1]), 0.0);
        }
        let (cuts, none) = with_and_without_cuts(&pictures);
        assert_eq!(cuts, none);
    }

    #[test]
    fn a_slow_fade_is_no_cut() {
        let from = Seeded(0xFADE).picture(|seed| seed.scaled(0.8));
        let to = Seeded(0xF00D).picture(|seed| seed.scaled(0.8));
        // Two seconds at 30 fps: about 0.01 in Oklab per frame.
        let pictures: Vec<_> = (0..=60)
            .map(|frame| from.blend(&to, f32::from(u8::try_from(frame).unwrap()) / 60.0))
            .collect();
        let (cuts, none) = with_and_without_cuts(&pictures);
        assert_eq!(cuts, none);
    }

    #[test]
    fn black_bars_do_not_hide_a_cut() {
        let mut seed = Seeded(0xBA4);
        let scene = |seed: &mut Seeded| {
            // Top and bottom are black bars; left and right show the picture.
            let mut samples = vec![LinearColor::black(); 4 * PER_EDGE];
            for edge in [1, 3] {
                for sample in &mut samples[edge * PER_EDGE..(edge + 1) * PER_EDGE] {
                    *sample = seed.scaled(0.7);
                }
            }
            picture(&samples)
        };
        let (before, after) = (scene(&mut seed), scene(&mut seed));
        // Half the samples are black in both: counted, they would leave 50%.
        assert_eq!(cut_strength(&before, &after), 1.0);
        assert_eq!(
            change(&mut smoother(0.5, 100, 0.0), &before, &after, FRAME)[1],
            after
        );
    }

    #[test]
    fn a_one_frame_flash_with_full_response_goes_up_and_comes_back() {
        let dark = uniform(LinearColor::new(0.02, 0.02, 0.03));
        let flash = uniform(LinearColor::new(1.0, 0.8, 0.5));
        let pictures = [dark.clone(), flash.clone(), dark.clone(), dark.clone()];
        let times: Vec<_> = (0..pictures.len()).map(time_of).collect();
        let shown = run(&mut smoother(0.5, 100, 0.0), &pictures, &times);
        assert_eq!(shown[1], flash);
        assert_eq!(shown[2], dark);
        assert_eq!(shown[3], dark);
    }

    #[test]
    fn cut_strength_eases_from_motion_to_a_cut() {
        let still = uniform(GRAY);
        let with_changed = |count: usize| {
            let mut samples = samples(&still);
            samples[..count].fill(RED);
            picture(&samples)
        };
        let strength = |count| cut_strength(&still, &with_changed(count));
        assert_eq!(strength(0), 0.0);
        assert_eq!(strength(16), 0.0);
        assert_eq!(strength(28), 1.0);
        assert_eq!(strength(40), 1.0);
        let between: Vec<_> = (16..=28).map(strength).collect();
        assert!(
            between.windows(2).all(|pair| pair[0] < pair[1]),
            "{between:?}"
        );
        let black = uniform(LinearColor::black());
        assert_eq!(cut_strength(&black, &black), 0.0);
        assert_eq!(cut_strength(&black, &still), 1.0);
        assert_eq!(cut_strength(&still, &black), 1.0);
    }

    /// Lightness of every frame after a jump from `from` to `to`, near-instant
    /// smoothing, so the ramp alone sets the pace.
    fn ramp(from: LinearColor, to: LinearColor, seconds: f32, cut: u8) -> Vec<f32> {
        let mut smoother = smoother(0.001, cut, seconds);
        change(
            &mut smoother,
            &uniform(from),
            &uniform(to),
            2.0 * f64::from(seconds) + 0.1,
        )
        .iter()
        .map(|shown| lightness(samples(shown)[0]))
        .collect()
    }

    /// The first frame index at which `lightness` reaches `goal`.
    fn reached(lightness: &[f32], goal: impl Fn(f32) -> bool) -> usize {
        lightness.iter().position(|&l| goal(l)).unwrap()
    }

    #[test]
    fn the_ramp_takes_its_time_from_dark_to_full_and_back() {
        for seconds in [0.5, 1.0, 2.0] {
            let frames = frame_count(f64::from(seconds));
            let up = ramp(LinearColor::black(), WHITE, seconds, 0);
            let at = reached(&up, |l| l > 0.999);
            assert!(
                at.abs_diff(frames) <= 1,
                "{seconds} s up: frame {at} of {frames}"
            );
            let down = ramp(WHITE, LinearColor::black(), seconds, 0);
            let at = reached(&down, |l| l < 0.001);
            assert!(
                at.abs_diff(frames) <= 1,
                "{seconds} s down: frame {at} of {frames}"
            );
            let step = (FRAME / f64::from(seconds)) as f32;
            for lightness in [&up, &down] {
                for pair in lightness.windows(2) {
                    assert!((pair[1] - pair[0]).abs() <= step + 1e-4, "{pair:?}");
                }
            }
        }
    }

    #[test]
    fn the_ramp_also_holds_a_cut() {
        let up = ramp(LinearColor::black(), WHITE, 1.0, 100);
        assert!(up[1] < 0.034, "{up:?}");
        let frames = frame_count(1.0);
        assert!(reached(&up, |l| l > 0.999).abs_diff(frames) <= 1);
    }

    #[test]
    fn a_hue_change_at_equal_luminance_is_not_slowed() {
        let shown = LinearColor::new(0.6, 0.1, 0.3);
        let (r, b) = (0.05, 0.6);
        let g = (shown.luminance() - 0.2126 * r - 0.0722 * b) / 0.7152;
        let target = LinearColor::new(r, g, b);
        assert!((shown.luminance() - target.luminance()).abs() < 1e-6);
        let limited = change(
            &mut smoother(0.5, 0, 2.0),
            &uniform(shown),
            &uniform(target),
            0.5,
        );
        let free = change(
            &mut smoother(0.5, 0, 0.0),
            &uniform(shown),
            &uniform(target),
            0.5,
        );
        assert_eq!(limited, free);
        assert_eq!(ramp_cap(GRAY, GRAY, 0.0), 1.0);
    }

    #[test]
    fn ordinary_change_and_the_ramp_do_not_depend_on_the_frame_rate() {
        // How far from `to` the lights are 0.8 s after a jump from `from`.
        let remaining = |fps: u32, ramp: f32, from: LinearColor, to: LinearColor| {
            let mut smoother = smoother(0.5, 0, ramp);
            let count = 4 * fps / 5;
            let times: Vec<_> = (0..=count).map(|i| f64::from(i) / f64::from(fps)).collect();
            let pictures: Vec<_> = std::iter::once(uniform(from))
                .chain(std::iter::repeat_n(
                    uniform(to),
                    usize::try_from(count).unwrap(),
                ))
                .collect();
            let shown = run(&mut smoother, &pictures, &times);
            Oklab::from(samples(shown.last().unwrap())[0]).distance(Oklab::from(to))
        };
        // Without the ramp the exponential is exact: a product of shares.
        for fps in [24, 30, 60, 120] {
            let difference =
                (remaining(fps, 0.0, GRAY, RED) - remaining(240, 0.0, GRAY, RED)).abs();
            assert!(difference < 1e-4, "{fps} fps: {difference}");
        }
        let black = LinearColor::black();
        let reference = remaining(240, 1.0, black, WHITE);
        for fps in [24, 30, 60, 120] {
            let difference = (remaining(fps, 1.0, black, WHITE) - reference).abs();
            assert!(
                difference < Oklab::JUST_NOTICEABLE / 2.0,
                "{fps} fps, ramp: {difference}"
            );
        }
    }

    #[test]
    fn a_cut_under_a_ramp_proceeds_at_the_ramp_pace_at_any_frame_rate() {
        let black = LinearColor::black();
        let lightness_after = |fps: u32, frames: u32| {
            let mut smoother = smoother(2.0, 100, 0.5);
            let start = Instant::now();
            let mut shown = smoother.process(uniform(black), start);
            for frame in 1..=frames {
                let at = start + Duration::from_secs_f64(f64::from(frame) / f64::from(fps));
                shown = smoother.process(uniform(WHITE), at);
            }
            lightness(samples(&shown)[0])
        };
        for fps in [24_u16, 30, 60, 120] {
            let frames = u32::from(fps / 4);
            let expected = f32::from(u16::try_from(frames).unwrap()) / f32::from(fps) / 0.5;
            let halfway = lightness_after(u32::from(fps), frames);
            assert!((halfway - expected).abs() < 0.01, "{fps} fps: {halfway}");
            assert!(
                lightness_after(u32::from(fps), u32::from(fps) * 6 / 10) > 0.999,
                "{fps} fps"
            );
        }
    }

    #[test]
    fn repeated_or_earlier_times_change_nothing_and_keep_the_cut_for_later() {
        let mut smoother = smoother(0.5, 100, 0.5);
        let start = Instant::now() + Duration::from_secs(1);
        let first = smoother.process(uniform(GRAY), start);
        for at in [start, start - Duration::from_millis(100)] {
            assert_eq!(smoother.process(uniform(RED), at), first);
        }
        let mut fixed = self::smoother(0.5, 0, 0.5);
        fixed.process(uniform(GRAY), start);
        let later = start + Duration::from_secs_f64(FRAME);
        let with_cut = smoother.process(uniform(RED), later);
        let without = fixed.process(uniform(RED), later);
        let toward = |shown: &EdgeColors| samples(shown)[0].r - GRAY.r;
        assert!(toward(&with_cut) > toward(&without), "{with_cut:?}");
    }

    #[test]
    fn a_crop_change_keeps_positions_and_is_no_cut() {
        let left_to_right =
            |count: usize| ColorStrip::new(vec![GRAY, RED]).unwrap().resample(count);
        let strips = |count| {
            let strip = ColorStrip::new(left_to_right(count)).unwrap();
            EdgeColors::new(strip.clone(), strip.clone(), strip.clone(), strip)
        };
        let (before, after) = (strips(2), strips(3));
        let start = Instant::now();
        let later = start + Duration::from_secs_f64(FRAME);
        let mut cuts = smoother(0.5, 100, 0.0);
        cuts.process(before.clone(), start);
        let shown = cuts.process(after.clone(), later);
        assert_eq!(shown.top.len(), 3);
        for (shown, expected) in shown.top.samples().iter().zip(after.top.samples()) {
            for (a, b) in [
                (shown.r, expected.r),
                (shown.g, expected.g),
                (shown.b, expected.b),
            ] {
                assert!((a - b).abs() < 1e-6, "{shown:?} {expected:?}");
            }
        }
        let mut none = smoother(0.5, 0, 0.0);
        none.process(before, start);
        assert_eq!(none.process(after, later), shown);
    }
}
