use crate::Color;
use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign};

/// Color in linear RGB space (0.0-1.0 normalized).
/// All processing math (blending, averaging, scaling) uses this type.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LinearColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

/// Precomputed sRGB-to-linear lookup table (IEC 61966-2-1).
/// Maps each u8 sRGB value to its linear [0.0, 1.0] equivalent.
/// Generated from: if srgb <= 0.04045 { srgb / 12.92 } else { ((srgb + 0.055) / 1.055)^2.4 }
#[allow(clippy::unreadable_literal)]
const SRGB_TO_LINEAR: [f32; 256] = [
    0.0, 0.0003035, 0.0006071, 0.0009106, 0.0012141, 0.0015176,
    0.0018212, 0.0021247, 0.0024282, 0.0027317, 0.0030353, 0.0033465,
    0.0036765, 0.0040247, 0.0043914, 0.0047770, 0.0051815, 0.0056054,
    0.0060488, 0.0065121, 0.0069954, 0.0074990, 0.0080232, 0.0085681,
    0.0091341, 0.0097212, 0.0103298, 0.0109601, 0.0116122, 0.0122865,
    0.0129830, 0.0137021, 0.0144438, 0.0152085, 0.0159963, 0.0168074,
    0.0176420, 0.0185002, 0.0193824, 0.0202886, 0.0212190, 0.0221739,
    0.0231534, 0.0241576, 0.0251869, 0.0262412, 0.0273209, 0.0284260,
    0.0295568, 0.0307134, 0.0318960, 0.0331048, 0.0343398, 0.0356013,
    0.0368895, 0.0382044, 0.0395462, 0.0409152, 0.0423114, 0.0437350,
    0.0451862, 0.0466651, 0.0481718, 0.0497066, 0.0512695, 0.0528606,
    0.0544803, 0.0561285, 0.0578054, 0.0595112, 0.0612461, 0.0630100,
    0.0648033, 0.0666259, 0.0684782, 0.0703601, 0.0722719, 0.0742136,
    0.0761854, 0.0781874, 0.0802198, 0.0822827, 0.0843762, 0.0865005,
    0.0886556, 0.0908417, 0.0930590, 0.0953075, 0.0975873, 0.0998987,
    0.1022417, 0.1046165, 0.1070231, 0.1094617, 0.1119324, 0.1144354,
    0.1169707, 0.1195384, 0.1221388, 0.1247718, 0.1274377, 0.1301365,
    0.1328683, 0.1356333, 0.1384316, 0.1412633, 0.1441285, 0.1470273,
    0.1499598, 0.1529262, 0.1559265, 0.1589608, 0.1620294, 0.1651322,
    0.1682694, 0.1714411, 0.1746474, 0.1778884, 0.1811642, 0.1844750,
    0.1878208, 0.1912017, 0.1946178, 0.1980693, 0.2015563, 0.2050787,
    0.2086369, 0.2122308, 0.2158605, 0.2195262, 0.2232280, 0.2269659,
    0.2307400, 0.2345506, 0.2383976, 0.2422811, 0.2462013, 0.2501583,
    0.2541521, 0.2581829, 0.2622507, 0.2663556, 0.2704978, 0.2746773,
    0.2788943, 0.2831487, 0.2874408, 0.2917706, 0.2961383, 0.3005438,
    0.3049873, 0.3094689, 0.3139887, 0.3185468, 0.3231432, 0.3277781,
    0.3324515, 0.3371636, 0.3419144, 0.3467041, 0.3515326, 0.3564001,
    0.3613068, 0.3662526, 0.3712377, 0.3762621, 0.3813260, 0.3864294,
    0.3915725, 0.3967552, 0.4019778, 0.4072402, 0.4125426, 0.4178851,
    0.4232677, 0.4286905, 0.4341536, 0.4396572, 0.4452012, 0.4507858,
    0.4564110, 0.4620770, 0.4677838, 0.4735315, 0.4793202, 0.4851499,
    0.4910208, 0.4969330, 0.5028865, 0.5088813, 0.5149177, 0.5209956,
    0.5271151, 0.5332764, 0.5394795, 0.5457245, 0.5520114, 0.5583404,
    0.5647115, 0.5711248, 0.5775804, 0.5840784, 0.5906188, 0.5972018,
    0.6038273, 0.6104956, 0.6172066, 0.6239604, 0.6307571, 0.6375969,
    0.6444797, 0.6514056, 0.6583748, 0.6653873, 0.6724432, 0.6795425,
    0.6866853, 0.6938718, 0.7011019, 0.7083758, 0.7156935, 0.7230551,
    0.7304607, 0.7379104, 0.7454042, 0.7529422, 0.7605245, 0.7681511,
    0.7758222, 0.7835378, 0.7912979, 0.7991027, 0.8069523, 0.8148466,
    0.8227858, 0.8307699, 0.8387990, 0.8468732, 0.8549926, 0.8631572,
    0.8713671, 0.8796224, 0.8879231, 0.8962694, 0.9046612, 0.9130987,
    0.9215819, 0.9301109, 0.9386857, 0.9473065, 0.9559734, 0.9646862,
    0.9734453, 0.9822506, 0.9911021, 1.0,
];

