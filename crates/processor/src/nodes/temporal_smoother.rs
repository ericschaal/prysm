use prysm_core::{BrightnessSmoothing, ColorStrip, Config, EdgeColors, LinearColor, Oklab};
use std::time::Instant;

/// At full response, the rate gain per just-noticeable step of change. Chosen so
/// that with 0.5 s smoothing a 10-step change (a cut) completes 95% in one
/// 30 fps frame: k0 · (1 + 1.4 · 10) · 1/30 s = ln 20.
const FULL_RESPONSE: f64 = 1.4;

/// With [`BrightnessSmoothing::Faster`], lightness settles this many times as
/// fast as hue and saturation: a cut's light level follows in a third of the time.
const FASTER_LIGHTNESS: f64 = 3.0;

/// With [`BrightnessSmoothing::Limited`], Oklab lightness changes at most this
/// much per second: black to white in no less than a quarter second, quick
/// enough for a cut, slow enough that a flash does not dazzle.
const LIGHTNESS_PER_SECOND: f64 = 4.0;

/// Temporal smoothing node
///
/// Each sample moves exponentially toward its new colour, by elapsed time, so
/// the result does not depend on the frame rate. The rate adapts to how large
/// the change looks: `k(d) = k0 · (1 + β · d / JND)`, with `d` the Oklab
/// distance between the shown and the new colour. Grain (small `d`) settles
/// slowly and does not shimmer; a cut (large `d`) follows quickly. With no
/// response (β = 0) every change moves at `k0`, exactly as a fixed rate.
#[derive(Debug, Clone)]
pub struct TemporalSmoother {
    rates: Rates,
    previous: Option<(EdgeColors, Instant)>,
}

impl TemporalSmoother {
    /// `None` when the config turns smoothing off.
    pub fn new(config: &Config) -> Option<Self> {
        Some(Self {
            rates: Rates::new(config)?,
            previous: None,
        })
    }

    /// The same history at the new rates; `None` when smoothing is now off.
    pub fn reconfigured(self, config: &Config) -> Option<Self> {
        Some(Self {
            rates: Rates::new(config)?,
            ..self
        })
    }

    pub fn process(&mut self, input: EdgeColors, mut now: Instant) -> EdgeColors {
        let rates = self.rates;
        let smoothed = match &mut self.previous {
            Some((previous, previous_time)) => {
                // Repeated or earlier times do not advance smoothing.
                now = now.max(*previous_time);
                let elapsed = now.duration_since(*previous_time).as_secs_f64();
                adopt_sample_counts(previous, &input);
                if elapsed > 0.0 {
                    previous.combine(&input, |shown, target| rates.step(shown, target, elapsed))
                } else {
                    previous.clone()
                }
            }
            None => input,
        };
        self.previous = Some((smoothed.clone(), now));
        smoothed
    }
}

/// Keeps LED positions across crop changes: each previous strip is resampled
/// to the new sample count, so its colours stay where they were on the edge.
fn adopt_sample_counts(previous: &mut EdgeColors, input: &EdgeColors) {
    for (old, new) in [
        (&mut previous.top, &input.top),
        (&mut previous.right, &input.right),
        (&mut previous.bottom, &input.bottom),
        (&mut previous.left, &input.left),
    ] {
        if old.len() != new.len() {
            *old = ColorStrip::new(old.resample(new.len())).expect("sample count is nonzero");
        }
    }
}

/// The configured speeds. Only exists for a finite, positive smoothing time.
#[derive(Debug, Clone, Copy)]
struct Rates {
    /// Seconds for a change too small to see to complete 95%, that is,
    /// `k0 = ln 20 / seconds`.
    seconds: f64,
    /// β: how much each just-noticeable step of change speeds the rate up.
    beta: f64,
    brightness: BrightnessSmoothing,
}

impl Rates {
    fn new(config: &Config) -> Option<Self> {
        config.smoothing_seconds.is_enabled().then(|| Self {
            seconds: f64::from(config.smoothing_seconds.0),
            beta: FULL_RESPONSE * f64::from(config.smoothing_response_percent.as_ratio()),
            brightness: config.brightness_smoothing,
        })
    }

    /// `k(d) / k0 = 1 + β · d / JND`: how much faster than `k0` a change of
    /// Oklab distance `d` moves.
    fn speedup(self, d: f32) -> f64 {
        1.0 + self.beta * f64::from(d / Oklab::JUST_NOTICEABLE)
    }

