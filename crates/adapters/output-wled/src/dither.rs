//! Linear colours to bytes over the light levels WLED shows.
//!
//! WLED raises each byte through its 8-bit gamma table and scales it by its
//! brightness, so at the dark end few levels remain: at gamma 2.2 and full
//! brightness bytes 0–14 are off and 15–24 the first level. Rounding each
//! channel to the nearest byte then steps fades and flips the hue of dim
//! colours. Instead each LED takes one of the two levels around the wanted
//! light: the upper one when the wanted light's place between them (linear
//! in light) exceeds the LED's fixed threshold, so a run of LEDs averages to
//! the wanted light. A threshold depends only on the LED's place in the
//! frame (a golden-ratio sequence, evenly spread over any run of LEDs): no
//! state, time or chance, so a still picture stays still. The three channels
//! of an LED share its threshold and round alike, keeping its hue.
//!
//! WLED 16 drives a channel at `color_fade(gammaT[byte], bri, true)` (`show`
//! in `FX_fcn.cpp`, `BusDigital::setPixelColor`): the gamma table (see
//! [`Gamma::correct`](crate::ddp::Gamma::correct); skipped when gamma is off or "Disable realtime gamma
//! correction" is on), then at `bri` 255 the value as is, else
//! `(v * bri + 127) >> 8` with the lowest bit set when `v` exceeds a quarter
//! of the LED's brightest channel plus one (`colors.cpp`). The levels are
//! those of an LED's brightest channel, as every channel of a grey is; below
//! full brightness a channel under a quarter of its LED's brightest can show
//! one level lower. WLED 0.15 scales by `(v * (bri + 1)) >> 8` instead
//! (NeoPixelBus `Dim`, through `SetLuminance`): the same at full brightness,
//! 2 lower to 1 higher of 255 below it (`v` 10 at `bri` 50: 1 where 16
//! shows 3).
//!
//! Not modelled: WLED's current limiter, RGBW auto-white and its colour
//! correction (white balance).
use crate::ddp::Transfer;
use prysm_core::{Color, LinearColor};
use std::num::NonZeroU8;

/// How colours become bytes for WLED.
#[derive(Debug)]
pub(crate) enum Encoding {
    /// The nearest byte for the transfer: dithering off.
    Nearest(Transfer),
    /// Dithered over the levels WLED shows.
    Dithered(Levels),
}

impl Encoding {
    /// sRGB at full brightness until WLED's transfer and brightness are
    /// followed.
    pub(crate) fn new(dither: bool) -> Self {
        if dither {
            Self::Dithered(Levels::new(Transfer::Srgb, None))
        } else {
            Self::Nearest(Transfer::Srgb)
        }
    }

    /// Follows WLED's transfer and brightness (`scaledBri`, 0–255; `None`
    /// when unknown). The levels are rebuilt only when either changes.
    pub(crate) fn follow(&mut self, transfer: Transfer, brightness: Option<u8>) {
        match self {
            Self::Nearest(current) => *current = transfer,
            Self::Dithered(levels) => levels.follow(transfer, brightness),
        }
    }

    /// The bytes for `color` at LED `led`, its place in the frame from 0.
    pub(crate) fn encode(&self, color: LinearColor, led: u32) -> Color {
        match self {
            Self::Nearest(transfer) => transfer.encode(color),
            Self::Dithered(levels) => {
                // One threshold for all three channels: they round alike.
                let threshold = Threshold::of(led);
                let byte = |light| levels.byte(light, threshold);
                Color::new(byte(color.r), byte(color.g), byte(color.b))
            }
        }
    }
}

/// Every distinct light WLED shows a channel at, ascending.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Levels {
    key: Key,
    /// At least two: the first off (byte 0), the last full (byte 255).
    levels: Vec<Level>,
}

/// What the levels depend on.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    transfer: Transfer,
    brightness: NonZeroU8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Level {
    /// A fraction of the top level.
    light: f32,
    /// A byte that shows it.
    byte: u8,
}

impl Key {
    /// An unknown gamma is taken as sRGB at full brightness: dithering then
    /// only spreads Prysm's own rounding. An unknown brightness, or none
    /// (nothing shows), is taken as full.
    fn new(transfer: Transfer, brightness: Option<u8>) -> Self {
        let brightness = match transfer {
            Transfer::Srgb => None,
            Transfer::Gamma(_) => brightness.and_then(NonZeroU8::new),
        };
        Self {
            transfer,
            brightness: brightness.unwrap_or(NonZeroU8::MAX),
        }
    }

