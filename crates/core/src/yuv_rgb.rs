//! Fixed-point YCbCr to RGB for one pixel, shared by the YUYV and NV12 samplers.
use yuv::{YuvRange, YuvStandardMatrix};

/// Precomputed coefficients for one range and matrix. Build it once per frame
/// (or per row) and call [`Self::rgb`] per pixel. [`Self::new`] uses the
/// `yuv` crate's 13-bit fixed point (its balanced NV12 path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YuvToRgb {
    shift: u32,
    bias: i32,
    y: i32,
    r_v: i32,
    g_v: i32,
    g_u: i32,
    b_u: i32,
}

impl YuvToRgb {
    #[must_use]
    pub fn new(range: YuvRange, matrix: YuvStandardMatrix) -> Self {
        Self::with_precision(range, matrix, 13)
    }

    /// `precision` fixed-point bits: 6 matches the `yuv` crate's fast YUYV path.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "coefficients intentionally quantized to fixed point"
    )]
    pub(crate) fn with_precision(
        range: YuvRange,
        matrix: YuvStandardMatrix,
        precision: u32,
    ) -> Self {
        let (bias, y_scale, uv_scale) = match range {
            YuvRange::Full => (0, 1.0, 1.0),
            YuvRange::Limited => (16, 255.0 / 219.0, 255.0 / 224.0),
        };
        let coefficients = matrix.get_kr_kb();
        let (kr, kb) = (coefficients.kr, coefficients.kb);
        let kg = 1.0 - kr - kb;
        let scale = (1_i32 << precision) as f32;
        let fixed = |value: f32| (value * scale).round() as i32;
        Self {
            shift: precision,
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
        let round = 1 << (self.shift - 1);
        let channel = |value: i32| ((value + round) >> self.shift).clamp(0, 255) as u8;
        (
            channel(y + self.r_v * v),
            channel(y - self.g_v * v - self.g_u * u),
            channel(y + self.b_u * u),
        )
    }
}
