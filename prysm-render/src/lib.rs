#![no_main]

use std::collections::HashMap;
use futures::Stream;

pub trait PrysmRenderer {
    fn run(&mut self, input: impl Stream<Item = HashMap<Zone, Color>>);
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