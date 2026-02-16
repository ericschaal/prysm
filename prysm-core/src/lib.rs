mod color;
mod config;
mod linear;
mod spectrum;

pub use color::Color;
pub use config::{Config, SampleDensity};
pub use linear::LinearColor;
pub use spectrum::{EdgeSpectra, Spectrum};

/// Edge represents one of the four edges of the screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Top,    // Left to right
    Right,  // Top to bottom
    Bottom, // Right to left
    Left,   // Bottom to top
}
