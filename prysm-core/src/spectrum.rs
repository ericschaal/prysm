use crate::LinearColor;
use std::ops::{Add, Mul};
use std::sync::Arc;

/// Gradient of colors along an edge, in linear RGB space.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    samples: Arc<Vec<LinearColor>>,
}

impl Spectrum {
    /// Create a new `Spectrum` from a vector of linear color samples
    pub fn new(samples: Vec<LinearColor>) -> Self {
        assert!(
            !samples.is_empty(),
            "Spectrum must have at least one sample"
        );
        Self {
            samples: Arc::new(samples),
        }
    }

    /// Create a `Spectrum` filled with the specified color
    pub fn fill(color: LinearColor, count: usize) -> Self {
        Self::new(vec![color; count])
    }

    /// Create a `Spectrum` with all black samples
    pub fn black(count: usize) -> Self {
        Self::fill(LinearColor::black(), count)
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

    /// Quantize the spectrum into exactly N linear colors
    pub fn quantize(&self, count: usize) -> Vec<LinearColor> {
        (0..count).map(|i| self.color_at(i, count)).collect()
    }

    /// Blend two spectra together with a ratio (0.0 = full self, 1.0 = full other)
    pub fn blend(&self, other: &Spectrum, ratio: f32) -> Spectrum {
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
        Spectrum::new(blended)
    }
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::black(1)
    }
}

impl Add for Spectrum {
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

impl Mul<f32> for Spectrum {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        let scaled: Vec<LinearColor> = self.samples.iter().map(|&c| c * scalar).collect();
        Self::new(scaled)
    }
}

/// Color spectra for all four screen edges, in linear RGB space.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeSpectra {
    pub top: Spectrum,
    pub right: Spectrum,
    pub bottom: Spectrum,
    pub left: Spectrum,
}

impl EdgeSpectra {
    /// Create new EdgeSpectra with the given spectra
    pub fn new(top: Spectrum, right: Spectrum, bottom: Spectrum, left: Spectrum) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Create `EdgeSpectra` filled with the specified color
    #[must_use]
    pub fn fill(
        color: LinearColor,
        width: usize,
        height: usize,
        sample_density: crate::SampleDensity,
    ) -> Self {
        let top_samples = sample_density.samples_for_length(width);
        let bottom_samples = top_samples;
        let left_samples = sample_density.samples_for_length(height);
        let right_samples = left_samples;

        Self {
            top: Spectrum::fill(color, top_samples),
            right: Spectrum::fill(color, right_samples),
            bottom: Spectrum::fill(color, bottom_samples),
            left: Spectrum::fill(color, left_samples),
        }
    }

    /// Create `EdgeSpectra` with all black colors
    #[must_use]
    pub fn black(width: usize, height: usize, sample_density: crate::SampleDensity) -> Self {
        Self::fill(LinearColor::black(), width, height, sample_density)
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

impl Default for EdgeSpectra {
    fn default() -> Self {
        Self::black(1920, 1080, crate::SampleDensity(50))
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
