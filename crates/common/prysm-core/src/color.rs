use std::fmt;
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign};

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
