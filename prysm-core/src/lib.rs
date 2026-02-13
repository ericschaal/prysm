mod color;
mod config;
mod spectrum;

pub use color::Color;
pub use config::{Config, SampleDensity};
pub use spectrum::{ColorSpectrum, EdgeSpectra};

/// Edge represents one of the four edges of the screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Top,    // Left to right
    Right,  // Top to bottom
    Bottom, // Right to left
    Left,   // Bottom to top
}
