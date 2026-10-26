mod color;
mod color_strip;
mod config;
mod linear;

pub use color::Color;
pub use color_strip::{ColorStrip, EdgeColors, SampleDensity};
pub use config::{
    BrightnessPercent, CaptureResolution, Config, ShadowLiftPercent, SmoothingSeconds,
    SpatialReachPercent,
};
pub use linear::LinearColor;

/// Edge represents one of the four edges of the screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Top,    // Left to right
    Right,  // Top to bottom
    Bottom, // Right to left
    Left,   // Bottom to top
}

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
