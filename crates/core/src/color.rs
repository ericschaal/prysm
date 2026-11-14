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
}

impl Default for Color {
    fn default() -> Self {
        Self::black()
    }
}
