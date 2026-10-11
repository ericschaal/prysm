use crate::LinearColor;
use std::ops::{Add, Mul};
use std::sync::Arc;

/// Sample density representing samples per 1000 pixels of edge length
///
/// Example: `SampleDensity(50)` means 50 samples per 1000px, so a 1920px edge gets ~96 samples
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleDensity(pub usize);

impl SampleDensity {
    /// Calculate samples, capped at one per pixel (at least one for empty edges).
    #[must_use]
    pub fn samples_for_length(self, length_px: usize) -> usize {
        (((length_px as f32 / 1000.0) * self.0 as f32).max(1.0) as usize).min(length_px.max(1))
    }

    /// Get the raw density value (samples per 1000px)
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for SampleDensity {
    fn default() -> Self {
        // About 38 samples across a 640px edge, independent of the LED count.
        Self(60)
    }
}

/// Gradient of colors along an edge, in linear RGB space.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorStrip {
    samples: Arc<Vec<LinearColor>>,
}

impl ColorStrip {
    /// Create a new `ColorStrip` from a vector of linear color samples
    pub fn new(samples: Vec<LinearColor>) -> Self {
        assert!(
            !samples.is_empty(),
            "ColorStrip must have at least one sample"
        );
        Self {
            samples: Arc::new(samples),
        }
    }

    /// Create a `ColorStrip` filled with the specified color
    pub fn fill(color: LinearColor, count: usize) -> Self {
        Self::new(vec![color; count])
    }

    /// Create a `ColorStrip` with all black samples
    pub fn black(count: usize) -> Self {
        Self::fill(LinearColor::black(), count)
    }

    /// Get the number of samples in the strip
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if the strip is empty (should never be true)
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Sample the strip at a normalized position [0.0, 1.0] using linear interpolation
    pub fn sample_at(&self, position: f32) -> LinearColor {
        let position = position.clamp(0.0, 1.0);

        if self.samples.len() == 1 {
            return self.samples[0];
        }

        let float_index = position * (self.samples.len() - 1) as f32;
        let index = float_index.floor() as usize;
        let next_index = (index + 1).min(self.samples.len() - 1);
        let ratio = float_index - index as f32;

        self.samples[index].blend(&self.samples[next_index], ratio)
    }

    /// Get color at specific index in a count-sized output
    pub fn color_at(&self, index: usize, count: usize) -> LinearColor {
        assert!(index < count, "Index out of bounds");
        let position = if count == 1 {
            0.5
        } else {
            index as f32 / (count - 1) as f32
        };
        self.sample_at(position)
    }

    /// Resample the strip into exactly N linear colors
    pub fn resample(&self, count: usize) -> Vec<LinearColor> {
        (0..count).map(|i| self.color_at(i, count)).collect()
    }

    /// Blend two strips together with a ratio (0.0 = full self, 1.0 = full other)
    pub fn blend(&self, other: &ColorStrip, ratio: f32) -> ColorStrip {
        let result_len = self.samples.len().max(other.samples.len());
        let blended: Vec<LinearColor> = (0..result_len)
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
        ColorStrip::new(blended)
    }
}

impl Default for ColorStrip {
    fn default() -> Self {
        Self::black(1)
    }
}

impl Add for ColorStrip {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        let result_len = self.samples.len().max(other.samples.len());
        let summed: Vec<LinearColor> = (0..result_len)
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

impl Mul<f32> for ColorStrip {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        let scaled: Vec<LinearColor> = self.samples.iter().map(|&c| c * scalar).collect();
        Self::new(scaled)
    }
}

/// Color strips for all four screen edges, in linear RGB space.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeColors {
    pub top: ColorStrip,
    pub right: ColorStrip,
    pub bottom: ColorStrip,
    pub left: ColorStrip,
}

impl EdgeColors {
    /// Create new EdgeColors with the given strips
    pub fn new(top: ColorStrip, right: ColorStrip, bottom: ColorStrip, left: ColorStrip) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Create `EdgeColors` filled with the specified color
    #[must_use]
    pub fn fill(
        color: LinearColor,
        width: usize,
        height: usize,
        sample_density: SampleDensity,
    ) -> Self {
        let top_samples = sample_density.samples_for_length(width);
        let bottom_samples = top_samples;
        let left_samples = sample_density.samples_for_length(height);
        let right_samples = left_samples;

        Self {
            top: ColorStrip::fill(color, top_samples),
            right: ColorStrip::fill(color, right_samples),
            bottom: ColorStrip::fill(color, bottom_samples),
            left: ColorStrip::fill(color, left_samples),
        }
    }

    /// Create `EdgeColors` with all black colors
    #[must_use]
    pub fn black(width: usize, height: usize, sample_density: crate::SampleDensity) -> Self {
        Self::fill(LinearColor::black(), width, height, sample_density)
    }

    /// Blend two `EdgeColors` together with a ratio (0.0 = full self, 1.0 = full other)
    #[must_use]
    pub fn blend(&self, other: &EdgeColors, ratio: f32) -> EdgeColors {
        EdgeColors {
            top: self.top.blend(&other.top, ratio),
            right: self.right.blend(&other.right, ratio),
            bottom: self.bottom.blend(&other.bottom, ratio),
            left: self.left.blend(&other.left, ratio),
        }
    }
}

impl Default for EdgeColors {
    fn default() -> Self {
        Self::black(1920, 1080, SampleDensity::default())
    }
}

impl Add for EdgeColors {
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

impl Mul<f32> for EdgeColors {
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
