//! Nearest-pixel sampling of a captured frame into RGB, for the surfaces' previews.
use prysm_core::nv12::Nv12View;
use prysm_core::{Frame, PixelFormat, Viewport};
use std::fmt;

/// Why a frame cannot be sampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleError {
    /// The frame has no pixels.
    Empty,
    /// The frame is compressed (MJPEG).
    Compressed,
}

impl From<SampleError> for &'static str {
    fn from(error: SampleError) -> Self {
        match error {
            SampleError::Empty => "the frame is empty",
            SampleError::Compressed => "MJPEG frames cannot be previewed",
        }
    }
}

impl fmt::Display for SampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str((*self).into())
    }
}

impl std::error::Error for SampleError {}

/// Sample `area` of `frame` into `width` x `height` RGB pixels, row-major, by
/// nearest pixel. Reads only the sampled pixels; never converts the whole frame.
/// A zero `width` or `height` gives no pixels; pixels `area` places outside the
/// frame are black.
///
/// # Errors
/// [`SampleError`]: the frame is empty, or compressed.
pub fn sample_rgb(
    frame: &Frame,
    area: Viewport,
    width: u32,
    height: u32,
) -> Result<Vec<[u8; 3]>, SampleError> {
    if frame.width() == 0 || frame.height() == 0 {
        return Err(SampleError::Empty);
    }
    if frame.format() == PixelFormat::MJPEG {
        return Err(SampleError::Compressed);
    }
    let nv12 = Nv12View::new(frame);
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        let sy = area.y + scale(y, area.height, height);
        for x in 0..width {
            let sx = area.x + scale(x, area.width, width);
            pixels
                .push(nv12.map_or_else(|| pixel(frame, sx, sy), |nv12| nv12.pixel(sx, sy).into()));
        }
    }
    Ok(pixels)
}

/// Target coordinate `i` of `of` mapped onto `span` source pixels: below `span`.
fn scale(i: u32, span: u32, of: u32) -> u32 {
    // `i < of`, so the quotient is below `span` and fits.
    u32::try_from(u64::from(i) * u64::from(span) / u64::from(of)).unwrap_or(u32::MAX)
}

fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 3] {
    let data = frame.as_slice();
    match frame.format() {
        PixelFormat::RGB24 | PixelFormat::BGR24 => {
            let offset = (y as usize * frame.width() as usize + x as usize) * 3;
            let Some(&[a, b, c]) = data.get(offset..offset + 3) else {
                return [0; 3];
            };
            if frame.format() == PixelFormat::RGB24 {
                [a, b, c]
            } else {
                [c, b, a]
            }
        }
        PixelFormat::YUYV { range, matrix } => {
            let (r, g, b) =
                prysm_core::yuyv::yuyv_pixel_to_rgb(data, x, y, frame.width(), range, matrix);
            [r, g, b]
        }
        // Sampled through `Nv12View` by the caller.
        PixelFormat::NV12 { .. } | PixelFormat::MJPEG => [0; 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{YuvRange, YuvStandardMatrix};

    /// A `width` x `height` RGB24 frame whose pixel (x, y) is `[x, y, 7]`.
    fn coordinates(width: u32, height: u32) -> Frame {
        let data = (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| [x as u8, y as u8, 7]))
            .collect();
        Frame::new(data, width, height, PixelFormat::RGB24).unwrap()
    }

    fn full(frame: &Frame) -> Viewport {
        Viewport::full_frame(frame.width(), frame.height())
    }

    #[test]
    fn rgb_and_bgr_come_out_in_rgb_order() {
        for (format, data) in [
            (PixelFormat::RGB24, vec![1, 2, 3, 40, 50, 60]),
            (PixelFormat::BGR24, vec![3, 2, 1, 60, 50, 40]),
        ] {
            let frame = Frame::new(data, 2, 1, format).unwrap();
            assert_eq!(
                sample_rgb(&frame, full(&frame), 2, 1).unwrap(),
                [[1, 2, 3], [40, 50, 60]],
                "{format:?}"
            );
        }
    }

    #[test]
    fn yuyv_matches_the_full_conversion_at_odd_offsets() {
        for range in [YuvRange::Full, YuvRange::Limited] {
            for matrix in [YuvStandardMatrix::Bt601, YuvStandardMatrix::Bt709] {
                let format = PixelFormat::YUYV { range, matrix };
                let frame =
                    Frame::new(vec![32, 90, 180, 210, 210, 180, 70, 100], 4, 1, format).unwrap();
                let sampled = sample_rgb(&frame, full(&frame), 4, 1).unwrap();
                let converted = prysm_core::yuyv::yuyv_to_rgb(&frame).unwrap();
                for (actual, expected) in sampled.iter().flatten().zip(&converted) {
                    assert!(actual.abs_diff(*expected) <= 1, "{format:?}");
                }
            }
        }
    }

    #[test]
    fn nv12_matches_the_full_conversion_at_odd_offsets() {
        for range in [YuvRange::Full, YuvRange::Limited] {
            for matrix in [YuvStandardMatrix::Bt601, YuvStandardMatrix::Bt709] {
                let format = PixelFormat::NV12 { range, matrix };
                // 6x4 with legal codes: a distinct luma per pixel, chroma per 2x2 block.
                let data: Vec<u8> = (0..24)
                    .map(|i| 20 + i * 9)
                    .chain([60, 200, 180, 90, 128, 128, 30, 220, 200, 40, 128, 70])
                    .collect();
                let frame = Frame::new(data, 6, 4, format).unwrap();
                let converted = prysm_core::nv12::nv12_to_rgb(&frame).unwrap();
                // Odd x and y, and a pair straddling two chroma blocks.
                let area = Viewport {
                    x: 1,
                    y: 1,
                    width: 5,
                    height: 3,
                };
                let sampled = sample_rgb(&frame, area, 5, 3).unwrap();
                for (i, actual) in sampled.iter().enumerate() {
                    let (x, y) = (1 + i % 5, 1 + i / 5);
                    let at = (y * 6 + x) * 3;
                    for (a, e) in actual.iter().zip(&converted[at..at + 3]) {
                        assert!(a.abs_diff(*e) <= 1, "{format:?} ({x}, {y})");
                    }
                }
            }
        }
        let limited = PixelFormat::NV12 {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        };
        let frame = Frame::new(vec![16, 235, 235, 16, 128, 128], 2, 2, limited).unwrap();
        assert_eq!(
            sample_rgb(&frame, full(&frame), 2, 2).unwrap(),
            [[0; 3], [255; 3], [255; 3], [0; 3]]
        );
        // Outside the frame is black, as for the other formats.
        let outside = Viewport {
            x: 2,
            y: 0,
            width: 1,
            height: 1,
        };
        assert_eq!(sample_rgb(&frame, outside, 1, 1).unwrap(), [[0; 3]]);
    }

    #[test]
    fn odd_sizes_take_the_nearest_lower_pixel() {
        let frame = coordinates(5, 3);
        assert_eq!(
            sample_rgb(&frame, full(&frame), 2, 2).unwrap(),
            [[0, 0, 7], [2, 0, 7], [0, 1, 7], [2, 1, 7]]
        );
        // Enlarging repeats pixels.
        let frame = coordinates(1, 1);
        assert_eq!(
            sample_rgb(&frame, full(&frame), 3, 1).unwrap(),
            [[0, 0, 7]; 3]
        );
    }

    #[test]
    fn samples_only_the_area() {
        let frame = coordinates(7, 5);
        let area = Viewport {
            x: 3,
            y: 1,
            width: 3,
            height: 3,
        };
        assert_eq!(
            sample_rgb(&frame, area, 3, 1).unwrap(),
            [[3, 1, 7], [4, 1, 7], [5, 1, 7]]
        );
    }

    #[test]
    fn empty_targets_give_no_pixels_and_outside_pixels_are_black() {
        let frame = coordinates(3, 3);
        assert!(sample_rgb(&frame, full(&frame), 0, 2).unwrap().is_empty());
        let outside = Viewport {
            x: 0,
            y: 3,
            width: 3,
            height: 1,
        };
        assert_eq!(sample_rgb(&frame, outside, 1, 1).unwrap(), [[0; 3]]);
    }

    #[test]
    fn empty_and_compressed_frames_are_errors() {
        let empty = Frame::dummy(0, 2).unwrap();
        assert_eq!(
            sample_rgb(&empty, full(&empty), 1, 1),
            Err(SampleError::Empty)
        );
        let jpeg = Frame::new(vec![1, 2, 3], 4, 2, PixelFormat::MJPEG).unwrap();
        assert_eq!(
            sample_rgb(&jpeg, full(&jpeg), 1, 1),
            Err(SampleError::Compressed)
        );
    }
}
