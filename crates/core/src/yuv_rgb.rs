//! Fixed-point YCbCr to RGB for one pixel, shared by the YUYV and NV12 samplers.
use yuv::{YuvRange, YuvStandardMatrix};

/// Fixed-point fraction bits.
const PRECISION: u32 = 13;

/// Precomputed coefficients for one range and matrix. Build it once per frame
/// (or per region) and call [`Self::rgb`] per pixel. 13-bit fixed point, as
/// the `yuv` crate's balanced NV12 path: within 0.03 of a code of the exact
/// maths before rounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YuvToRgb {
    bias: i32,
    y: i32,
    r_v: i32,
    g_v: i32,
    g_u: i32,
    b_u: i32,
}

impl YuvToRgb {
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "coefficients intentionally quantized to fixed point"
    )]
    pub fn new(range: YuvRange, matrix: YuvStandardMatrix) -> Self {
        let (bias, y_scale, uv_scale) = match range {
            YuvRange::Full => (0, 1.0, 1.0),
            YuvRange::Limited => (16, 255.0 / 219.0, 255.0 / 224.0),
        };
        let coefficients = matrix.get_kr_kb();
        let (kr, kb) = (coefficients.kr, coefficients.kb);
        let kg = 1.0 - kr - kb;
        let scale = f32::from(1_u16 << PRECISION);
        let fixed = |value: f32| (value * scale).round() as i32;
        Self {
            bias,
            y: fixed(y_scale),
            r_v: fixed(2.0 * (1.0 - kr) * uv_scale),
            g_v: fixed(2.0 * (1.0 - kr) * kr / kg * uv_scale),
            g_u: fixed(2.0 * (1.0 - kb) * kb / kg * uv_scale),
            b_u: fixed(2.0 * (1.0 - kb) * uv_scale),
        }
    }

    /// One pixel from its luma and (shared) chroma samples.
    #[inline]
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "RGB is clamped to 8 bits before narrowing"
    )]
    pub fn rgb(&self, y: u8, u: u8, v: u8) -> (u8, u8, u8) {
        let y = (i32::from(y) - self.bias) * self.y;
        let u = i32::from(u) - 128;
        let v = i32::from(v) - 128;
        let round = 1 << (PRECISION - 1);
        let channel = |value: i32| ((value + round) >> PRECISION).clamp(0, 255) as u8;
        (
            channel(y + self.r_v * v),
            channel(y - self.g_v * v - self.g_u * u),
            channel(y + self.b_u * u),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standards' own definition in f64, from their published `kr`/`kb`:
    /// R and B from Cr and Cb, then G from `Y = kr R + kg G + kb B`.
    fn exact(range: YuvRange, (kr, kb): (f64, f64), y: u8, u: u8, v: u8) -> [f64; 3] {
        let (bias, y_scale, uv_scale) = match range {
            YuvRange::Full => (0.0, 1.0, 1.0),
            YuvRange::Limited => (16.0, 255.0 / 219.0, 255.0 / 224.0),
        };
        let luma = (f64::from(y) - bias) * y_scale;
        let cb = (f64::from(u) - 128.0) * uv_scale;
        let cr = (f64::from(v) - 128.0) * uv_scale;
        let r = luma + 2.0 * (1.0 - kr) * cr;
        let b = luma + 2.0 * (1.0 - kb) * cb;
        let g = (luma - kr * r - kb * b) / (1.0 - kr - kb);
        [r, g, b].map(|channel| channel.clamp(0.0, 255.0))
    }

    /// Every channel is the exact value rounded: off by at most half a code
    /// plus the fixed point's own error, never a whole code past rounding.
    #[test]
    fn matches_the_exact_maths_for_each_matrix_and_range() {
        let codes = (0..=255).step_by(5).chain([16, 128, 235]);
        for range in [YuvRange::Full, YuvRange::Limited] {
            for (matrix, k) in [
                (YuvStandardMatrix::Bt601, (0.299, 0.114)),
                (YuvStandardMatrix::Bt709, (0.2126, 0.0722)),
                (YuvStandardMatrix::Bt2020, (0.2627, 0.0593)),
                (YuvStandardMatrix::Smpte240, (0.212, 0.087)),
            ] {
                let convert = YuvToRgb::new(range, matrix);
                for y in codes.clone() {
                    for u in codes.clone() {
                        for v in codes.clone() {
                            let (r, g, b) = convert.rgb(y, u, v);
                            for (actual, exact) in
                                [r, g, b].into_iter().zip(exact(range, k, y, u, v))
                            {
                                assert!(
                                    (f64::from(actual) - exact).abs() <= 0.53,
                                    "{range:?} {matrix:?} ({y}, {u}, {v}): {actual} vs {exact}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