    /// The light WLED drives a channel at for `byte`, on any one scale.
    fn light(self, byte: u8) -> f32 {
        match self.transfer {
            Transfer::Srgb => LinearColor::from_srgb(Color::new(byte, byte, byte)).r,
            Transfer::Gamma(gamma) => f32::from(dim(gamma.correct(byte), self.brightness)),
        }
    }
}

/// WLED 16's brightness on a channel that leads its LED (`color_fade` with
/// video scaling): `(value * brightness + 127) >> 8`, the lowest bit set to
/// keep the hue when `value` exceeds a quarter of the brightest (itself)
/// plus one.
#[allow(
    clippy::cast_possible_truncation,
    reason = "(255 × 254 + 127) >> 8 fits a byte"
)]
fn dim(value: u8, brightness: NonZeroU8) -> u8 {
    if brightness == NonZeroU8::MAX || value == 0 {
        return value;
    }
    let scaled = (u16::from(value) * u16::from(brightness.get()) + 127) >> 8;
    scaled as u8 | u8::from(value > value / 4 + 1)
}

impl Levels {
    pub(crate) fn new(transfer: Transfer, brightness: Option<u8>) -> Self {
        Self::of(Key::new(transfer, brightness))
    }

    /// Light is a fraction of what byte 255 shows at WLED's current
    /// brightness, and Prysm's linear value means just that: WLED's
    /// brightness dims Prysm's colours, so 1 is the top level, 0 off, and
    /// the levels between keep their true ratios to the top. (A fraction of
    /// `bri / 255` would leave the top level a rounding from 1, such as 127
    /// of 255 at `bri` 128, and white short of full.)
    ///
    /// Bytes that show the same light are one level, shown by the middle
    /// byte so an off-by-one in this model cannot reach another level; off
    /// keeps byte 0 and full byte 255, which WLED shows exactly.
    fn of(key: Key) -> Self {
        let lights: Vec<(u8, f32)> = (0..=u8::MAX).map(|byte| (byte, key.light(byte))).collect();
        // Byte 255 lights at every brightness above 0.
        let full = key.light(u8::MAX);
        let levels = lights
            .chunk_by(|(_, a), (_, b)| a == b)
            .map(|run| {
                let (first, light) = run[0];
                let (last, _) = run[run.len() - 1];
                let byte = match (first, last) {
                    (0, _) => 0,
                    (_, u8::MAX) => u8::MAX,
                    _ => first.midpoint(last),
                };
                Level {
                    light: light / full,
                    byte,
                }
            })
            .collect();
        Self { key, levels }
    }

    fn follow(&mut self, transfer: Transfer, brightness: Option<u8>) {
        let key = Key::new(transfer, brightness);
        if key != self.key {
            *self = Self::of(key);
        }
    }

    /// The byte for `light` (0–1, a fraction of full) at an LED with
    /// `threshold`. On a level that level; between two the upper when the
    /// place between them exceeds the threshold.
    fn byte(&self, light: f32, threshold: Threshold) -> u8 {
        let light = if light.is_nan() {
            0.0
        } else {
            light.clamp(0.0, 1.0)
        };
        let upper = self
            .levels
            .partition_point(|level| level.light <= light)
            .clamp(1, self.levels.len() - 1);
        let (lower, upper) = (self.levels[upper - 1], self.levels[upper]);
        let place = (light - lower.light) / (upper.light - lower.light);
        if place > threshold.0 {
            upper.byte
        } else {
            lower.byte
        }
    }
}

/// An LED's threshold, strictly between 0 and 1.
#[derive(Debug, Clone, Copy)]
struct Threshold(f32);

