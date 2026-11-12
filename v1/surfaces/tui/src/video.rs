//! The captured frame as Unicode half blocks: two square-ish pixels per cell.
use prysm_core::{Frame, Viewport};
use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

/// A small RGB image sampled from a frame for one display area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 3]>,
}

impl Image {
    fn get(&self, x: u32, y: u32) -> [u8; 3] {
        self.pixels[(y * self.width + x) as usize]
    }
}

/// Sample `frame` to `width` x `height` pixels, center-cropping it to fill the
/// target without stretching. Reads only the sampled pixels; never converts the
/// whole frame.
pub(crate) fn sample(frame: &Frame, width: u32, height: u32) -> Result<Image, &'static str> {
    if width == 0 || height == 0 {
        return Err("preview area is empty");
    }
    let crop = cover(
        Viewport::full_frame(frame.width(), frame.height()),
        (width, height),
    );
    let pixels = prysm_runtime::sample_rgb(frame, crop, width, height)?;
    Ok(Image {
        width,
        height,
        pixels,
    })
}

/// The largest centered part of `crop` with the aspect ratio of `target`.
fn cover(mut crop: Viewport, (width, height): (u32, u32)) -> Viewport {
    let wide =
        u64::from(crop.width) * u64::from(height) > u64::from(crop.height) * u64::from(width);
    if wide {
        let cropped = scale_to(crop.height, width, height).min(crop.width);
        crop.x += (crop.width - cropped) / 2;
        crop.width = cropped;
    } else {
        let cropped = scale_to(crop.width, height, width).min(crop.height);
        crop.y += (crop.height - cropped) / 2;
        crop.height = cropped;
    }
    crop
}

fn scale_to(value: u32, numerator: u32, denominator: u32) -> u32 {
    let scaled = u64::from(value) * u64::from(numerator) / u64::from(denominator);
    u32::try_from(scaled).unwrap_or(u32::MAX).max(1)
}

/// Renders an image sampled at the area's width and twice its height.
pub(crate) struct HalfBlocks<'a>(pub &'a Image);

impl Widget for HalfBlocks<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let image = self.0;
        let columns = u32::from(area.width).min(image.width);
        let rows = u32::from(area.height).min(image.height / 2);
        for row in 0..rows {
            for column in 0..columns {
                let [tr, tg, tb] = image.get(column, row * 2);
                let [br, bg, bb] = image.get(column, row * 2 + 1);
                // Both bounded by the area's u16 extent.
                let (Ok(dx), Ok(dy)) = (u16::try_from(column), u16::try_from(row)) else {
                    continue;
                };
                buffer[(area.x + dx, area.y + dy)]
                    .set_symbol("▀")
                    .set_fg(Color::Rgb(tr, tg, tb))
                    .set_bg(Color::Rgb(br, bg, bb));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{PixelFormat, YuvRange, YuvStandardMatrix};

    fn rgb(pixels: &[[u8; 3]]) -> Vec<u8> {
        pixels.iter().flatten().copied().collect()
    }

    #[test]
    fn fills_the_target_by_cropping_the_center_without_stretching() {
        let row: Vec<u8> = [10, 20, 30, 40]
            .into_iter()
            .flat_map(|r| [r, 0, 0])
            .collect();
        let frame = Frame::new(row.repeat(2), 4, 2, PixelFormat::RGB24).unwrap();
        let image = sample(&frame, 2, 2).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.get(0, 0)[0], 20);
        assert_eq!(image.get(1, 1)[0], 30);
    }

    #[test]
    fn samples_rgb_and_bgr_in_source_order() {
        for (format, pixels) in [
            (PixelFormat::RGB24, vec![1, 2, 3, 40, 50, 60]),
            (PixelFormat::BGR24, vec![3, 2, 1, 60, 50, 40]),
        ] {
            let frame = Frame::new(pixels, 2, 1, format).unwrap();
            let image = sample(&frame, 2, 1).unwrap();
            assert_eq!(rgb(&image.pixels), [1, 2, 3, 40, 50, 60]);
        }
    }

    #[test]
    fn yuyv_sampling_honors_odd_offsets_range_and_matrix() {
        for range in [YuvRange::Full, YuvRange::Limited] {
            for matrix in [YuvStandardMatrix::Bt601, YuvStandardMatrix::Bt709] {
                let frame = Frame::new(
                    vec![32, 90, 180, 210, 210, 180, 70, 100],
                    4,
                    1,
                    PixelFormat::YUYV { range, matrix },
                )
                .unwrap();
                let sampled = sample(&frame, 4, 1).unwrap();
                let full = prysm_core::yuyv::yuyv_to_rgb(&frame).unwrap();
                for (actual, expected) in rgb(&sampled.pixels).iter().zip(&full) {
                    assert!(actual.abs_diff(*expected) <= 1);
                }
            }
        }
    }

    #[test]
    fn cover_preserves_source_aspect() {
        assert_eq!(
            cover(Viewport::full_frame(1920, 1080), (100, 100)),
            Viewport {
                x: 420,
                y: 0,
                width: 1080,
                height: 1080
            }
        );
        assert_eq!(
            cover(Viewport::full_frame(1080, 1920), (100, 100)),
            Viewport {
                x: 0,
                y: 420,
                width: 1080,
                height: 1080
            }
        );
        assert_eq!(
            cover(Viewport::full_frame(1, 1920), (100, 100)),
            Viewport {
                x: 0,
                y: 959,
                width: 1,
                height: 1
            }
        );
        let large = sample(&Frame::dummy(1920, 1080).unwrap(), 300, 120).unwrap();
        assert_eq!(large.pixels.len(), 300 * 120);
    }

    #[test]
    fn empty_or_unsupported_frames_are_preview_errors() {
        let frame = Frame::dummy(4, 2).unwrap();
        assert!(sample(&frame, 0, 2).is_err());
        assert!(sample(&Frame::dummy(0, 2).unwrap(), 4, 2).is_err());
        let jpeg = Frame::new(vec![1, 2, 3], 4, 2, PixelFormat::MJPEG).unwrap();
        assert!(sample(&jpeg, 4, 2).is_err());
    }

    #[test]
    fn half_blocks_pair_vertical_pixels_per_cell() {
        let image = Image {
            width: 1,
            height: 2,
            pixels: vec![[255, 0, 0], [0, 0, 255]],
        };
        let area = Rect::new(0, 0, 3, 3);
        let mut buffer = Buffer::empty(area);
        HalfBlocks(&image).render(area, &mut buffer);
        let cell = &buffer[(0, 0)];
        assert_eq!(cell.symbol(), "▀");
        assert_eq!(cell.fg, Color::Rgb(255, 0, 0));
        assert_eq!(cell.bg, Color::Rgb(0, 0, 255));
        assert_eq!(buffer[(1, 0)].symbol(), " ");
        assert_eq!(buffer[(0, 1)].symbol(), " ");
    }
}
