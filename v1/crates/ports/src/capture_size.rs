//! The frame size and rate a local capture (a video device, a video file)
//! is configured with: their defaults and their one validation, shared so
//! every such source accepts the same values with the same messages.
use crate::ValidationError;
use prysm_core::CaptureResolution;

/// 640, from [`CaptureResolution::default`].
#[must_use]
pub fn default_width() -> u32 {
    CaptureResolution::default().dimensions()[0]
}

/// 360, from [`CaptureResolution::default`].
#[must_use]
pub fn default_height() -> u32 {
    CaptureResolution::default().dimensions()[1]
}

#[must_use]
pub const fn default_fps() -> u32 {
    30
}

/// Rejects a size [`CaptureResolution`] refuses (at `width` or `height`,
/// whichever is wrong) and a zero rate (at `fps`).
///
/// # Errors
/// The first invalid field, relative to the `capture` section.
pub fn validate_capture_size(width: u32, height: u32, fps: u32) -> Result<(), ValidationError> {
    if let Err(reason) = CaptureResolution::new(width, height) {
        // The width alone, with any valid height, says which is wrong.
        let field = if CaptureResolution::new(width, 1).is_err() {
            "width"
        } else {
            "height"
        };
        return Err(ValidationError::new(field, reason));
    }
    if fps == 0 {
        return Err(ValidationError::new("fps", "must be a positive frame rate"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrong_field_is_named() {
        assert_eq!(validate_capture_size(640, 360, 30), Ok(()));
        for (width, height, fps, field) in [
            (0, 360, 30, "width"),
            (3841, 360, 30, "width"),
            (640, 0, 30, "height"),
            (640, 2161, 30, "height"),
            (3841, 2161, 30, "width"),
            (640, 360, 0, "fps"),
        ] {
            let error = validate_capture_size(width, height, fps).unwrap_err();
            assert_eq!(error.field, field, "{width}x{height}@{fps}");
        }
        assert_eq!(
            (default_width(), default_height(), default_fps()),
            (640, 360, 30)
        );
    }
}