    /// The share of the remaining change completed in `elapsed` seconds at
    /// `speedup · k0`: `1 - e^(-k · elapsed)`, in 0..1.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a share in 0..1 fits f32; f64 only keeps the exponent precise"
    )]
    fn share(self, speedup: f64, elapsed: f64) -> f32 {
        // k0 · elapsed is computed as ln 20 · elapsed / seconds, the fixed
        // rate's own order, so response 0 (speedup exactly 1) is bit-for-bit
        // the fixed rate. exp_m1 keeps small shares when frames arrive close together.
        let exponent = -20.0_f64.ln() * elapsed * speedup / self.seconds;
        -exponent.exp_m1() as f32
    }

    /// One sample's colour after `elapsed` (> 0) seconds of moving from
    /// `shown` toward `target`.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a lightness step beyond f32 is infinite, which clamps to no limit"
    )]
    fn step(self, shown: LinearColor, target: LinearColor, elapsed: f64) -> LinearColor {
        let (from, to) = (Oklab::from(shown), Oklab::from(target));
        let speedup = self.speedup(from.distance(to));
        let color = self.share(speedup, elapsed);
        match self.brightness {
            BrightnessSmoothing::WithColor => shown.blend(&target, color),
            BrightnessSmoothing::Faster => {
                let lightness = self.share(speedup * FASTER_LIGHTNESS, elapsed);
                toward(from, to, lightness, color).to_linear()
            }
            BrightnessSmoothing::Limited => {
                let mut next = toward(from, to, color, color);
                // Finite and non-negative: elapsed is a positive duration.
                let reach = (LIGHTNESS_PER_SECOND * elapsed) as f32;
                next.l = from.l + (next.l - from.l).clamp(-reach, reach);
                next.to_linear()
            }
        }
    }
}

