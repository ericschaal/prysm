#[derive(Debug, Clone)]
pub struct Config {
    /// Sample density per 1000 pixels of edge length
    /// Example: 40 samples per 1000px means a 1920px edge gets ~77 samples
    pub samples_per_1000px: usize,

    /// Brightness (0.0 to 1.0)
    pub brightness: f32,

    /// Smoothing factor for color transitions (0.0 = no smoothing, 1.0 = maximum smoothing)
    pub smoothing: f32,

    /// Depth of the edge sampling in pixels from the screen edge
    pub edge_depth_px: u32,

    /// Target FPS
    pub target_fps: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            samples_per_1000px: 50,
            brightness: 0.8,
            smoothing: 0.7,
            edge_depth_px: 100,
            target_fps: 30,
        }
    }
}

use std::fmt;
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign};
use std::sync::Arc;

/// Edge represents one of the four edges of the screen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Top,    // Left to right
    Right,  // Top to bottom
    Bottom, // Right to left
    Left,   // Bottom to top
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

// Trait implementations for Color

impl Default for Color {
    fn default() -> Self {
        Self::black()
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rgb({}, {}, {})", self.r, self.g, self.b)
    }
}

impl Add for Color {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            r: self.r.saturating_add(other.r),
            g: self.g.saturating_add(other.g),
            b: self.b.saturating_add(other.b),
        }
    }
}

impl AddAssign for Color {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Color {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            r: self.r.saturating_sub(other.r),
            g: self.g.saturating_sub(other.g),
            b: self.b.saturating_sub(other.b),
        }
    }
}

impl SubAssign for Color {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

impl Mul<f32> for Color {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        let scalar = scalar.clamp(0.0, 1.0);
        Self {
            r: (self.r as f32 * scalar) as u8,
            g: (self.g as f32 * scalar) as u8,
            b: (self.b as f32 * scalar) as u8,
        }
    }
}

impl MulAssign<f32> for Color {
    fn mul_assign(&mut self, scalar: f32) {
        *self = *self * scalar;
    }
}

impl Div<f32> for Color {
    type Output = Self;

    fn div(self, scalar: f32) -> Self {
        if scalar == 0.0 {
            return Self::black();
        }
        let scalar = 1.0 / scalar;
        Self {
            r: ((self.r as f32 * scalar).min(255.0)) as u8,
            g: ((self.g as f32 * scalar).min(255.0)) as u8,
            b: ((self.b as f32 * scalar).min(255.0)) as u8,
        }
    }
}

impl DivAssign<f32> for Color {
    fn div_assign(&mut self, scalar: f32) {
        *self = *self / scalar;
    }
}

/// ColorSpectrum represents a gradient of colors along an edge
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorSpectrum {
    samples: Arc<Vec<Color>>,
}

impl ColorSpectrum {
    /// Create a new ColorSpectrum from a vector of color samples
    pub fn new(samples: Vec<Color>) -> Self {
        assert!(
            !samples.is_empty(),
            "ColorSpectrum must have at least one sample"
        );
        Self {
            samples: Arc::new(samples),
        }
    }

    /// Create a ColorSpectrum with all black samples
    pub fn black(count: usize) -> Self {
        Self::new(vec![Color::black(); count])
    }

    /// Get the number of samples in the spectrum
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if the spectrum is empty (should never be true)
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Sample the spectrum at a normalized position [0.0, 1.0] using linear interpolation
    pub fn sample_at(&self, position: f32) -> Color {
        let position = position.clamp(0.0, 1.0);

        if self.samples.len() == 1 {
            return self.samples[0];
        }

        // Map position to sample indices
        let float_index = position * (self.samples.len() - 1) as f32;
        let index = float_index.floor() as usize;
        let next_index = (index + 1).min(self.samples.len() - 1);
        let ratio = float_index - index as f32;

        self.samples[index].blend(&self.samples[next_index], ratio)
    }

    /// Get color at specific index in a count-sized output
    /// Example: color_at(5, 20) = 6th color out of 20 total
    pub fn color_at(&self, index: usize, count: usize) -> Color {
        assert!(index < count, "Index out of bounds");
        let position = if count == 1 {
            0.5
        } else {
            index as f32 / (count - 1) as f32
        };
        self.sample_at(position)
    }

