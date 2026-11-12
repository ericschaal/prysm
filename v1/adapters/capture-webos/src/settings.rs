use prysm_ports::ValidationError;
use serde::{Deserialize, Serialize};

/// `{ "kind": "webos", "width": 640, "height": 360, "fps": 30, "include_ui": true }`
///
/// LG's capture driver scales to `width`×`height` and paces at `fps`; all
/// fields are optional. `include_ui` blends on-screen menus into the capture
/// (LG dump mode 3) instead of video only (mode 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebosSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub include_ui: bool,
}

impl Default for WebosSettings {
    fn default() -> Self {
        Self {
            width: 640,
            height: 360,
            fps: 30,
            include_ui: true,
        }
    }
}

impl WebosSettings {
    /// Rejects settings the driver can never negotiate, without touching it.
    ///
    /// # Errors
    /// The first invalid field, as `capture.<field>`.
    pub fn validate(&self) -> Result<(), ValidationError> {
        // NV12 needs even dimensions; the driver takes 16-bit sizes up to full HD.
        let even = |field: &str, value: u32, max: u32| {
            if (2..=max).contains(&value) && value.is_multiple_of(2) {
                Ok(())
            } else {
                Err(ValidationError::new(
                    format!("capture.{field}"),
                    format!("must be an even number from 2 to {max}"),
                ))
            }
        };
        even("width", self.width, 1920)?;
        even("height", self.height, 1080)?;
        if !(1..=60).contains(&self.fps) {
            return Err(ValidationError::new("capture.fps", "must be 1 to 60"));
        }
        Ok(())
    }

    /// The driver's options; only called on validated settings.
    pub(crate) fn options(&self) -> prysm_capture_webos_sys::Options {
        let narrow = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
        prysm_capture_webos_sys::Options {
            width: narrow(self.width),
            height: narrow(self.height),
            fps: u8::try_from(self.fps).unwrap_or(u8::MAX),
            include_ui: self.include_ui,
        }
    }
}
