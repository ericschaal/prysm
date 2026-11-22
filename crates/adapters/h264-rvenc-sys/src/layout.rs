//! NV12 layout of the DMA input buffers; safe and testable on any host.
use crate::Error;
use std::ops::RangeInclusive;

/// `rvenc` input pitch alignment, in bytes.
pub const PITCH: usize = 192;
/// Widest picture, and widest DMA pitch, the encoder takes.
pub const MAX_WIDTH: u32 = 1920;
/// Tallest picture the encoder takes.
pub const MAX_HEIGHT: u32 = 1080;
/// Frame rates the encoder takes: `rvenc` programs 30 fps for any rate above
/// 25 (research `webos-capture/2026-11-02-encoder-latency`, F-001).
pub const FPS: RangeInclusive<u32> = 1..=30;
/// Target bitrates the encoder takes, in bits per second.
pub const BITRATE: RangeInclusive<u32> = 64_000..=100_000_000;
/// Largest access unit accepted from the encoder.
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// The DMA buffer pitch for `width`: the next multiple of [`PITCH`].
#[must_use]
pub fn pitch(width: u32) -> usize {
    (width as usize).div_ceil(PITCH) * PITCH
}

/// Whether an even `width`x`height` picture, at most
/// [`MAX_WIDTH`]x[`MAX_HEIGHT`], fits rows of `stride` bytes, at most
/// [`MAX_WIDTH`].
pub(crate) fn fits(width: usize, height: usize, stride: usize) -> bool {
    let (max_width, max_height) = (MAX_WIDTH as usize, MAX_HEIGHT as usize);
    (2..=max_width).contains(&width)
        && (2..=max_height).contains(&height)
        && width.is_multiple_of(2)
        && height.is_multiple_of(2)
        && (width..=max_width).contains(&stride)
}

/// Copy tightly packed NV12 (`width` bytes per row) into a buffer with
/// `stride` bytes per row, leaving the padding untouched.
///
/// # Errors
/// [`Error::Config`] for odd, empty or oversized dimensions, or lengths that
/// do not match; `output` is unchanged then.
pub fn copy_nv12(
    width: usize,
    height: usize,
    stride: usize,
    input: &[u8],
    output: &mut [u8],
) -> Result<(), Error> {
    if !(fits(width, height, stride)
        && input.len() == width * height * 3 / 2
        && output.len() == stride * height * 3 / 2)
    {
        return Err(Error::Config("invalid NV12 copy layout"));
    }
    for (source, destination) in input
        .chunks_exact(width)
        .zip(output.chunks_exact_mut(stride))
    {
        destination[..width].copy_from_slice(source);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_rounds_up_to_192() {
        assert_eq!(pitch(320), 384);
        assert_eq!(pitch(640), 768);
        assert_eq!(pitch(192), 192);
    }

    #[test]
    fn the_largest_picture_fits_and_no_more() {
        assert!(fits(1920, 1080, 1920) && fits(2, 2, 2));
        for (width, height, stride) in [(1920, 1082, 1920), (1922, 1080, 1922), (1920, 1080, 1921)]
        {
            assert!(!fits(width, height, stride));
        }
    }

    #[test]
    fn direct_nv12_upload_rewrites_active_rows_and_preserves_padding() {
        for stride in [64, 192] {
            let mut destination = vec![0xa5; stride * 34 * 3 / 2];
            for value in [16, 235] {
                let input = vec![value; 64 * 34 * 3 / 2];
                copy_nv12(64, 34, stride, &input, &mut destination).unwrap();
                for row in destination.chunks_exact(stride) {
                    assert_eq!(&row[..64], &[value; 64]);
                    assert!(row[64..].iter().all(|v| *v == 0xa5));
                }
            }
            let unchanged = destination.clone();
            for (width, height, pitch, length) in [
                (0, 34, stride, 0),
                (63, 34, stride, 63 * 34 * 3 / 2),
                (64, 33, stride, 64 * 33 * 3 / 2),
                (64, 34, 63, 64 * 34 * 3 / 2),
                (64, 34, stride, 64 * 34 * 3 / 2 - 1),
            ] {
                assert!(
                    copy_nv12(width, height, pitch, &vec![0; length], &mut destination).is_err()
                );
                assert_eq!(destination, unchanged);
            }
        }
    }

    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    #[test]
    fn the_pipeline_is_unavailable_off_the_tv() {
        assert!(matches!(
            crate::Pipeline::new(320, 180, 384, 30, 2_000_000),
            Err(Error::Unavailable)
        ));
    }
}
