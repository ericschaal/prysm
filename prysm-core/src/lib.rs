mod color;
mod color_strip;
mod config;
mod linear;

pub use color::Color;
pub use color_strip::{ColorStrip, EdgeColors, SampleDensity};
pub use config::Config;
pub use linear::LinearColor;

/// Edge represents one of the four edges of the screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Top,    // Left to right
    Right,  // Top to bottom
    Bottom, // Right to left
    Left,   // Bottom to top
}
