use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Color, Edge, LinearColor};

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
/// YUYV stores luma at every even byte, so black-bar detection
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

    /// Average a viewport-relative edge region in linear RGB, with quadratic
    /// falloff toward the interior. Decodes every pixel in the region.
    pub fn average_edge_linear(
        &self,
        x_start: u32,
        y_start: u32,
        x_end: u32,
        y_end: u32,
        edge: Edge,
    ) -> LinearColor {
        let mut sum = LinearColor::black();
        let mut total_weight = 0.0;
        let depth = match edge {
            Edge::Top | Edge::Bottom => y_end - y_start,
            Edge::Left | Edge::Right => x_end - x_start,
        };
        let inverse_depth = 1.0 / depth.max(1) as f32;

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
                    let distance = match edge {
                        Edge::Top => y - y_start,
                        Edge::Bottom => y_end - 1 - y,
                        Edge::Left => x - x_start,
                        Edge::Right => x_end - 1 - x,
                    };
                    // Pixel centers keep even a one-pixel-deep region weighted.
                    let falloff = 1.0 - (distance as f32 + 0.5) * inverse_depth;
                    let weight = falloff * falloff;
                    sum += LinearColor::from_srgb(color) * weight;
                    // Include black pixels so small highlights do not fill the whole sample.
                    total_weight += weight;
                }
            }
        }

        if total_weight == 0.0 {
            return LinearColor::black();
        }
        sum * (1.0 / total_weight)
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
                    self.frame.yuv_range,
                    self.frame.yuv_matrix,
                );
                Some(Color::new(r, g, b))
            }
            PixelFormat::RGB24 | PixelFormat::BGR24 => {
                let idx = ((y * self.frame.width + x) * 3) as usize;
                let px = self.frame.data.get(idx..idx + 3)?;
                Some(if self.frame.format == PixelFormat::BGR24 {
                    Color::new(px[2], px[1], px[0])
                } else {
                    Color::new(px[0], px[1], px[2])
                })
            }
            PixelFormat::MJPEG => None,
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
    fn average_edge_linear_uniform_region() {
        let frame = yuyv_frame_from_luma(8, 8, |_, _| 128);
        let view = ViewFrame::new(frame);
        let avg = view.average_edge_linear(0, 0, 8, 8, Edge::Top);
        let expected = LinearColor::from_srgb(Color::new(128, 128, 128));
        assert!((avg.r - expected.r).abs() < 0.01, "r = {}", avg.r);
        assert!((avg.g - expected.g).abs() < 0.01);
        assert!((avg.b - expected.b).abs() < 0.01);
    }

    #[test]
    fn average_edge_linear_respects_viewport_offset_and_keeps_black_in_the_average() {
        let frame = yuyv_frame_from_luma(8, 8, |x, y| {
            if (2..6).contains(&x) && y == 2 {
                255
            } else {
                0
            }
        });
        let mut view = ViewFrame::new(frame);
        view.viewport = Viewport {
            x: 2,
            y: 2,
            width: 4,
            height: 4,
        };
        let avg = view.average_edge_linear(0, 0, 4, 2, Edge::Top);
        // The white outer row and black inner row have weights in a 9:1 ratio.
        assert!(
            (avg.r - 0.9).abs() < 1e-6,
            "expected 90% white, got {}",
            avg.r
        );
    }
}