impl Threshold {
    /// The fractional part of ½ + `led` / φ, in 2⁻²³ steps offset by half a
    /// step: LED 0 has about ½, and any run of LEDs spreads evenly over 0–1.
    #[allow(
        clippy::cast_precision_loss,
        reason = "23 bits and a half are exact in an f32"
    )]
    fn of(led: u32) -> Self {
        // 2³² / φ: the wrapping product is the fraction in 2⁻³² steps.
        const INVERSE_PHI: u32 = 0x9E37_79B9;
        let fraction = led.wrapping_mul(INVERSE_PHI).wrapping_add(1 << 31) >> 9;
        Self((fraction as f32 + 0.5) / (1 << 23) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddp::Gamma;

    const LEDS: u32 = 64;

    fn gamma(value: f32) -> Transfer {
        Transfer::Gamma(Gamma::new(value).unwrap())
    }

    /// WLED 16 driving one LED, written from its source apart from the
    /// levels above: the gamma table, then `color_fade(c, bri, true)` over
    /// the whole LED.
    fn wled(bytes: [u8; 3], gamma: f32, bri: u8) -> [u8; 3] {
        let table = |byte: u8| ((f32::from(byte) / 255.0).powf(gamma) * 255.0 + 0.5) as u8;
        let channels = bytes.map(table);
        if channels == [0; 3] || bri == 0 {
            return [0; 3];
        }
        if bri == 255 {
            return channels;
        }
        let keep = (channels.into_iter().max().unwrap() >> 2) + 1;
        channels.map(|c| {
            let scaled = ((u32::from(c) * u32::from(bri) + 0x7F) >> 8) as u8;
            scaled | u8::from(c > keep)
        })
    }

    /// The light WLED shows for a grey of `byte`, as a fraction of full.
    fn grey_light(byte: u8, gamma: f32, bri: u8) -> f32 {
        f32::from(wled([byte; 3], gamma, bri)[0]) / f32::from(wled([255; 3], gamma, bri)[0])
    }

    /// The distinct grey lights WLED shows, ascending.
    fn wled_levels(gamma: f32, bri: u8) -> Vec<f32> {
        let mut levels: Vec<f32> = (0..=u8::MAX).map(|b| grey_light(b, gamma, bri)).collect();
        levels.dedup();
        levels
    }

    fn grey(light: f32) -> LinearColor {
        LinearColor::new(light, light, light)
    }

    fn dithered(transfer: Transfer, bri: u8) -> Encoding {
        Encoding::Dithered(Levels::new(transfer, Some(bri)))
    }

    #[test]
    fn a_run_of_leds_averages_to_the_wanted_light() {
        for bri in [255, 128, 40] {
            let encoding = dithered(gamma(2.2), bri);
            let levels = wled_levels(2.2, bri);
            for step in 0..=600 {
                let wanted = f32::from(u16::try_from(step).unwrap()) / 4000.0;
                // Spread is a fraction of the gap between WLED's levels.
                let above = levels
                    .partition_point(|&level| level <= wanted)
                    .min(levels.len() - 1);
                let gap = levels[above] - levels[above - 1];
                for start in [0, 1000, 40_000] {
                    let mean = (start..start + LEDS)
                        .map(|led| {
                            let color = encoding.encode(grey(wanted), led);
                            grey_light(color.r, 2.2, bri)
                        })
                        .sum::<f32>()
                        / LEDS as f32;
                    assert!(
                        (mean - wanted).abs() <= gap * 0.04 + 1e-6,
                        "bri {bri}, LEDs from {start}: {wanted} averages {mean}, gap {gap}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_dim_colour_averages_to_itself_channel_by_channel() {
        let encoding = dithered(gamma(2.2), 255);
        let levels = wled_levels(2.2, 255);
        let full = f32::from(wled([255; 3], 2.2, 255)[0]);
        for wanted in [
            [0.004, 0.0012, 0.0003],
            [0.02, 0.0005, 0.009],
            [0.001, 0.03, 0.06],
        ] {
            let shown: Vec<[u8; 3]> = (0..LEDS)
                .map(|led| {
                    let color =
                        encoding.encode(LinearColor::new(wanted[0], wanted[1], wanted[2]), led);
                    wled([color.r, color.g, color.b], 2.2, 255)
                })
                .collect();
            for (channel, &wanted) in wanted.iter().enumerate() {
                let above = levels.partition_point(|&level| level <= wanted);
                let gap = levels[above] - levels[above - 1];
                let mean = shown
                    .iter()
                    .map(|led| f32::from(led[channel]) / full)
                    .sum::<f32>()
                    / LEDS as f32;
                assert!(
                    (mean - wanted).abs() <= gap * 0.04 + 1e-6,
                    "channel {channel}: {wanted} averages {mean}, gap {gap}"
                );
            }
        }
    }

    #[test]
    fn every_table_ascends_from_off_to_full() {
        for gamma in [
            Gamma::new(0.1).unwrap(),
            Gamma::new(0.5).unwrap(),
            Gamma::LINEAR,
            Gamma::new(2.2).unwrap(),
            Gamma::new(2.8).unwrap(),
            Gamma::new(3.0).unwrap(),
        ] {
            for bri in 0..=u8::MAX {
                let levels = Levels::new(Transfer::Gamma(gamma), Some(bri)).levels;
                assert!(levels.len() >= 2, "{gamma:?} at {bri}");
                assert!(
                    levels
                        .windows(2)
                        .all(|pair| pair[0].light < pair[1].light && pair[0].byte < pair[1].byte),
                    "{gamma:?} at {bri}"
                );
                assert_eq!(
                    (levels[0].light, levels[0].byte),
                    (0.0, 0),
                    "{gamma:?} at {bri}"
                );
                let top = levels[levels.len() - 1];
                assert_eq!((top.light, top.byte), (1.0, 255), "{gamma:?} at {bri}");
            }
        }
    }

    #[test]
    fn off_full_and_every_level_are_the_same_on_every_led() {
        let transfers = [
            (gamma(2.2), 255),
            (gamma(2.2), 128),
            (gamma(2.2), 40),
            (gamma(2.8), 1),
            (Transfer::Gamma(Gamma::LINEAR), 200),
            (Transfer::Srgb, 255),
        ];
        for (transfer, bri) in transfers {
            let encoding = dithered(transfer, bri);
            let Encoding::Dithered(levels) = &encoding else {
                unreachable!()
            };
            assert_eq!(levels.levels.first().map(|l| l.byte), Some(0));
            assert_eq!(
                levels.levels.last().map(|l| (l.light, l.byte)),
                Some((1.0, 255))
            );
            for led in 0..LEDS {
                let at = |light| encoding.encode(grey(light), led);
                assert_eq!(at(0.0), Color::black(), "{transfer:?} at {bri}");
                assert_eq!(at(-1.0), Color::black(), "{transfer:?} at {bri}");
                assert_eq!(at(f32::NAN), Color::black(), "{transfer:?} at {bri}");
                assert_eq!(at(1.0), Color::new(255, 255, 255), "{transfer:?} at {bri}");
                assert_eq!(at(2.0), Color::new(255, 255, 255), "{transfer:?} at {bri}");
                for level in &levels.levels {
                    let byte = level.byte;
                    assert_eq!(at(level.light), Color::new(byte, byte, byte), "{level:?}");
                }
            }
        }
    }

    #[test]
    fn levels_are_wleds_shown_by_the_middle_of_their_bytes() {
        for bri in [255, 128, 40, 1] {
            let Encoding::Dithered(levels) = dithered(gamma(2.2), bri) else {
                unreachable!()
            };
            let lights: Vec<f32> = levels.levels.iter().map(|level| level.light).collect();
            assert_eq!(lights, wled_levels(2.2, bri), "bri {bri}");
            for (index, level) in levels.levels.iter().enumerate() {
                let shown = |byte| grey_light(byte, 2.2, bri);
                assert_eq!(shown(level.byte), level.light, "bri {bri}, {level:?}");
                let run = (0..=u8::MAX).filter(|&byte| shown(byte) == level.light);
                let (first, last) = (run.clone().min().unwrap(), run.max().unwrap());
                if index > 0 && index < levels.levels.len() - 1 {
                    assert_eq!(level.byte, first.midpoint(last), "bri {bri}, {level:?}");
                }
            }
        }
        // 2.2 at full: bytes 0–14 are off, 15–24 the first level.
        let Encoding::Dithered(levels) = dithered(gamma(2.2), 255) else {
            unreachable!()
        };
        assert_eq!(levels.levels[1].byte, 19);
    }

    #[test]
    fn the_same_colours_give_the_same_bytes() {
        let encoding = dithered(gamma(2.2), 128);
        let color = LinearColor::new(0.003, 0.011, 0.0007);
        let first: Vec<_> = (0..LEDS).map(|led| encoding.encode(color, led)).collect();
        let second: Vec<_> = (0..LEDS).map(|led| encoding.encode(color, led)).collect();
        assert_eq!(first, second);
    }

    #[test]
    fn more_light_never_dims_an_led() {
        for bri in [255, 40] {
            let encoding = dithered(gamma(2.2), bri);
            for led in 0..LEDS {
                let mut previous = 0;
                for step in 0..=2000 {
                    let light = f32::from(u16::try_from(step).unwrap()) / 2000.0;
                    let byte = encoding.encode(grey(light), led).r;
                    assert!(
                        byte >= previous,
                        "LED {led} at {light}: {byte} < {previous}"
                    );
                    previous = byte;
                }
            }
        }
    }

    #[test]
    fn an_leds_channels_round_alike() {
        let encoding = dithered(gamma(2.2), 255);
        let Encoding::Dithered(levels) = &encoding else {
            unreachable!()
        };
        // Each channel at the same place between different levels, and a grey.
        let (first, second, third) = (levels.levels[1], levels.levels[2], levels.levels[3]);
        let between = |lower: f32, upper: f32| lower + (upper - lower) * 0.3;
        let color = LinearColor::new(
            between(first.light, second.light),
            between(second.light, third.light),
            between(0.0, first.light),
        );
        for led in 0..LEDS {
            let color = encoding.encode(color, led);
            let up = color.r == second.byte;
            assert_eq!(color.g == third.byte, up, "LED {led}");
            assert_eq!(color.b == first.byte, up, "LED {led}");
            let grey = encoding.encode(grey(0.004), led);
            assert!(grey.r == grey.g && grey.g == grey.b, "LED {led}: {grey:?}");
        }
    }

    #[test]
    fn dithering_off_rounds_to_the_nearest_byte_as_before() {
        let mut encoding = Encoding::new(false);
        for (transfer, bri) in [(gamma(2.2), Some(40)), (Transfer::Srgb, None)] {
            encoding.follow(transfer, bri);
            for step in 0..=1000 {
                let value = f32::from(u16::try_from(step).unwrap()) / 1000.0;
                let color = LinearColor::new(value, value / 2.0, 1.0 - value);
                assert_eq!(encoding.encode(color, step), transfer.encode(color));
            }
        }
    }

    #[test]
    fn levels_follow_wled_and_unknowns_take_srgb_and_full_brightness() {
        let mut encoding = Encoding::new(true);
        let levels = |encoding: &Encoding| match encoding {
            Encoding::Dithered(levels) => levels.clone(),
            Encoding::Nearest(_) => unreachable!(),
        };
        assert_eq!(levels(&encoding), Levels::new(Transfer::Srgb, Some(40)));
        encoding.follow(gamma(2.2), Some(40));
        assert_eq!(levels(&encoding), Levels::new(gamma(2.2), Some(40)));
        assert_ne!(levels(&encoding), Levels::new(gamma(2.2), Some(41)));
        for unknown in [None, Some(0)] {
            encoding.follow(gamma(2.2), unknown);
            assert_eq!(levels(&encoding), Levels::new(gamma(2.2), Some(255)));
        }
    }

    #[test]
    fn thresholds_spread_evenly_over_any_run_of_leds() {
        for start in (0..=u32::from(u16::MAX) - LEDS).step_by(97) {
            let mut thresholds: Vec<f32> = (start..start + LEDS)
                .map(|led| Threshold::of(led).0)
                .collect();
            thresholds.sort_by(f32::total_cmp);
            // The k-th smallest of N lies near k / N: the run's discrepancy.
            for (k, threshold) in thresholds.iter().enumerate() {
                let even = (k as f32 + 0.5) / LEDS as f32;
                assert!(
                    (threshold - even).abs() <= 2.5 / LEDS as f32,
                    "LEDs from {start}: {k}th threshold {threshold}"
                );
            }
        }
        assert_eq!(Threshold::of(0).0, 0.5 + 0.5 / (1 << 23) as f32);
        assert!((0..=u32::from(u16::MAX)).all(|led| {
            let Threshold(threshold) = Threshold::of(led);
            threshold > 0.0 && threshold < 1.0
        }));
    }
}
