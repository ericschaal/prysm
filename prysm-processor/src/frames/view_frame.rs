use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Color, LinearColor};

/// Viewport within a frame
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    pub fn full_frame(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }
}

/// Brightness (luma) of a pixel read directly from raw frame data.
///
/// YUYV stores full-range luma at every even byte, so band/change detection
/// never needs a color conversion. Out-of-bounds coordinates read as black.
pub fn luma_at(frame: &Frame, x: u32, y: u32) -> u8 {
    if x >= frame.width || y >= frame.height {
        return 0;
    }
    match frame.format {
        PixelFormat::YUYV => {
            let idx = ((y * frame.width + x) * 2) as usize;
            frame.data.get(idx).copied().unwrap_or(0)
        }
        PixelFormat::RGB24 | PixelFormat::BGR24 => {
            let idx = ((y * frame.width + x) * 3) as usize;
            match frame.data.get(idx..idx + 3) {
                Some(px) => ((px[0] as u16 + px[1] as u16 + px[2] as u16) / 3) as u8,
                None => 0,
            }
        }
        PixelFormat::MJPEG => 0,
    }
}

/// A raw frame plus the viewport that downstream nodes should read from.
///
/// Pixels stay in the capture format; consumers decode only what they touch.
#[derive(Debug, Clone)]
pub struct ViewFrame {
    pub frame: Frame,
    pub viewport: Viewport,
}

impl ViewFrame {
    pub fn new(frame: Frame) -> Self {
        let viewport = Viewport::full_frame(frame.width, frame.height);
        Self { frame, viewport }
    }

    pub fn viewport_width(&self) -> u32 {
        self.viewport.width
    }

    pub fn viewport_height(&self) -> u32 {
        self.viewport.height
    }

    /// Average a viewport-relative region in linear RGB space, integrating
    /// every pixel in the region. Decodes only the pixels it visits.
    pub fn average_linear(
        &self,
        x_start: u32,
        y_start: u32,
        x_end: u32,
        y_end: u32,
    ) -> LinearColor {
        let mut sum = LinearColor::black();
        let mut count: u32 = 0;

        for y in y_start..y_end {
            let abs_y = self.viewport.y + y;
            if abs_y >= self.frame.height {
                break;
            }
            for x in x_start..x_end {
                let abs_x = self.viewport.x + x;
                if abs_x >= self.frame.width {
                    break;
                }
                if let Some(color) = self.pixel_srgb(abs_x, abs_y) {
                    sum += LinearColor::from_srgb(color);
                    count += 1;
                }
            }
        }

        if count == 0 {
            return LinearColor::black();
        }
        sum * (1.0 / count as f32)
    }

    /// Decode a single pixel (absolute coordinates) to sRGB.
    fn pixel_srgb(&self, x: u32, y: u32) -> Option<Color> {
        match self.frame.format {
            PixelFormat::YUYV => {
                let (r, g, b) = prysm_capture::yuyv::yuyv_pixel_to_rgb(
                    &self.frame.data,
                    x,
                    y,
                    self.frame.width,
                );
                Some(Color::new(r, g, b))
            }
            PixelFormat::RGB24 => {
                let idx = ((y * self.frame.width + x) * 3) as usize;
                let px = self.frame.data.get(idx..idx + 3)?;
                Some(Color::new(px[0], px[1], px[2]))
            }
            PixelFormat::BGR24 | PixelFormat::MJPEG => None,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Build a YUYV frame with neutral chroma from a per-pixel luma function.
    pub fn yuyv_frame_from_luma(width: u32, height: u32, luma: impl Fn(u32, u32) -> u8) -> Frame {
        let mut data = Vec::with_capacity((width * height * 2) as usize);
        for y in 0..height {
            for pair_x in (0..width).step_by(2) {
                data.push(luma(pair_x, y)); // Y0
                data.push(128); // U
                data.push(luma((pair_x + 1).min(width - 1), y)); // Y1
                data.push(128); // V
            }
        }
        Frame::new(data, width, height, PixelFormat::YUYV)
    }

    #[test]
    fn luma_reads_yuyv_directly() {
        let frame = yuyv_frame_from_luma(4, 2, |x, y| (x + y * 10) as u8 * 10);
        assert_eq!(luma_at(&frame, 0, 0), 0);
        assert_eq!(luma_at(&frame, 1, 0), 10);
        assert_eq!(luma_at(&frame, 3, 1), 130);
        // Out of bounds reads as black
        assert_eq!(luma_at(&frame, 4, 0), 0);
        assert_eq!(luma_at(&frame, 0, 2), 0);
    }

    #[test]
    fn average_linear_uniform_region() {
        let frame = yuyv_frame_from_luma(8, 8, |_, _| 128);
        let view = ViewFrame::new(frame);
        let avg = view.average_linear(0, 0, 8, 8);
        let expected = LinearColor::from_srgb(Color::new(128, 128, 128));
        assert!((avg.r - expected.r).abs() < 0.01, "r = {}", avg.r);
        assert!((avg.g - expected.g).abs() < 0.01);
        assert!((avg.b - expected.b).abs() < 0.01);
    }

    #[test]
    fn average_linear_respects_viewport_offset() {
        // Left half black, right half white; viewport covers only the right half
        let frame = yuyv_frame_from_luma(8, 4, |x, _| if x < 4 { 0 } else { 255 });
        let mut view = ViewFrame::new(frame);
        view.viewport = Viewport {
            x: 4,
            y: 0,
            width: 4,
            height: 4,
        };
        let avg = view.average_linear(0, 0, 4, 4);
        assert!(avg.r > 0.99, "expected white, got {}", avg.r);
    }
}
