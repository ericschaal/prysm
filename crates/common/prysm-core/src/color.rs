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
    #[must_use]
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    #[must_use]
    pub fn black() -> Self {
        Self { r: 0, g: 0, b: 0 }
    }

    /// Blend two colors with a ratio (0.0 = full self, 1.0 = full other)
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "8-bit color arithmetic deliberately quantizes with saturating float casts"
    )]
    pub fn blend(&self, other: &Color, ratio: f32) -> Color {
        let ratio = ratio.clamp(0.0, 1.0);
        Color {
            r: (f32::from(self.r) * (1.0 - ratio) + f32::from(other.r) * ratio) as u8,
            g: (f32::from(self.g) * (1.0 - ratio) + f32::from(other.g) * ratio) as u8,
            b: (f32::from(self.b) * (1.0 - ratio) + f32::from(other.b) * ratio) as u8,
        }
    }

    /// Apply brightness multiplier (0.0 to 1.0)
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "8-bit color arithmetic deliberately quantizes with saturating float casts"
    )]
    pub fn with_brightness(&self, brightness: f32) -> Color {
        let brightness = brightness.clamp(0.0, 1.0);
        Color {
            r: (f32::from(self.r) * brightness) as u8,
            g: (f32::from(self.g) * brightness) as u8,
            b: (f32::from(self.b) * brightness) as u8,
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

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "8-bit color arithmetic deliberately quantizes with saturating float casts"
)]
impl Mul<f32> for Color {
    type Output = Self;

    fn mul(self, scalar: f32) -> Self {
        let scalar = scalar.clamp(0.0, 1.0);
        Self {
            r: (f32::from(self.r) * scalar) as u8,
            g: (f32::from(self.g) * scalar) as u8,
            b: (f32::from(self.b) * scalar) as u8,
        }
    }
}

impl MulAssign<f32> for Color {
    fn mul_assign(&mut self, scalar: f32) {
        *self = *self * scalar;
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "8-bit color arithmetic deliberately quantizes with saturating float casts"
)]
impl Div<f32> for Color {
    type Output = Self;

    fn div(self, scalar: f32) -> Self {
        if scalar == 0.0 {
            return Self::black();
        }
        let scalar = 1.0 / scalar;
        Self {
            r: ((f32::from(self.r) * scalar).min(255.0)) as u8,
            g: ((f32::from(self.g) * scalar).min(255.0)) as u8,
            b: ((f32::from(self.b) * scalar).min(255.0)) as u8,
        }
    }
}

impl DivAssign<f32> for Color {
    fn div_assign(&mut self, scalar: f32) {
        *self = *self / scalar;
    }
}
