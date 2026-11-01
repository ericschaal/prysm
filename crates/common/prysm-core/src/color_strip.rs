use crate::LinearColor;
use std::num::NonZeroUsize;
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
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "sample counts use approximate geometry and saturate to the edge length"
    )]
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
        crate::Config::default().sample_density
    }
}

/// Gradient of colors along an edge, in linear RGB space.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorStrip {
    samples: Arc<Vec<LinearColor>>,
}

/// A color strip needs at least one sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyColorStrip;

impl std::fmt::Display for EmptyColorStrip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("color strip must have at least one sample")
    }
}

impl std::error::Error for EmptyColorStrip {}

impl ColorStrip {
    /// Create a strip from linear color samples.
    ///
    /// # Errors
    /// Returns [`EmptyColorStrip`] if no samples are supplied.
    pub fn new(samples: Vec<LinearColor>) -> Result<Self, EmptyColorStrip> {
        if samples.is_empty() {
            return Err(EmptyColorStrip);
        }
        Ok(Self {
            samples: Arc::new(samples),
        })
    }

    /// Create a nonempty strip filled with the specified color.
    #[must_use]
    pub fn fill(color: LinearColor, count: NonZeroUsize) -> Self {
        Self {
            samples: Arc::new(vec![color; count.get()]),
        }
    }

    /// Create a nonempty strip of black samples.
    #[must_use]
    pub fn black(count: NonZeroUsize) -> Self {
        Self::fill(LinearColor::black(), count)
    }

    /// Get the number of samples in the strip
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if the strip is empty (should never be true)
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Sample the strip at a normalized position [0.0, 1.0] using linear interpolation
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "normalized coordinates are quantized to nonnegative indices, then capped at the last sample"
    )]
    pub fn sample_at(&self, position: f32) -> LinearColor {
        let position = position.clamp(0.0, 1.0);

        if self.samples.len() == 1 {
            return self.samples[0];
        }

        let float_index = position * (self.samples.len() - 1) as f32;
        let index = (float_index.floor() as usize).min(self.samples.len() - 1);
        let next_index = (index + 1).min(self.samples.len() - 1);
        let ratio = float_index - index as f32;

        self.samples[index].blend(&self.samples[next_index], ratio)
    }

    /// Get color at specific index in a count-sized output.
    ///
    /// Returns `None` if `count` is zero or `index >= count`.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "output indices become normalized floating-point interpolation positions"
    )]
    pub fn color_at(&self, index: usize, count: usize) -> Option<LinearColor> {
        if index >= count {
            return None;
        }
        let position = if count == 1 {
            0.5
        } else {
            index as f32 / (count - 1) as f32
        };
        Some(self.sample_at(position))
    }

    /// Resample the strip into exactly N linear colors
    #[must_use]
    pub fn resample(&self, count: usize) -> Vec<LinearColor> {
        (0..count).filter_map(|i| self.color_at(i, count)).collect()
    }

    /// Blend two strips together with a ratio (0.0 = full self, 1.0 = full other)
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "strip blending resamples both operands at normalized floating-point positions"
    )]
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
        Self {
            samples: Arc::new(blended),
        }
    }
}

impl Default for ColorStrip {
    fn default() -> Self {
        Self::black(NonZeroUsize::MIN)
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "strip addition resamples both operands at normalized floating-point positions"
)]
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
        Self {
            samples: Arc::new(summed),
        }
    }
}

impl Mul<f32> for ColorStrip {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        let scaled: Vec<LinearColor> = self.samples.iter().map(|&c| c * scalar).collect();
        Self {
            samples: Arc::new(scaled),
        }
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
    /// Create new `EdgeColors` with the given strips
    #[must_use]
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
    #[allow(
        clippy::missing_panics_doc,
        reason = "sample density guarantees nonzero counts"
    )]
    pub fn fill(
        color: LinearColor,
        width: usize,
        height: usize,
        sample_density: SampleDensity,
    ) -> Self {
        let top_samples = NonZeroUsize::new(sample_density.samples_for_length(width))
            .expect("sample density produces at least one sample");
        let bottom_samples = top_samples;
        let left_samples = NonZeroUsize::new(sample_density.samples_for_length(height))
            .expect("sample density produces at least one sample");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_at_checks_output_bounds_and_preserves_resampling() {
        let start = LinearColor::black();
        let end = LinearColor::new(1.0, 0.5, 0.0);
        let strip = ColorStrip::new(vec![start, end]).unwrap();
        assert_eq!(strip.color_at(0, 0), None);
        assert_eq!(strip.color_at(1, 1), None);
        assert_eq!(strip.color_at(3, 2), None);
        assert_eq!(strip.color_at(usize::MAX, usize::MAX), None);
        assert_eq!(strip.color_at(0, 1), Some(start.blend(&end, 0.5)));
        assert_eq!(strip.color_at(0, 2), Some(start));
        assert_eq!(strip.color_at(1, 2), Some(end));
        assert_eq!(strip.resample(0), Vec::<LinearColor>::new());
        assert_eq!(strip.resample(1), vec![start.blend(&end, 0.5)]);
        assert_eq!(strip.resample(3), vec![start, start.blend(&end, 0.5), end]);
    }

    #[test]
    fn sampling_the_endpoint_caps_float_rounding_at_the_last_sample() {
        // Above f32's exact integer range, the last index can round up to len.
        let count = (1 << 24) + 4;
        let mut samples = vec![LinearColor::black(); count];
        let last = LinearColor::new(1.0, 0.5, 0.0);
        samples[count - 1] = last;
        let strip = ColorStrip::new(samples).unwrap();
        assert_eq!(strip.sample_at(1.0), last);
        assert_eq!(strip.color_at(count - 1, count), Some(last));
    }

    #[test]
    fn strip_construction_preserves_nonempty_invariant() {
        assert_eq!(ColorStrip::new(vec![]), Err(EmptyColorStrip));
        let color = LinearColor::new(1.0, 0.5, 0.0);
        let strip = ColorStrip::new(vec![color]).unwrap();
        assert_eq!(strip.sample_at(0.5), color);
        assert_eq!(ColorStrip::fill(color, NonZeroUsize::MIN).len(), 1);
        assert_eq!(
            ColorStrip::black(NonZeroUsize::MIN).sample_at(0.0),
            LinearColor::black()
        );
    }
}
