//! Nearest-pixel sampling of a captured frame into RGB, for the surfaces' previews.
use prysm_core::{Frame, PixelFormat, Viewport};

/// Sample `area` of `frame` into `width` x `height` RGB pixels, row-major, by
/// nearest pixel. Reads only the sampled pixels; never converts the whole frame.
/// A zero `width` or `height` gives no pixels; pixels `area` places outside the
/// frame are black.
///
/// # Errors
/// The frame is empty, or compressed (MJPEG).
pub fn sample_rgb(
    frame: &Frame,
    area: Viewport,
    width: u32,
    height: u32,
) -> Result<Vec<[u8; 3]>, &'static str> {
    if frame.width() == 0 || frame.height() == 0 {
        return Err("the frame is empty");
    }
    if frame.format() == PixelFormat::MJPEG {
        return Err("MJPEG frames cannot be previewed");
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        let sy = area.y + scale(y, area.height, height);
        for x in 0..width {
            let sx = area.x + scale(x, area.width, width);
            pixels.push(pixel(frame, sx, sy));
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
        PixelFormat::MJPEG => [0; 3],
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
            Err("the frame is empty")
        );
        let jpeg = Frame::new(vec![1, 2, 3], 4, 2, PixelFormat::MJPEG).unwrap();
        assert_eq!(
            sample_rgb(&jpeg, full(&jpeg), 1, 1),
            Err("MJPEG frames cannot be previewed")
        );
    }
}
