use crate::Color;
use std::ops::{Add, Mul};
use std::sync::Arc;

/// `ColorSpectrum` represents a gradient of colors along an edge
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorSpectrum {
    samples: Arc<Vec<Color>>,
}

impl ColorSpectrum {
    /// Create a new `ColorSpectrum` from a vector of color samples
    pub fn new(samples: Vec<Color>) -> Self {
        assert!(
            !samples.is_empty(),
            "ColorSpectrum must have at least one sample"
        );
        Self {
            samples: Arc::new(samples),
        }
    }

    /// Create a `ColorSpectrum` filled with the specified color
    pub fn fill(color: Color, count: usize) -> Self {
        Self::new(vec![color; count])
    }

    /// Create a `ColorSpectrum` with all black samples
    pub fn black(count: usize) -> Self {
        Self::fill(Color::black(), count)
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

    /// Blend two spectra together with a ratio (0.0 = full self, 1.0 = full other)
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

/// EdgeSpectra contains color spectra for all four edges
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeSpectra {
    pub top: ColorSpectrum,
    pub right: ColorSpectrum,
    pub bottom: ColorSpectrum,
    pub left: ColorSpectrum,
}

impl EdgeSpectra {
    /// Create new EdgeSpectra with the given spectra
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

    /// Create `EdgeSpectra` filled with the specified color
    /// Sample counts are based on aspect ratio (more samples for longer edges)
    #[must_use]
    pub fn fill(color: Color, width: usize, height: usize, samples_per_1000px: usize) -> Self {
        let top_samples = ((width as f32 / 1000.0) * samples_per_1000px as f32).max(1.0) as usize;
        let bottom_samples = top_samples;
        let left_samples = ((height as f32 / 1000.0) * samples_per_1000px as f32).max(1.0) as usize;
        let right_samples = left_samples;

        Self {
            top: ColorSpectrum::fill(color, top_samples),
            right: ColorSpectrum::fill(color, right_samples),
            bottom: ColorSpectrum::fill(color, bottom_samples),
            left: ColorSpectrum::fill(color, left_samples),
        }
    }

    /// Create `EdgeSpectra` with all black colors
    /// Sample counts are based on aspect ratio (more samples for longer edges)
    #[must_use]
    pub fn black(width: usize, height: usize, samples_per_1000px: usize) -> Self {
        Self::fill(Color::black(), width, height, samples_per_1000px)
    }

    /// Create a dummy EdgeSpectra (filled with magenta to make it obviously a placeholder)
    #[must_use]
    pub fn dummy(width: usize, height: usize) -> Self {
        Self::fill(
            Color {
                r: 255,
                g: 0,
                b: 255,
            },
            width,
            height,
            1,
        )
    }

    /// Blend two `EdgeSpectra` together with a ratio (0.0 = full self, 1.0 = full other)
    #[must_use]
    pub fn blend(&self, other: &EdgeSpectra, ratio: f32) -> EdgeSpectra {
        EdgeSpectra {
            top: self.top.blend(&other.top, ratio),
            right: self.right.blend(&other.right, ratio),
            bottom: self.bottom.blend(&other.bottom, ratio),
            left: self.left.blend(&other.left, ratio),
        }
    }
}

// Trait implementations for EdgeSpectra

impl Default for EdgeSpectra {
    fn default() -> Self {
        Self::black(1920, 1080, 50)
    }
}

impl Add for EdgeSpectra {
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

impl Mul<f32> for EdgeSpectra {
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
