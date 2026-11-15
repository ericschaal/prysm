//! sRGB codes as linear light in fixed point, for exact integer sums.
//!
//! Cell averaging adds hundreds of thousands of linear values a frame. In
//! integers that is one add per channel and pixel, with no float conversion
//! and no rounding: the sum is exact, so it is the same in any order and on
//! any number of threads, and the only rounding is the final division.

/// Fraction bits of [`LINEAR`]: a step is 2^-20 ≈ 9.5e-7, so each value is
/// within 4.8e-7 of the exact linear light, about `f32`'s own resolution near 1.
pub(crate) const FRACTION_BITS: u32 = 20;

/// Most pixels [`add_linear`] sums in `u32` before widening: each adds at
/// most 2^20 (white) to a channel, so 4,095 of them stay below 2^32.
const RUN: usize = (u32::MAX >> FRACTION_BITS) as usize;

/// Linear light of each sRGB code, as `round(linear × 2^20)`; white is 2^20.
pub(crate) const LINEAR: [u32; 256] = table();

/// Adds to `sum`, in units of 2^-20, the linear light of every `step`th of
/// `len` pixels from the first, where `len` is a slice's length and
/// `codes(i)` is pixel `i`'s red, green and blue sRGB codes. Inlined,
/// `codes` decodes each pixel in the same loop.
#[inline]
pub(crate) fn add_linear(
    sum: &mut [u64; 3],
    len: usize,
    step: usize,
    codes: impl Fn(usize) -> [u8; 3],
) {
    // A step past the end reads only the first pixel, and keeps `i + step`
    // below 2 × `len`, which fits in `usize` for a slice's length.
    let step = step.clamp(1, len.max(1));
    // A cell's row is far shorter than `RUN` pixels. Summing it in `u32`
    // keeps the loop to one 32-bit add per channel on the 32-bit TV.
    // Every pixel is the common case (pictures up to 640x360); a constant
    // step there frees a register in the loop.
    if step == 1 {
        add_every(sum, len, 1, codes);
    } else {
        add_every(sum, len, step, codes);
    }
}

#[inline(always)]
fn add_every(sum: &mut [u64; 3], len: usize, step: usize, codes: impl Fn(usize) -> [u8; 3]) {
    let run = RUN.saturating_mul(step);
    let mut start = 0;
    while start < len {
        let end = len.min(start.saturating_add(run));
        let mut partial = [0_u32; 3];
        // An index loop, unlike `step_by` on an iterator, compiles to one
        // add and compare per pixel and lets `codes` index without checks.
        let mut i = start;
        while i < end {
            for (partial, code) in partial.iter_mut().zip(codes(i)) {
                *partial += LINEAR[usize::from(code)];
            }
            i += step;
        }
        for (sum, partial) in sum.iter_mut().zip(partial) {
            *sum += u64::from(partial);
        }
        start = end;
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "linear light in 0..=1 scaled by 2^20 fits u32; adding a half before `as` rounds"
)]
const fn table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut code = 0;
    while code < table.len() {
        let linear = srgb_to_linear(code as f64 / 255.0);
        table[code] = (linear * (1 << FRACTION_BITS) as f64 + 0.5) as u32;
        code += 1;
    }
    table
}

/// The IEC 61966-2-1 decoding of an sRGB value in 0..=1, in `f64`.
const fn srgb_to_linear(srgb: f64) -> f64 {
    if srgb <= 0.04045 {
        srgb / 12.92
    } else {
        // base^2.4 = base^2 × (base^2)^(1/5); `powf` is not `const`.
        let base = (srgb + 0.055) / 1.055;
        let square = base * base;
        square * fifth_root(square)
    }
}

/// The fifth root of `value` in (0, 1], by Newton's method from 1: each step
/// lowers the estimate towards the root until rounding stops it.
const fn fifth_root(value: f64) -> f64 {
    let mut root = 1.0;
    loop {
        let fourth = root * root * root * root;
        let next = (4.0 * root + value / fourth) / 5.0;
        if next >= root {
            return root;
        }
        root = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{Color, LinearColor};

    fn exact(code: u8) -> f64 {
        let srgb = f64::from(code) / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    }

    #[test]
    fn table_is_the_exact_decoding_rounded_to_its_fixed_point() {
        let scale = f64::from(1 << FRACTION_BITS);
        for code in 0..=255 {
            let fixed = LINEAR[usize::from(code)];
            assert_eq!(f64::from(fixed), (exact(code) * scale).round(), "{code}");
            assert!((f64::from(fixed) / scale - exact(code)).abs() <= 0.5 / scale);
            // The f32 table the rest of Prysm uses agrees to its 7 decimals.
            let float = LinearColor::from_srgb(Color::new(code, 0, 0)).r;
            assert!((f64::from(fixed) / scale - f64::from(float)).abs() < 6e-7);
        }
        assert_eq!((LINEAR[0], LINEAR[255]), (0, 1 << FRACTION_BITS));
    }

    #[test]
    fn sums_are_exact_past_one_u32_run_at_every_step() {
        let len = 3 * RUN + 7;
        let white = |_| [255, 0, 255];
        for step in [1, 2, 3, 7, len - 1, len, len + 1, usize::MAX] {
            let mut sum = [1, 2, 3];
            add_linear(&mut sum, len, step, white);
            let read = len.div_ceil(step.min(len));
            let total = u64::try_from(read).unwrap() << FRACTION_BITS;
            assert_eq!(sum, [total + 1, 2, total + 3], "step {step}");
        }
        let mut sum = [0; 3];
        add_linear(&mut sum, 0, 1, |_| unreachable!("no pixels"));
        let codes = [[1, 2, 3], [9, 9, 9], [4, 5, 6]];
        add_linear(&mut sum, codes.len(), 2, |i| codes[i]);
        let code = |code: usize| u64::from(LINEAR[code]);
        assert_eq!(
            sum,
            [code(1) + code(4), code(2) + code(5), code(3) + code(6)]
        );
    }
}