/// `from` moved toward `to`: its lightness by the share `lightness` of the
/// way, its hue and saturation (a, b) by `color`.
fn toward(from: Oklab, to: Oklab, lightness: f32, color: f32) -> Oklab {
    let mix = |start: f32, end: f32, share: f32| start + (end - start) * share;
    Oklab {
        l: mix(from.l, to.l, lightness),
        a: mix(from.a, to.a, color),
        b: mix(from.b, to.b, color),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{SmoothingResponsePercent, SmoothingSeconds};
    use std::num::NonZeroUsize;
    use std::time::Duration;

    fn config(seconds: f32, response: u8, brightness: BrightnessSmoothing) -> Config {
        Config {
            smoothing_seconds: SmoothingSeconds(seconds),
            smoothing_response_percent: SmoothingResponsePercent(response),
            brightness_smoothing: brightness,
            ..Config::default()
        }
    }

    fn smoother(seconds: f32, response: u8, brightness: BrightnessSmoothing) -> TemporalSmoother {
        TemporalSmoother::new(&config(seconds, response, brightness)).unwrap()
    }

    /// One colour on every edge, one sample each.
    fn edges(color: LinearColor) -> EdgeColors {
        let strip = ColorStrip::fill(color, NonZeroUsize::MIN);
        EdgeColors::new(strip.clone(), strip.clone(), strip.clone(), strip)
    }

    fn shown(colors: &EdgeColors) -> LinearColor {
        colors.top.sample_at(0.0)
    }

    /// Shows `from`, then `to` at each of `times` after the start.
    fn run(smoother: &mut TemporalSmoother, from: LinearColor, to: LinearColor, times: &[f64]) {
        let start = Instant::now();
        smoother.process(edges(from), start);
        for &time in times {
            smoother.process(edges(to), start + Duration::from_secs_f64(time));
        }
    }

    fn last(smoother: &TemporalSmoother) -> LinearColor {
        shown(&smoother.previous.as_ref().unwrap().0)
    }

    fn frames(fps: u32, seconds: f64) -> Vec<f64> {
        let count = (seconds * f64::from(fps)).round() as u32;
        (1..=count).map(|i| f64::from(i) / f64::from(fps)).collect()
    }

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

    #[test]
    fn smoothing_off_builds_no_node() {
        for seconds in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(
                TemporalSmoother::new(&config(seconds, 50, BrightnessSmoothing::Faster)).is_none()
            );
            assert!(
                smoother(0.5, 0, BrightnessSmoothing::WithColor)
                    .reconfigured(&config(seconds, 0, BrightnessSmoothing::WithColor))
                    .is_none()
            );
        }
    }

    #[test]
    fn response_zero_is_the_fixed_rate_bit_for_bit() {
        // The fixed rate exactly as it was before response existed.
        let fixed = |previous: &EdgeColors, input: &EdgeColors, elapsed: f64, seconds: f32| {
            let ratio = -(-20.0_f64.ln() * elapsed / f64::from(seconds)).exp_m1();
            previous.blend(input, ratio as f32)
        };
        let colors = [
            GRAY,
            RED,
            LinearColor::new(0.01, 0.3, 0.9),
            LinearColor::black(),
            GRAY,
        ];
        for seconds in [0.05, 0.1, 0.5, 2.0] {
            let mut smoother = smoother(seconds, 0, BrightnessSmoothing::WithColor);
            let start = Instant::now();
            let mut expected = smoother.process(edges(colors[0]), start);
            let (mut time, mut before) = (0.0, start);
            for (index, &color) in colors.iter().cycle().take(40).enumerate() {
                time += [1.0 / 30.0, 1.0 / 60.0, 0.2, 0.004][index % 4];
                let at = start + Duration::from_secs_f64(time);
                let actual = smoother.process(edges(color), at);
                let elapsed = at.duration_since(before).as_secs_f64();
                expected = fixed(&expected, &edges(color), elapsed, seconds);
                before = at;
                assert_eq!(actual, expected, "{seconds} s, frame {index}");
            }
        }
    }

    #[test]
    fn a_large_change_settles_faster_than_a_small_one() {
        let small = LinearColor::new(0.21, 0.2, 0.2);
        let completed = |target: LinearColor, response: u8| {
            let mut smoother = smoother(0.5, response, BrightnessSmoothing::WithColor);
            run(&mut smoother, GRAY, target, &[1.0 / 30.0]);
            // With colour blends in linear light: every channel completes the same share.
            (last(&smoother).r - GRAY.r) / (target.r - GRAY.r)
        };
        assert!(completed(RED, 100) > completed(small, 100) + 0.5);
        // Without response, any change completes the same share.
        assert!((completed(RED, 0) - completed(small, 0)).abs() < 1e-5);
    }

    #[test]
    fn full_response_completes_a_ten_step_change_in_one_frame_at_half_a_second() {
        let rates = Rates::new(&config(0.5, 100, BrightnessSmoothing::WithColor)).unwrap();
        let share = rates.share(rates.speedup(10.0 * Oklab::JUST_NOTICEABLE), 1.0 / 30.0);
        assert!((share - 0.95).abs() < 1e-4, "{share}");
    }

    #[test]
    fn every_variant_moves_monotonically_without_overshoot() {
        let pairs = [
            (GRAY, RED),
            (RED, LinearColor::new(0.02, 0.1, 0.9)),
            (LinearColor::black(), LinearColor::new(1.0, 1.0, 1.0)),
        ];
        for brightness in BrightnessSmoothing::ALL {
            for response in [0, 50, 100] {
                for (from, to) in pairs {
                    let mut smoother = smoother(0.5, response, brightness);
                    let start = Instant::now();
                    smoother.process(edges(from), start);
                    let (begin, end) = (Oklab::from(from), Oklab::from(to));
                    let mut distance = begin.distance(end);
                    for time in frames(60, 2.0) {
                        let next = shown(
                            &smoother.process(edges(to), start + Duration::from_secs_f64(time)),
                        );
                        assert!(next.r >= 0.0 && next.g >= 0.0 && next.b >= 0.0, "{next:?}");
                        let now = Oklab::from(next);
                        let label =
                            format!("{brightness:?} {response}% {from:?} -> {to:?} at {time}");
                        // Each blends its own components: those stay between start and target.
                        let components = match brightness {
                            BrightnessSmoothing::WithColor => [
                                (next.r, from.r, to.r),
                                (next.g, from.g, to.g),
                                (next.b, from.b, to.b),
                            ],
                            BrightnessSmoothing::Faster | BrightnessSmoothing::Limited => {
                                assert!(now.distance(end) <= distance + 1e-5, "{label}");
                                [
                                    (now.l, begin.l, end.l),
                                    (now.a, begin.a, end.a),
                                    (now.b, begin.b, end.b),
                                ]
                            }
                        };
                        for (value, a, b) in components {
                            assert!(
                                value >= a.min(b) - 1e-4 && value <= a.max(b) + 1e-4,
                                "{label}: {next:?}"
                            );
                        }
                        distance = now.distance(end);
                    }
                    assert!(
                        distance < Oklab::JUST_NOTICEABLE,
                        "{brightness:?} {response}%: {distance}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_frame_rate_barely_changes_an_adaptive_transition() {
        // One long step uses the starting distance, and so the fastest rate,
        // for its whole interval; small steps slow down as the distance
        // shrinks. An adaptive rate therefore cannot be exactly frame-rate
        // independent, only close; the fixed rate (response 0) is exact.
        for brightness in BrightnessSmoothing::ALL {
            let remaining = |fps: u32, response: u8| {
                let mut smoother = smoother(0.5, response, brightness);
                run(&mut smoother, GRAY, RED, &frames(fps, 0.2));
                Oklab::from(last(&smoother)).distance(Oklab::from(RED))
            };
            let reference = remaining(240, 100);
            for fps in [24, 30, 60, 120] {
                let difference = (remaining(fps, 100) - reference).abs();
                assert!(
                    difference < Oklab::JUST_NOTICEABLE,
                    "{brightness:?} {fps} fps: {difference}"
                );
            }
            let difference = (remaining(30, 0) - remaining(240, 0)).abs();
            let tolerance = if brightness == BrightnessSmoothing::Limited {
                Oklab::JUST_NOTICEABLE
            } else {
                1e-4
            };
            assert!(difference < tolerance, "{brightness:?} fixed: {difference}");
        }
    }

    #[test]
    fn faster_brightness_settles_lightness_at_three_times_the_rate() {
        let from = Oklab::from(GRAY);
        let to = Oklab::from(RED);
        let mut smoother = smoother(0.5, 0, BrightnessSmoothing::Faster);
        run(&mut smoother, GRAY, RED, &[0.1]);
        let now = Oklab::from(last(&smoother));
        let left = |value: f32, start: f32, end: f32| (end - value) / (end - start);
        // e^(-3 k t) = (e^(-k t))^3
        let color = left(now.a, from.a, to.a);
        assert!(
            (left(now.l, from.l, to.l) - color.powi(3)).abs() < 1e-3,
            "{now:?}"
        );
        assert!((left(now.b, from.b, to.b) - color).abs() < 1e-3, "{now:?}");
    }

    #[test]
    fn limited_brightness_never_moves_lightness_faster_than_its_pace() {
        let white = LinearColor::new(1.0, 1.0, 1.0);
        for response in [0, 100] {
            let mut smoother = smoother(0.1, response, BrightnessSmoothing::Limited);
            let start = Instant::now();
            let mut previous = smoother.process(edges(LinearColor::black()), start);
            let step = 1.0 / 60.0;
            for time in frames(60, 0.5) {
                let next = smoother.process(edges(white), start + Duration::from_secs_f64(time));
                let change = Oklab::from(shown(&next)).l - Oklab::from(shown(&previous)).l;
                assert!(
                    f64::from(change) <= LIGHTNESS_PER_SECOND * step + 1e-4,
                    "{time}: {change}"
                );
                previous = next;
            }
            // Within the pace, it gets there.
            assert!(Oklab::from(shown(&previous)).l > 0.99);
        }
        // A small change, well within the pace, blends as with colour.
        let mut limited = smoother(0.5, 0, BrightnessSmoothing::Limited);
        let mut together = smoother(0.5, 0, BrightnessSmoothing::WithColor);
        let small = LinearColor::new(0.22, 0.2, 0.2);
        run(&mut limited, GRAY, small, &[0.05]);
        run(&mut together, GRAY, small, &[0.05]);
        assert!(Oklab::from(last(&limited)).distance(Oklab::from(last(&together))) < 1e-3);
    }

    #[test]
    fn repeated_or_earlier_times_keep_every_variant_still() {
        for brightness in BrightnessSmoothing::ALL {
            let mut smoother = smoother(0.5, 100, brightness);
            let start = Instant::now() + Duration::from_secs(1);
            let first = smoother.process(edges(GRAY), start);
            for at in [start, start - Duration::from_millis(100)] {
                assert_eq!(smoother.process(edges(RED), at), first, "{brightness:?}");
            }
        }
    }
}
