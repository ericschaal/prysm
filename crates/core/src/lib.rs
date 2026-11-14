mod capture_resolution;
mod color;
mod color_strip;
mod config;
mod correction;
pub use correction::ColorCorrection;
mod frame;
mod linear;
pub mod nv12;
mod yuv_rgb;
pub mod yuyv;

pub use frame::{Frame, FrameError, FramePool, FramePoolStats, FrameWriter, PixelFormat};
pub use yuv::{YuvRange, YuvStandardMatrix};
pub use yuv_rgb::YuvToRgb;

pub use capture_resolution::CaptureResolution;
pub use color::Color;
pub use color_strip::{ColorStrip, EdgeColors, EmptyColorStrip};
pub use config::{
    BrightnessPercent, Config, SampleDensity, ShadowLiftPercent, SmoothingSeconds,
    SpatialReachPercent,
};
pub use linear::LinearColor;

/// Edge represents one of the four edges of the screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Top,    // Left to right
    Right,  // Top to bottom
    Bottom, // Left to right
    Left,   // Top to bottom
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
    #[must_use]
    pub fn full_frame(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }
}
