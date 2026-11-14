use crate::LinearColor;

/// The identity matrix: every picture primary drives only its own LED channel.
const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

// How far each primary's controls mix other LED channels into it, at ±100.
// The mix grows with the square of the control, so the first steps near 0
// are fine (10 mixes 1 % of the maximum) while the ends leave room beyond
// the few tenths of a neighbouring channel that matching an LED strip's
// purer primaries to the picture's is expected to take (not yet measured).
// Before rows are normalised a full turn or calm mixes 0.5, which leaves an
// off-diagonal term of 1/3 (1/2 for both together). Strengthening subtracts
// and is clipped at the gamut, so it is kept smaller: at 0.25 each row's sum
// stays at least 0.5, well away from zero, whatever the controls are.
const MAX_TURN: f32 = 0.5;
const MAX_CALM: f32 = 0.5;
const MAX_STRENGTHEN: f32 = 0.25;

/// One picture primary's controls, each -100 to 100; zero leaves it as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrimaryAdjustment {
    /// Positive turns the primary toward the next one in red, green, blue
    /// order (red toward green, green toward blue, blue toward red), negative
    /// toward the previous one.
    pub hue: i16,
    /// Negative calms the primary by mixing both other channels into it,
    /// positive strengthens it.
    pub strength: i16,
}

impl PrimaryAdjustment {
    /// The amount of each LED channel to add for this picture primary, at
    /// index `primary` (0 red, 1 green, 2 blue), before rows are normalised.
    fn mixes(self, primary: usize) -> [f32; 3] {
        let next = (primary + 1) % 3;
        let previous = (primary + 2) % 3;
        let mut mix = [0.0; 3];
        let turn = MAX_TURN * curve(self.hue);
        mix[if self.hue > 0 { next } else { previous }] += turn;
        let calm = if self.strength < 0 {
            MAX_CALM
        } else {
            -MAX_STRENGTHEN
        } * curve(self.strength);
        mix[next] += calm;
        mix[previous] += calm;
        mix
    }
}

/// A control's weight, 0 at 0 to 1 at ±100, growing with its square.
fn curve(control: i16) -> f32 {
    let t = f32::from(control) / 100.0;
    t * t
}

/// A calibration's correction in linear light: a tone curve on the shared
/// peak, then one 3x3 matrix from picture to LED channels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorCorrection {
    /// White balance, kept to rebuild the matrix with other primaries.
    gains: [f32; 3],
    tone: f32,
    /// `LED = matrix · picture`: the white-balance gains (a diagonal) times
    /// the primaries' matrix, whose rows each sum to 1.
    matrix: [[f32; 3]; 3],
}

impl Default for ColorCorrection {
    fn default() -> Self {
        Self {
            gains: [1.0; 3],
            tone: 1.0,
            matrix: IDENTITY,
        }
    }
}

/// The white-balance gains applied after the primaries: `diag(gains) · primaries`.
fn balanced(gains: [f32; 3], mut primaries: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    for (row, gain) in primaries.iter_mut().zip(gains) {
        for value in row {
            *value *= gain;
        }
    }
    primaries
}

fn in_range(controls: &[i16]) -> Result<(), &'static str> {
    if controls.iter().all(|v| (-100..=100).contains(v)) {
        Ok(())
    } else {
        Err("color correction must be between -100 and 100")
    }
}

