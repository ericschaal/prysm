use crate::LinearColor;

/// Profile correction in linear light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorCorrection {
    gains: [f32; 3],
    tone: f32,
    intensity: i16,
    hue: [i16; 6],
}
impl Default for ColorCorrection {
    fn default() -> Self {
        Self {
            gains: [1.0; 3],
            tone: 1.0,
            intensity: 0,
            hue: [0; 6],
        }
    }
}
impl ColorCorrection {
    #[must_use]
    pub const fn gains(&self) -> [f32; 3] {
        self.gains
    }
    #[must_use]
    pub const fn tone(&self) -> f32 {
        self.tone
    }
    #[must_use]
    pub const fn intensity(&self) -> i16 {
        self.intensity
    }
    #[must_use]
    pub const fn hue(&self) -> [i16; 6] {
        self.hue
    }

    /// Controls span -100 to 100; zero is identity. Positive values warm, add
    /// magenta, and lighten midtones respectively. Gains only attenuate.
    ///
    /// # Errors
    /// Rejects controls outside -100..=100.
    pub fn new(warmth: i16, tint: i16, midtones: i16) -> Result<Self, &'static str> {
        if [warmth, tint, midtones]
            .iter()
            .any(|v| !(-100..=100).contains(v))
        {
            return Err("color correction must be between -100 and 100");
        }
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
            ..Self::default()
        })
    }

    /// Adjust chroma and six hue anchors (red, yellow, green, cyan, blue, magenta).
    ///
    /// # Errors
    /// Rejects controls outside -100..=100.
    pub fn with_chroma(mut self, intensity: i16, hue: [i16; 6]) -> Result<Self, &'static str> {
        if std::iter::once(&intensity)
            .chain(&hue)
            .any(|v| !(-100..=100).contains(v))
        {
            return Err("color correction must be between -100 and 100");
        }
        self.intensity = intensity;
        self.hue = hue;
        Ok(self)
    }

    #[allow(
        clippy::float_cmp,
        reason = "exactly equal channels represent an absent hue component"
    )]
    fn chroma(&self, color: LinearColor) -> LinearColor {
        const WHEEL: [[f32; 3]; 6] = [
            [1., 0., 0.],
            [1., 1., 0.],
            [0., 1., 0.],
            [0., 1., 1.],
            [0., 0., 1.],
            [1., 0., 1.],
        ];
        let rgb = [color.r, color.g, color.b];
        // Split the color into gray plus its two adjacent wheel anchors.
        let (hi, mid, lo, primary, secondary) = match rgb {
            [r, g, b] if r >= g && g >= b => (r, g, b, 0, 1),
            [r, g, b] if r >= b && b >= g => (r, b, g, 0, 5),
            [r, g, b] if g >= r && r >= b => (g, r, b, 2, 1),
            [r, g, b] if g >= b && b >= r => (g, b, r, 2, 3),
            [r, g, b] if b >= r && r >= g => (b, r, g, 4, 5),
            [r, g, b] => (b, g, r, 4, 3),
        };
        if hi == lo
            || (self.intensity == 0
                && (hi == mid || self.hue[primary] == 0)
                && (mid == lo || self.hue[secondary] == 0))
        {
            return color;
        }
        let mut shifted = rgb;
        for (index, weight) in [(primary, hi - mid), (secondary, mid - lo)] {
            let shift = f32::from(self.hue[index]) / 200.0;
            let neighbor = (index + if shift < 0.0 { 5 } else { 1 }) % 6;
            for (channel, value) in shifted.iter_mut().enumerate() {
                *value += weight * shift.abs() * (WHEEL[neighbor][channel] - WHEEL[index][channel]);
            }
        }
        // Scale chroma about BT.709 luminance. Limit all channels together at
        // the gamut boundary so a saturated channel cannot pull the hue around.
        let luminance = |[r, g, b]: [f32; 3]| 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let y = luminance(rgb);
        let shifted_y = luminance(shifted);
        let chroma = shifted.map(|v| v - shifted_y);
        let mut amount = 1.0 + f32::from(self.intensity) / 100.0;
        for c in chroma {
            if c > 0.0 {
                amount = amount.min((1.0 - y) / c);
            } else if c < 0.0 {
                amount = amount.min(-y / c);
            }
        }
        let [r, g, b] = chroma.map(|c| (y + c * amount).clamp(0.0, 1.0));
        LinearColor::new(r, g, b)
    }

    /// Correct the shared peak before white balance, preserving black and hue
    /// under the tone curve. The curve fixes 0 and 1 and is strictly monotonic.
    #[must_use]
    pub fn apply(&self, color: LinearColor) -> LinearColor {
        if *self == Self::default() {
            return color;
        }
        let peak = color.r.max(color.g).max(color.b);
        if peak <= 0.0 {
            return LinearColor::black();
        }
        let scale = 1.0 / (peak + (1.0 - peak) * self.tone);
        let color = color * scale;
        let color = if self.intensity == 0 && self.hue == [0; 6] {
            color
        } else {
            self.chroma(color)
        };
        LinearColor::new(
            color.r * self.gains[0],
            color.g * self.gains[1],
            color.b * self.gains[2],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chroma_preserves_neutrals_and_luminance_at_the_gamut_boundary() {
        let correction = ColorCorrection::default();
        let red = LinearColor::new(1.0, 0.0, 0.0);
        let leaf = LinearColor::new(0.15, 0.45, 0.1);
        let luma = |c: LinearColor| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
        for intensity in [-100, 0, 100] {
            let adjusted = correction
                .with_chroma(intensity, [100, -100, 100, -100, 100, -100])
                .unwrap();
            for gray in [0.0, 0.3, 1.0] {
                let input = LinearColor::new(gray, gray, gray);
                assert_eq!(adjusted.apply(input), input);
            }
            for input in [
                red,
                leaf,
                LinearColor::new(0.3, 0.6, 0.9),
                LinearColor::new(1.0, 1.0, 0.0),
            ] {
                let result = adjusted.apply(input);
                assert!((luma(result) - luma(input)).abs() < 1e-5);
                assert!(
                    [result.r, result.g, result.b]
                        .into_iter()
                        .all(|v| (0.0..=1.0).contains(&v))
                );
                if intensity == -100 {
                    assert_eq!(result.r, result.g);
                    assert_eq!(result.g, result.b);
                }
            }
        }
        let vivid = correction.with_chroma(100, [0; 6]).unwrap().apply(leaf);
        assert!(vivid.g > leaf.g && vivid.b < leaf.b);
        assert!(correction.with_chroma(101, [0; 6]).is_err());
        assert!(correction.with_chroma(0, [0, 0, 0, 0, 0, -101]).is_err());
    }
    #[test]
    fn red_hue_moves_both_sides_of_red_but_leaves_other_anchors_alone() {
        let shift = ColorCorrection::default()
            .with_chroma(0, [100, 0, 0, 0, 0, 0])
            .unwrap();
        let red = LinearColor::new(0.7, 0.1, 0.1);
        let out = shift.apply(red);
        assert!(out.g > red.g && out.r > out.g && out.g > out.b);
        for near in [
            LinearColor::new(0.7, 0.10001, 0.1),
            LinearColor::new(0.7, 0.1, 0.10001),
        ] {
            let adjacent = shift.apply(near);
            assert!((adjacent.g - out.g).abs() < 0.0001);
        }
        for other in [
            LinearColor::new(0.1, 0.8, 0.1),
            LinearColor::new(0.9, 0.1, 0.9),
        ] {
            assert_eq!(shift.apply(other), other);
        }
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
