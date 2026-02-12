#![no_main]

#[derive(Debug, Clone)]
pub struct Config {
    /// Number of LEDs per zone
    pub leds_per_zone: usize,

    /// Total number of LEDs
    pub total_leds: usize,

    /// Brightness (0.0 to 1.0)
    pub brightness: f32,

    /// Smoothing factor for color transitions (0.0 = no smoothing, 1.0 = maximum smoothing)
    pub smoothing: f32,

    /// Depth of the edge sampling (as fraction of screen, e.g., 0.1 = 10% from edge)
    pub edge_depth: f32,

    /// Target FPS
    pub target_fps: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            leds_per_zone: 20,
            total_leds: 160,
            brightness: 0.8,
            smoothing: 0.7,
            edge_depth: 0.15,
            target_fps: 30,
        }
    }
}

/// Zone represents a region of the screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Zone {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Zone {
    pub fn all() -> &'static [Zone] {
        &[
            Zone::TopLeft,
            Zone::Top,
            Zone::TopRight,
            Zone::Right,
            Zone::BottomRight,
            Zone::Bottom,
            Zone::BottomLeft,
            Zone::Left,
        ]
    }
}


/// RGB color representation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn black() -> Self {
        Self { r: 0, g: 0, b: 0 }
    }

    /// Blend two colors with a ratio (0.0 = full self, 1.0 = full other)
    pub fn blend(&self, other: &Color, ratio: f32) -> Color {
        let ratio = ratio.clamp(0.0, 1.0);
        Color {
            r: (self.r as f32 * (1.0 - ratio) + other.r as f32 * ratio) as u8,
            g: (self.g as f32 * (1.0 - ratio) + other.g as f32 * ratio) as u8,
            b: (self.b as f32 * (1.0 - ratio) + other.b as f32 * ratio) as u8,
        }
    }

    /// Apply brightness multiplier (0.0 to 1.0)
    pub fn with_brightness(&self, brightness: f32) -> Color {
        let brightness = brightness.clamp(0.0, 1.0);
        Color {
            r: (self.r as f32 * brightness) as u8,
            g: (self.g as f32 * brightness) as u8,
            b: (self.b as f32 * brightness) as u8,
        }
    }
}