impl ColorCorrection {
    /// Controls span -100 to 100; zero is identity. Positive values warm, add
    /// magenta, and lighten midtones respectively. Gains only attenuate.
    /// The primaries are left as they are.
    ///
    /// # Errors
    /// Rejects controls outside -100..=100.
    pub fn new(warmth: i16, tint: i16, midtones: i16) -> Result<Self, &'static str> {
        in_range(&[warmth, tint, midtones])?;
        let mut gains = [
            (f32::from(warmth) / 200.0).exp2(),
            (-f32::from(tint) / 200.0).exp2(),
            (-f32::from(warmth) / 200.0).exp2(),
        ];
        let maximum = gains.iter().copied().fold(1.0_f32, f32::max);
        for gain in &mut gains {
            *gain /= maximum;
        }
        Ok(Self {
            gains,
            tone: (-f32::from(midtones) / 100.0).exp2(),
            matrix: balanced(gains, IDENTITY),
        })
    }

    /// Adjust the red, green and blue primaries. Each row of the primaries'
    /// matrix is scaled to sum to 1, so white and greys stay exactly as the
    /// white balance leaves them: adjusting a primary never moves white.
    ///
    /// # Errors
    /// Rejects controls outside -100..=100.
    pub fn with_primaries(self, primaries: [PrimaryAdjustment; 3]) -> Result<Self, &'static str> {
        in_range(&primaries.map(|p| p.hue))?;
        in_range(&primaries.map(|p| p.strength))?;
        // Column p is what picture primary p lights: its own channel plus
        // the mixes its controls add.
        let mut matrix = IDENTITY;
        for (column, primary) in primaries.iter().enumerate() {
            for (row, mix) in primary.mixes(column).into_iter().enumerate() {
                matrix[row][column] += mix;
            }
        }
        for row in &mut matrix {
            // At least 1 - 2 * MAX_STRENGTHEN: never zero. Exactly 1 for a
            // row no control touches, so all zero stays the identity.
            let sum: f32 = row.iter().sum();
            for value in row {
                *value /= sum;
            }
        }
        Ok(Self {
            matrix: balanced(self.gains, matrix),
            ..self
        })
    }

    /// Correct the shared peak before the matrix, preserving black and hue
    /// under the tone curve. The curve fixes 0 and 1 and is strictly
    /// monotonic. The result is clipped to 0–1, where a strengthened primary
    /// leaves the LEDs' gamut.
    #[must_use]
    pub fn apply(&self, color: LinearColor) -> LinearColor {
        if *self == Self::default() {
            return color;
        }
        let peak = color.r.max(color.g).max(color.b);
        if peak <= 0.0 {
            return LinearColor::black();
        }
        let c = color * (1.0 / (peak + (1.0 - peak) * self.tone));
        let [r, g, b] = self
            .matrix
            .map(|[red, green, blue]| (red * c.r + green * c.g + blue * c.b).clamp(0.0, 1.0));
        LinearColor::new(r, g, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARIES: [LinearColor; 3] = [
        LinearColor {
            r: 1.0,
            g: 0.0,
            b: 0.0,
        },
        LinearColor {
            r: 0.0,
            g: 1.0,
            b: 0.0,
        },
        LinearColor {
            r: 0.0,
            g: 0.0,
            b: 1.0,
        },
    ];

    fn primary(index: usize, hue: i16, strength: i16) -> ColorCorrection {
        let mut primaries = [PrimaryAdjustment::default(); 3];
        primaries[index] = PrimaryAdjustment { hue, strength };
        ColorCorrection::default()
            .with_primaries(primaries)
            .unwrap()
    }

    fn channels(color: LinearColor) -> [f32; 3] {
        [color.r, color.g, color.b]
    }

    /// A spread of controls at each primary, including the ends.
    fn every_primaries() -> impl Iterator<Item = [PrimaryAdjustment; 3]> {
        let values = [-100, -37, 0, 12, 100];
        values.into_iter().flat_map(move |hue| {
            values.into_iter().flat_map(move |strength| {
                (0..3).map(move |rotate| {
                    let mut primaries = [
                        PrimaryAdjustment { hue, strength },
                        PrimaryAdjustment {
                            hue: -strength,
                            strength: hue,
                        },
                        PrimaryAdjustment {
                            hue: strength,
                            strength: -hue,
                        },
                    ];
                    primaries.rotate_left(rotate);
                    primaries
                })
            })
        })
    }

    #[test]
    fn zero_primaries_are_bit_exact_identity() {
        let zero = ColorCorrection::new(0, 0, 0)
            .unwrap()
            .with_primaries([PrimaryAdjustment::default(); 3])
            .unwrap();
        assert_eq!(zero, ColorCorrection::default());
        // With white balance only, the matrix is the gains' diagonal.
        let warm = ColorCorrection::new(40, -20, 0).unwrap();
        let same = warm
            .with_primaries([PrimaryAdjustment::default(); 3])
            .unwrap();
        assert_eq!(same, warm);
        for step in 0..=100_u8 {
            let x = f32::from(step) / 100.0;
            let input = LinearColor::new(x, x * 0.3, 1.0 - x);
            assert_eq!(zero.apply(input), input);
            let [r, g, b] = warm.gains;
            let want = [input.r * r, input.g * g, input.b * b];
            for (got, want) in channels(warm.apply(input)).into_iter().zip(want) {
                assert!((got - want).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn white_and_greys_stay_as_the_white_balance_leaves_them() {
        let balance = ColorCorrection::new(30, -15, 0).unwrap();
        for primaries in every_primaries() {
            let corrected = balance.with_primaries(primaries).unwrap();
            for gray in [0.0, 0.05, 0.3, 0.75, 1.0] {
                let input = LinearColor::new(gray, gray, gray);
                let (got, want) = (corrected.apply(input), balance.apply(input));
                for (a, b) in channels(got).into_iter().zip(channels(want)) {
                    assert!((a - b).abs() < 1e-6, "{primaries:?} {gray}: {got:?}");
                }
            }
        }
    }

    #[test]
    fn a_primary_moves_only_its_own_column() {
        // Red's controls change what pure red lights. Pure green and pure blue
        // stay on their own channel; only the row normalisation scales them,
        // by 1 / (1 + red's mix into that channel), so they may dim, never
        // change colour.
        for index in 0..3 {
            for (hue, strength) in [(100, 0), (-60, 0), (0, -100), (0, 100), (45, -70)] {
                let corrected = primary(index, hue, strength);
                for other in (0..3).filter(|&other| other != index) {
                    let out = channels(corrected.apply(PRIMARIES[other]));
                    for (channel, value) in out.into_iter().enumerate() {
                        if channel == other {
                            assert!(value > 0.5 && value <= 1.0, "{index} {hue} {strength}");
                        } else {
                            assert_eq!(value, 0.0, "{index} {hue} {strength}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn each_control_turns_or_calms_its_primary_the_way_it_says() {
        let [red, green, blue] = PRIMARIES.map(channels);
        // Positive hue mixes in the next primary: red more orange (green),
        // green more turquoise (blue), blue more purple (red).
        let out = channels(primary(0, 50, 0).apply(PRIMARIES[0]));
        assert!(out[1] > 0.0 && out[2] == 0.0 && out[0] == red[0]);
        let out = channels(primary(1, 50, 0).apply(PRIMARIES[1]));
        assert!(out[2] > 0.0 && out[0] == 0.0 && out[1] == green[1]);
        let out = channels(primary(2, 50, 0).apply(PRIMARIES[2]));
        assert!(out[0] > 0.0 && out[1] == 0.0 && out[2] == blue[2]);
        // Negative hue mixes in the previous one: red more pink (blue),
        // green more yellow (red), blue more turquoise (green).
        let out = channels(primary(0, -50, 0).apply(PRIMARIES[0]));
        assert!(out[2] > 0.0 && out[1] == 0.0);
        let out = channels(primary(1, -50, 0).apply(PRIMARIES[1]));
        assert!(out[0] > 0.0 && out[2] == 0.0);
        let out = channels(primary(2, -50, 0).apply(PRIMARIES[2]));
        assert!(out[1] > 0.0 && out[0] == 0.0);

        // Softer mixes both others in equally; stronger takes them out.
        let moderate = LinearColor::new(0.6, 0.2, 0.2);
        let softer = primary(0, 0, -50).apply(moderate);
        assert!(softer.g > moderate.g && (softer.g - softer.b).abs() < 1e-6);
        let stronger = primary(0, 0, 50).apply(moderate);
        assert!(stronger.g < moderate.g && (stronger.g - stronger.b).abs() < 1e-6);
        // The most a primary can be calmed still leaves it clearly itself.
        let calmest = primary(0, 0, -100).apply(PRIMARIES[0]);
        assert!(calmest.r == 1.0 && calmest.g > 0.3 && calmest.g < 0.5);

        // Fine steps near zero, larger toward the ends.
        let mix = |control| primary(0, control, 0).apply(PRIMARIES[0]).g;
        assert!(mix(1) > 0.0 && mix(1) < 1e-4);
        assert!(mix(10) < 0.01 && mix(100) > 0.3);
    }

    #[test]
    fn the_matrix_is_linear_before_clipping() {
        let corrected = ColorCorrection::new(20, 10, 0)
            .unwrap()
            .with_primaries([
                PrimaryAdjustment {
                    hue: 40,
                    strength: -30,
                },
                PrimaryAdjustment {
                    hue: -25,
                    strength: -60,
                },
                PrimaryAdjustment {
                    hue: 70,
                    strength: 0,
                },
            ])
            .unwrap();
        let red = LinearColor::new(0.5, 0.0, 0.0);
        let green = LinearColor::new(0.0, 0.4, 0.0);
        let blue = LinearColor::new(0.0, 0.0, 0.3);
        for (a, b) in [(red, green), (green, blue), (red, blue)] {
            let sum = corrected.apply(LinearColor::new(a.r + b.r, a.g + b.g, a.b + b.b));
            let (a, b) = (channels(corrected.apply(a)), channels(corrected.apply(b)));
            for (channel, total) in channels(sum).into_iter().enumerate() {
                assert!((total - a[channel] - b[channel]).abs() < 1e-6, "{sum:?}");
            }
        }
    }

    #[test]
    fn output_stays_within_the_lights_range() {
        for primaries in every_primaries() {
            for midtones in [-100, 0, 100] {
                let corrected = ColorCorrection::new(-80, 60, midtones)
                    .unwrap()
                    .with_primaries(primaries)
                    .unwrap();
                for r in 0..=4_u8 {
                    for g in 0..=4_u8 {
                        for b in 0..=4_u8 {
                            let input =
                                LinearColor::new(f32::from(r), f32::from(g), f32::from(b)) / 4.0;
                            let out = channels(corrected.apply(input));
                            assert!(out.iter().all(|v| (0.0..=1.0).contains(v)), "{out:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn controls_outside_their_range_are_refused() {
        let base = ColorCorrection::default();
        let one = |hue, strength| {
            let mut primaries = [PrimaryAdjustment::default(); 3];
            primaries[2] = PrimaryAdjustment { hue, strength };
            base.with_primaries(primaries)
        };
        assert!(one(100, -100).is_ok());
        assert!(one(101, 0).is_err());
        assert!(one(0, -101).is_err());
        assert!(one(i16::MIN, 0).is_err());
    }

    #[test]
    fn identity_black_endpoints_monotonicity_and_bounds() {
        for warmth in [-100, 0, 100] {
            for tint in [-100, 0, 100] {
                for midtones in [-100, 0, 100] {
                    let correction = ColorCorrection::new(warmth, tint, midtones).unwrap();
                    assert_eq!(correction.apply(LinearColor::black()), LinearColor::black());
                    assert_eq!(
                        correction.apply(LinearColor::new(1.0, 1.0, 1.0)),
                        ColorCorrection::new(warmth, tint, 0)
                            .unwrap()
                            .apply(LinearColor::new(1.0, 1.0, 1.0))
                    );
                    let mut previous = LinearColor::black();
                    for step in 0..=1000_u16 {
                        let x = f32::from(step) / 1000.0;
                        let input = LinearColor::new(x, x * 0.5, x * 0.1);
                        let output = correction.apply(input);
                        for (a, b) in [
                            (previous.r, output.r),
                            (previous.g, output.g),
                            (previous.b, output.b),
                        ] {
                            assert!(b >= a && (0.0..=1.0).contains(&b));
                        }
                        if warmth == 0 && tint == 0 && midtones == 0 {
                            assert_eq!(output, input);
                        }
                        if warmth == 0 && tint == 0 {
                            assert!((output.g - output.r * 0.5).abs() < 1e-6);
                            assert!((output.b - output.r * 0.1).abs() < 1e-6);
                        }
                        previous = output;
                    }
                }
            }
        }
        assert!(ColorCorrection::new(101, 0, 0).is_err());
        assert!(ColorCorrection::new(0, -101, 0).is_err());
        assert!(ColorCorrection::new(0, 0, i16::MAX).is_err());
    }
}