impl LinearColor {
    pub fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    pub fn black() -> Self {
        Self {
            r: 0.0,
            g: 0.0,
            b: 0.0,
        }
    }

    /// Convert an sRGB u8 color to linear RGB via const LUT.
    pub fn from_srgb(color: Color) -> Self {
        Self {
            r: SRGB_TO_LINEAR[color.r as usize],
            g: SRGB_TO_LINEAR[color.g as usize],
            b: SRGB_TO_LINEAR[color.b as usize],
        }
    }

    /// Convert linear RGB back to sRGB u8 using the IEC 61966-2-1 transfer function.
    pub fn to_srgb(&self) -> Color {
        Color {
            r: linear_to_srgb_u8(self.r),
            g: linear_to_srgb_u8(self.g),
            b: linear_to_srgb_u8(self.b),
        }
    }

    /// Linearly interpolate between two colors.
    /// ratio=0.0 returns self, ratio=1.0 returns other.
    pub fn blend(&self, other: &LinearColor, ratio: f32) -> LinearColor {
        let ratio = ratio.clamp(0.0, 1.0);
        let inv = 1.0 - ratio;
        LinearColor {
            r: self.r * inv + other.r * ratio,
            g: self.g * inv + other.g * ratio,
            b: self.b * inv + other.b * ratio,
        }
    }
}

/// Apply the sRGB companding function to a single linear channel and round to u8.
fn linear_to_srgb_u8(linear: f32) -> u8 {
    let clamped = linear.clamp(0.0, 1.0);
    let srgb = if clamped <= 0.0031308 {
        clamped * 12.92
    } else {
        1.055 * clamped.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0 + 0.5) as u8
}

// Arithmetic trait implementations

impl Add for LinearColor {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            r: self.r + other.r,
            g: self.g + other.g,
            b: self.b + other.b,
        }
    }
}

impl AddAssign for LinearColor {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for LinearColor {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            r: self.r - other.r,
            g: self.g - other.g,
            b: self.b - other.b,
        }
    }
}

impl SubAssign for LinearColor {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

impl Mul<f32> for LinearColor {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        Self {
            r: self.r * scalar,
            g: self.g * scalar,
            b: self.b * scalar,
        }
    }
}

impl MulAssign<f32> for LinearColor {
    fn mul_assign(&mut self, scalar: f32) {
        *self = *self * scalar;
    }
}

impl Div<f32> for LinearColor {
    type Output = Self;

    fn div(self, scalar: f32) -> Self {
        Self {
            r: self.r / scalar,
            g: self.g / scalar,
            b: self.b / scalar,
        }
    }
}

impl DivAssign<f32> for LinearColor {
    fn div_assign(&mut self, scalar: f32) {
        *self = *self / scalar;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_identity() {
        // Every sRGB u8 value should round-trip through LinearColor within +/-1
        for r in 0..=255u8 {
            let color = Color::new(r, r, r);
            let linear = LinearColor::from_srgb(color);
            let back = linear.to_srgb();
            assert!(
                (back.r as i16 - r as i16).unsigned_abs() <= 1,
                "Round-trip failed for sRGB {r}: got {}",
                back.r
            );
        }
    }

    #[test]
    fn known_values() {
        // sRGB 0 -> linear 0.0
        let black = LinearColor::from_srgb(Color::new(0, 0, 0));
        assert_eq!(black.r, 0.0);

        // sRGB 255 -> linear 1.0
        let white = LinearColor::from_srgb(Color::new(255, 255, 255));
        assert_eq!(white.r, 1.0);

        // sRGB 128 -> ~0.2158 linear
        let mid = LinearColor::from_srgb(Color::new(128, 128, 128));
        assert!((mid.r - 0.2158).abs() < 0.001, "sRGB 128 linear = {}", mid.r);
    }

    #[test]
    fn linear_blend_midpoint() {
        let black = LinearColor::black();
        let white = LinearColor::new(1.0, 1.0, 1.0);
        let mid = black.blend(&white, 0.5);

        // Linear midpoint 0.5 should map to sRGB ~188 (not 128)
        let srgb = mid.to_srgb();
        assert!(
            (srgb.r as i16 - 188).unsigned_abs() <= 1,
            "Linear 0.5 -> sRGB {}, expected ~188",
            srgb.r
        );
    }

    #[test]
    fn blend_endpoints() {
        let a = LinearColor::new(0.2, 0.4, 0.6);
        let b = LinearColor::new(0.8, 0.1, 0.3);

        let at_zero = a.blend(&b, 0.0);
        assert_eq!(at_zero, a);

        let at_one = a.blend(&b, 1.0);
        assert_eq!(at_one, b);
    }

    #[test]
    fn arithmetic_ops() {
        let a = LinearColor::new(0.2, 0.3, 0.4);
        let b = LinearColor::new(0.1, 0.2, 0.3);

        let sum = a + b;
        assert!((sum.r - 0.3).abs() < 1e-6);
        assert!((sum.g - 0.5).abs() < 1e-6);
        assert!((sum.b - 0.7).abs() < 1e-6);

        let scaled = a * 2.0;
        assert!((scaled.r - 0.4).abs() < 1e-6);

        let divided = a / 2.0;
        assert!((divided.r - 0.1).abs() < 1e-6);
    }
}
