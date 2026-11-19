/// Positive capture dimensions up to 4K UHD (3840 × 2160).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureResolution([u32; 2]);

impl CaptureResolution {
    pub fn new(width: u32, height: u32) -> Result<Self, &'static str> {
        if width == 0 || height == 0 {
            return Err("capture resolution width and height must be positive");
        }
        if width > 3840 || height > 2160 {
            return Err("capture resolution must not exceed 3840 × 2160 (4K UHD)");
        }
        Ok(Self([width, height]))
    }

    #[must_use]
    pub const fn dimensions(self) -> [u32; 2] {
        self.0
    }
}

impl Default for CaptureResolution {
    fn default() -> Self {
        Self([640, 360])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolutions_reject_invalid_dimensions_and_accept_odd_rgb_widths() {
        for (width, height) in [
            (0, 360),
            (640, 0),
            (3841, 2160),
            (3840, 2161),
            (1_000_000_000, 1_000_000_000),
            (u32::MAX, u32::MAX),
        ] {
            assert!(CaptureResolution::new(width, height).is_err());
        }
        let uhd = CaptureResolution::new(3840, 2160).unwrap();
        assert_eq!(uhd.dimensions(), [3840, 2160]);
        let resolution = CaptureResolution::new(3, 2).unwrap();
        assert_eq!(resolution.dimensions(), [3, 2]);
    }
}