    /// Quantize the spectrum into exactly N colors
    pub fn quantize(&self, count: usize) -> Vec<Color> {
        (0..count).map(|i| self.color_at(i, count)).collect()
    }

    /// Blend two spectrums together with a ratio (0.0 = full self, 1.0 = full other)
    pub fn blend(&self, other: &ColorSpectrum, ratio: f32) -> ColorSpectrum {
        // Use the larger sample count for the result
        let result_len = self.samples.len().max(other.samples.len());
        let blended: Vec<Color> = (0..result_len)
            .map(|i| {
                let pos = if result_len == 1 {
                    0.5
                } else {
                    i as f32 / (result_len - 1) as f32
                };
                let color1 = self.sample_at(pos);
                let color2 = other.sample_at(pos);
                color1.blend(&color2, ratio)
            })
            .collect();
        ColorSpectrum::new(blended)
    }
}

// Trait implementations for ColorSpectrum

impl Default for ColorSpectrum {
    fn default() -> Self {
        Self::black(1)
    }
}

impl Add for ColorSpectrum {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        let result_len = self.samples.len().max(other.samples.len());
        let summed: Vec<Color> = (0..result_len)
            .map(|i| {
                let pos = if result_len == 1 {
                    0.5
                } else {
                    i as f32 / (result_len - 1) as f32
                };
                self.sample_at(pos) + other.sample_at(pos)
            })
            .collect();
        Self::new(summed)
    }
}

impl Mul<f32> for ColorSpectrum {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        let scaled: Vec<Color> = self.samples.iter().map(|&c| c * scalar).collect();
        Self::new(scaled)
    }
}

/// EdgeSpectrums contains color spectrums for all four edges
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeSpectrums {
    pub top: ColorSpectrum,
    pub right: ColorSpectrum,
    pub bottom: ColorSpectrum,
    pub left: ColorSpectrum,
}

impl EdgeSpectrums {
    /// Create new EdgeSpectrums with the given spectrums
    pub fn new(
        top: ColorSpectrum,
        right: ColorSpectrum,
        bottom: ColorSpectrum,
        left: ColorSpectrum,
    ) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Create EdgeSpectrums with all black colors
    /// Sample counts are based on aspect ratio (more samples for longer edges)
    pub fn black(width: usize, height: usize, samples_per_1000px: usize) -> Self {
        let top_samples = ((width as f32 / 1000.0) * samples_per_1000px as f32).max(1.0) as usize;
        let bottom_samples = top_samples;
        let left_samples = ((height as f32 / 1000.0) * samples_per_1000px as f32).max(1.0) as usize;
        let right_samples = left_samples;

        Self {
            top: ColorSpectrum::black(top_samples),
            right: ColorSpectrum::black(right_samples),
            bottom: ColorSpectrum::black(bottom_samples),
            left: ColorSpectrum::black(left_samples),
        }
    }

    /// Blend two EdgeSpectrums together with a ratio (0.0 = full self, 1.0 = full other)
    pub fn blend(&self, other: &EdgeSpectrums, ratio: f32) -> EdgeSpectrums {
        EdgeSpectrums {
            top: self.top.blend(&other.top, ratio),
            right: self.right.blend(&other.right, ratio),
            bottom: self.bottom.blend(&other.bottom, ratio),
            left: self.left.blend(&other.left, ratio),
        }
    }
}

// Trait implementations for EdgeSpectrums

impl Default for EdgeSpectrums {
    fn default() -> Self {
        Self::black(1920, 1080, 50)
    }
}

impl Add for EdgeSpectrums {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            top: self.top + other.top,
            right: self.right + other.right,
            bottom: self.bottom + other.bottom,
            left: self.left + other.left,
        }
    }
}

impl Mul<f32> for EdgeSpectrums {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        Self {
            top: self.top * scalar,
            right: self.right * scalar,
            bottom: self.bottom * scalar,
            left: self.left * scalar,
        }
    }
}
