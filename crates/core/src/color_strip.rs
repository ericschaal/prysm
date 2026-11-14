use crate::{LinearColor, SampleDensity};
use std::num::NonZeroUsize;
use std::sync::Arc;

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
        if count == self.samples.len() {
            return Some(self.samples[index]);
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
        let mut samples = Vec::with_capacity(count);
        samples.extend((0..count).filter_map(|i| self.color_at(i, count)));
        samples
    }

    /// Blend two strips together with a ratio (0.0 = full self, 1.0 = full other)
    #[must_use]
    #[allow(
        clippy::missing_panics_doc,
        reason = "indices are within the output count"
    )]
    pub fn blend(&self, other: &ColorStrip, ratio: f32) -> ColorStrip {
        let result_len = self.samples.len().max(other.samples.len());
        let blended = (0..result_len)
            .map(|i| {
                self.color_at(i, result_len)
                    .expect("bounded strip index")
                    .blend(
                        &other.color_at(i, result_len).expect("bounded strip index"),
                        ratio,
                    )
            })
            .collect();
        Self {
            samples: Arc::new(blended),
        }
    }

    /// Transform samples in place, copying only when another observer shares them.
    pub fn map_colors(&mut self, mut map: impl FnMut(LinearColor) -> LinearColor) {
        for color in Arc::make_mut(&mut self.samples) {
            *color = map(*color);
        }
    }
}

impl Default for ColorStrip {
    fn default() -> Self {
        Self::black(NonZeroUsize::MIN)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjustments_reuse_unique_storage_and_preserve_shared_samples() {
        let original = ColorStrip::new(vec![LinearColor::new(0.02, 0.01, 0.0); 7]).unwrap();
        let mut adjusted = original.clone();
        adjusted.map_colors(|color| color.lift_shadows(crate::ShadowLiftPercent(30)) * 0.5);
        assert_eq!(original.samples[0], LinearColor::new(0.02, 0.01, 0.0));
        assert_eq!(
            adjusted.samples[0],
            original.samples[0].lift_shadows(crate::ShadowLiftPercent(30)) * 0.5
        );
        let allocation = adjusted.samples.as_ptr();
        adjusted.map_colors(|color| color * 0.5);
        assert_eq!(adjusted.samples.as_ptr(), allocation);
    }

    #[test]
    fn equal_length_sampling_and_blending_preserve_exact_sample_positions() {
        let values: Vec<_> = (0..655_u16)
            .map(|i| LinearColor::new(f32::from(i % 7) / 7.0, 0.0, 0.0))
            .collect();
        let strip = ColorStrip::new(values.clone()).unwrap();
        assert_eq!(strip.resample(values.len()), values);
        let other = ColorStrip::fill(
            LinearColor::new(0.0, 0.5, 1.0),
            NonZeroUsize::new(values.len()).unwrap(),
        );
        let blended = strip.blend(&other, 0.25);
        for (i, color) in values.iter().enumerate() {
            assert_eq!(blended.samples[i], color.blend(&other.samples[i], 0.25));
        }
        let single = ColorStrip::black(NonZeroUsize::MIN);
        let blended = single.blend(&strip, 0.25);
        for (i, color) in values.iter().enumerate() {
            assert_eq!(blended.samples[i], LinearColor::black().blend(color, 0.25));
        }
    }

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
