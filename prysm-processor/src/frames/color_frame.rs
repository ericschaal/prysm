use prysm_core::Color;
use std::sync::Arc;

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

    #[allow(dead_code)]
    pub fn is_full_frame(&self, frame_width: u32, frame_height: u32) -> bool {
        self.x == 0 && self.y == 0 && self.width == frame_width && self.height == frame_height
    }
}

/// Decoded color frame with viewport
#[derive(Debug, Clone)]
pub struct ColorFrame {
    /// Row-major color array (width * height)
    pub pixels: Arc<Vec<Color>>,
    /// Physical frame dimensions
    pub width: u32,
    pub height: u32,
    /// Visible viewport (defaults to full frame)
    pub viewport: Viewport,
}

impl ColorFrame {
    pub fn new(pixels: Vec<Color>, width: u32, height: u32) -> Self {
        Self {
            pixels: Arc::new(pixels),
            width,
            height,
            viewport: Viewport::full_frame(width, height),
        }
    }

    /// Get pixel at viewport-relative coordinates
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<Color> {
        let abs_x = self.viewport.x + x;
        let abs_y = self.viewport.y + y;

        if abs_x >= self.width || abs_y >= self.height {
            return None;
        }

        let idx = (abs_y * self.width + abs_x) as usize;
        self.pixels.get(idx).copied()
    }

    /// Get viewport width
    pub fn viewport_width(&self) -> u32 {
        self.viewport.width
    }

    /// Get viewport height
    pub fn viewport_height(&self) -> u32 {
        self.viewport.height
    }
}
