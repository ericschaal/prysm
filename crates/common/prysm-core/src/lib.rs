mod color;
mod color_strip;
mod config;
mod frame;
mod linear;
pub mod yuyv;

pub use frame::{Frame, FrameError, FramePool, FrameWriter, PixelFormat};
pub use yuv::{YuvRange, YuvStandardMatrix};

pub use color::Color;
pub use color_strip::{ColorStrip, EdgeColors, EmptyColorStrip, SampleDensity};
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

/// Source image and the crop/colors produced by one processing pass.
/// Cloning shares the pixel and color storage without copying their contents.
#[derive(Debug, Clone)]
pub struct ProcessedSnapshot {
    pub frame: Frame,
    pub viewport: Option<Viewport>,
    pub colors: EdgeColors,
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn observers_share_the_paired_result_and_source_pixels() {
        let frame = Frame::dummy(4, 2).unwrap();
        let pixels = frame.as_slice().as_ptr();
        let snapshot = Arc::new(ProcessedSnapshot {
            frame,
            viewport: Some(Viewport {
                x: 1,
                y: 0,
                width: 2,
                height: 2,
            }),
            colors: EdgeColors::default(),
        });
        let observer = Arc::clone(&snapshot);
        assert!(Arc::ptr_eq(&snapshot, &observer));
        assert_eq!(observer.frame.as_slice().as_ptr(), pixels);
        assert_eq!(observer.viewport.unwrap().width, 2);
    }
}
