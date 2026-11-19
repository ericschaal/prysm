//! Premultiplied BGRA (the TV's graphics plane) to limited-range BT.709 NV12,
//! the format the video tap delivers, so consumers see one format.
//!
//! The source is uncached device memory: each row is read exactly once, in
//! order, with one bulk copy into a small cached row buffer, and converted
//! from there straight into the output frame. There is no full-frame
//! intermediate copy.
use prysm_ports::{Error, Result};

/// BT.709 coefficients scaled by 219/255 (Y) or 224/255 (Cb, Cr), in 1/65536.
/// Chroma rows sum to zero so greys stay neutral; Y sums so white is 235.
const Y: [i32; 3] = [11966, 40254, 4064];
const CB: [i32; 3] = [-6596, -22188, 28784];
const CR: [i32; 3] = [28784, -26145, -2639];

/// Reusable scratch for [`Nv12Converter::convert`].
#[derive(Debug, Default)]
pub(crate) struct Nv12Converter {
    row: Vec<u8>,
    /// Per 2×2 block: summed R, G, B of the current row pair.
    sums: Vec<[i32; 3]>,
}

impl Nv12Converter {
    /// Convert a `width`×`height` plane of premultiplied BGRA rows `pitch`
    /// bytes apart into packed NV12 (`output`: Y plane, then interleaved
    /// CbCr). Premultiplied colour over black is the colour itself, so alpha
    /// is not read: a transparent plane is black. Chroma averages each 2×2
    /// block.
    pub(crate) fn convert(
        &mut self,
        pixels: &[u8],
        pitch: usize,
        width: usize,
        height: usize,
        output: &mut [u8],
    ) -> Result<()> {
        let row_bytes = width * 4;
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(Error::other("TV UI plane size is not even"));
        }
        if pitch < row_bytes || pixels.len() < pitch * (height - 1) + row_bytes {
            return Err(Error::other("TV UI plane is shorter than its geometry"));
        }
        if output.len() != width * height * 3 / 2 {
            return Err(Error::other("NV12 output has the wrong size"));
        }
        self.row.resize(row_bytes, 0);
        self.sums.resize(width / 2, [0; 3]);
        let (luma, chroma) = output.split_at_mut(width * height);
        for (pair, uv) in chroma.chunks_exact_mut(width).enumerate() {
            self.sums.fill([0; 3]);
            for y in [pair * 2, pair * 2 + 1] {
                // The one read of this row of the mapping.
                self.row
                    .copy_from_slice(&pixels[y * pitch..y * pitch + row_bytes]);
                let luma = &mut luma[y * width..(y + 1) * width];
                for (x, (pixel, luma)) in self.row.as_chunks::<4>().0.iter().zip(luma).enumerate() {
                    let rgb = [pixel[2], pixel[1], pixel[0]].map(i32::from);
                    *luma = (16 + ((dot(Y, rgb) + (1 << 15)) >> 16)) as u8;
                    let sum = &mut self.sums[x / 2];
                    for (total, value) in sum.iter_mut().zip(rgb) {
                        *total += value;
                    }
                }
            }
            for (sum, uv) in self.sums.iter().zip(uv.as_chunks_mut::<2>().0) {
                // Four pixels per block: divide by 4 × 65536 with rounding.
                uv[0] = (128 + ((dot(CB, *sum) + (1 << 17)) >> 18)) as u8;
                uv[1] = (128 + ((dot(CR, *sum) + (1 << 17)) >> 18)) as u8;
            }
        }
        Ok(())
    }
}

fn dot(coefficients: [i32; 3], rgb: [i32; 3]) -> i32 {
    coefficients[0] * rgb[0] + coefficients[1] * rgb[1] + coefficients[2] * rgb[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 2×2 plane of a single premultiplied BGRA pixel; returns Y, Cb, Cr.
    fn solid(bgra: [u8; 4]) -> [u8; 3] {
        let pixels = bgra.repeat(4);
        let mut output = [0; 6];
        Nv12Converter::default()
            .convert(&pixels, 8, 2, 2, &mut output)
            .unwrap();
        assert!(output[..4].iter().all(|&y| y == output[0]));
        [output[0], output[4], output[5]]
    }

    #[test]
    fn known_colours_in_limited_range_bt709() {
        // Reference: BT.709 limited range, rounded.
        assert_eq!(solid([0, 0, 0, 255]), [16, 128, 128], "black");
        assert_eq!(solid([255, 255, 255, 255]), [235, 128, 128], "white");
        assert_eq!(solid([0, 0, 255, 255]), [63, 102, 240], "red");
        assert_eq!(solid([0, 255, 0, 255]), [173, 42, 26], "green");
        assert_eq!(solid([255, 0, 0, 255]), [32, 240, 118], "blue");
        assert_eq!(solid([128, 128, 128, 255]), [126, 128, 128], "grey");
    }

    #[test]
    fn premultiplied_alpha_is_composited_over_black() {
        assert_eq!(solid([0, 0, 0, 0]), [16, 128, 128], "transparent is black");
        // 50 % white, premultiplied, equals opaque mid grey over black.
        assert_eq!(solid([128, 128, 128, 128]), solid([128, 128, 128, 255]));
        // 50 % red.
        assert_eq!(solid([0, 0, 128, 128]), solid([0, 0, 128, 255]));
    }

    #[test]
    fn rows_follow_the_pitch_and_chroma_averages_each_block() {
        // 4×2 with a 4-byte pad per row: left block white, right block black
        // on top and red below.
        let white = [255, 255, 255, 255];
        let black = [0, 0, 0, 255];
        let red = [0, 0, 255, 255];
        let pad = [9; 4];
        let pixels = [
            white, white, black, black, pad, //
            white, white, red, red, pad,
        ]
        .concat();
        let mut output = [0; 12];
        let mut converter = Nv12Converter::default();
        converter.convert(&pixels, 20, 4, 2, &mut output).unwrap();
        assert_eq!(&output[..8], &[235, 235, 16, 16, 235, 235, 63, 63]);
        assert_eq!(&output[8..10], &[128, 128], "white block");
        // Half red: Cb, Cr halfway between neutral and red's.
        assert_eq!(&output[10..], &[115, 184], "black over red block");

        // Reused scratch carries nothing over.
        converter.convert(&pixels, 20, 4, 2, &mut output).unwrap();
        assert_eq!(&output[10..], &[115, 184]);
    }

    #[test]
    fn bad_geometry_is_an_error_not_a_panic() {
        let mut converter = Nv12Converter::default();
        let mut output = [0; 6];
        assert!(converter.convert(&[0; 15], 8, 2, 2, &mut output).is_err());
        assert!(converter.convert(&[0; 16], 4, 2, 2, &mut output).is_err());
        assert!(converter.convert(&[0; 12], 4, 1, 3, &mut output).is_err());
        assert!(converter.convert(&[0; 16], 8, 2, 2, &mut [0; 5]).is_err());
    }
}
