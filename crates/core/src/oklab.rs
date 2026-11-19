use crate::LinearColor;

/// A colour in Oklab (Björn Ottosson, 2020): lightness `l` (0 black, 1 white)
/// and the opponent axes `a` (green to red) and `b` (blue to yellow).
///
/// Distances here follow perceived differences far better than in linear
/// RGB, so they can say how large a change *looks*.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

impl Oklab {
    /// About the smallest distance a viewer notices between two colours side
    /// by side, the usual figure for Oklab.
    pub const JUST_NOTICEABLE: f32 = 0.02;

    /// Converts linear sRGB with Ottosson's published matrices.
    #[must_use]
    #[allow(
        clippy::excessive_precision,
        clippy::unreadable_literal,
        reason = "the published Oklab matrices, digit for digit"
    )]
    pub fn from_linear(color: LinearColor) -> Self {
        let LinearColor { r, g, b } = color;
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        Self {
            l: 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
            a: 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
            b: 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
        }
    }

    /// Converts back to linear sRGB. A mix of two displayable colours in
    /// Oklab can fall just outside the sRGB gamut; negative light cannot be
    /// shown, so each channel is clamped to zero.
    #[must_use]
    #[allow(
        clippy::excessive_precision,
        clippy::unreadable_literal,
        reason = "the published Oklab matrices, digit for digit"
    )]
    pub fn to_linear(self) -> LinearColor {
        let Self { l, a, b } = self;
        let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
        let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
        let s_ = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
        LinearColor {
            r: (4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_).max(0.0),
            g: (-1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_).max(0.0),
            b: (-0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_).max(0.0),
        }
    }

    /// Euclidean distance: how different the two colours look.
    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        let (dl, da, db) = (self.l - other.l, self.a - other.a, self.b - other.b);
        (dl * dl + da * da + db * db).sqrt()
    }
}

impl From<LinearColor> for Oklab {
    fn from(color: LinearColor) -> Self {
        Self::from_linear(color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    #[test]
    fn white_black_and_gray_sit_on_the_lightness_axis() {
        let white = Oklab::from_linear(LinearColor::new(1.0, 1.0, 1.0));
        assert!((white.l - 1.0).abs() < 1e-4, "{white:?}");
        assert!(white.a.abs() < 1e-4 && white.b.abs() < 1e-4, "{white:?}");
        assert_eq!(Oklab::from_linear(LinearColor::black()), Oklab::default());
        let gray = Oklab::from_linear(LinearColor::new(0.2, 0.2, 0.2));
        assert!(gray.a.abs() < 1e-4 && gray.b.abs() < 1e-4, "{gray:?}");
    }

    #[test]
    fn published_reference_values() {
        // Ottosson's table: linear sRGB red is L 0.628, a 0.225, b 0.126.
        let red = Oklab::from_linear(LinearColor::new(1.0, 0.0, 0.0));
        assert!((red.l - 0.627_955).abs() < 1e-4, "{red:?}");
        assert!((red.a - 0.224_863).abs() < 1e-4, "{red:?}");
        assert!((red.b - 0.125_846).abs() < 1e-4, "{red:?}");
    }

    #[test]
    fn every_srgb_color_round_trips() {
        for r in (0..=255u8).step_by(5) {
            for g in (0..=255u8).step_by(5) {
                for b in (0..=255u8).step_by(5) {
                    let linear = LinearColor::from_srgb(Color::new(r, g, b));
                    let back = Oklab::from_linear(linear).to_linear();
                    for (x, y) in [(linear.r, back.r), (linear.g, back.g), (linear.b, back.b)] {
                        assert!((x - y).abs() < 1e-5, "{r},{g},{b}: {linear:?} -> {back:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn colors_outside_srgb_come_back_without_negative_light() {
        // Far beyond saturated green: some channel would be negative.
        let outside = Oklab {
            l: 0.5,
            a: -0.4,
            b: 0.0,
        };
        let back = outside.to_linear();
        assert!(back.r >= 0.0 && back.g >= 0.0 && back.b >= 0.0, "{back:?}");
        assert_eq!(back.r, 0.0);
    }

    #[test]
    fn distance_is_symmetric_and_zero_for_equal_colors() {
        let a = Oklab::from_linear(LinearColor::new(0.3, 0.1, 0.05));
        let b = Oklab::from_linear(LinearColor::new(0.1, 0.2, 0.4));
        assert_eq!(a.distance(a), 0.0);
        assert_eq!(a.distance(b), b.distance(a));
        assert!(a.distance(b) > Oklab::JUST_NOTICEABLE);
    }
}
