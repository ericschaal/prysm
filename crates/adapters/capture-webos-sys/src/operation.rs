//! The native calls that answer a status code.

/// A native call of `libvtcapture` or `libhalgal` that answers a status
/// code; [`crate::Error::Status`] names the one that failed. One set for
/// both libraries, as both report through the one [`crate::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::VariantArray, strum::IntoStaticStr)]
#[strum(const_into_str)]
pub enum Operation {
    #[strum(serialize = "vtCapture_create")]
    VtCreate,
    #[strum(serialize = "vtCapture_init")]
    VtInit,
    #[strum(serialize = "vtCapture_preprocess")]
    VtPreprocess,
    #[strum(serialize = "vtCapture_planeInfo")]
    VtPlaneInfo,
    #[strum(serialize = "vtCapture_process")]
    VtProcess,
    #[strum(serialize = "vtCapture_currentCaptureBuffInfo")]
    VtCurrentCaptureBuffInfo,
    #[strum(serialize = "vtCapture_stop")]
    VtStop,
    #[strum(serialize = "vtCapture_postprocess")]
    VtPostprocess,
    #[strum(serialize = "vtCapture_finalize")]
    VtFinalize,
    #[strum(serialize = "HAL_GAL_Init")]
    HalInit,
    #[strum(serialize = "HAL_GAL_CreateSurface")]
    HalCreateSurface,
    #[strum(serialize = "HAL_GAL_CaptureFrameBuffer")]
    HalCaptureFrameBuffer,
    #[strum(serialize = "HAL_GAL_DestroySurface")]
    HalDestroySurface,
}

pub(crate) fn status(code: i32, operation: Operation) -> Result<(), crate::Error> {
    if code == 0 {
        Ok(())
    } else {
        Err(crate::Error::Status { operation, code })
    }
}

impl Operation {
    /// Every operation, in declaration order.
    pub const ALL: [Self; 13] = *<Self as strum::VariantArray>::VARIANTS.as_array().unwrap();

    /// The native function's name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.into_str()
    }

    /// What status `code` of this call is suspected to mean, as error
    /// messages show it after the name; empty where nothing is known.
    pub(crate) const fn hint(self, code: i32) -> &'static str {
        match (self, code) {
            (Self::VtInit, 11) => {
                " (11: on webOS 10.3.2 the library's code suggests the TV's resource manager did not grant the capture resource)"
            }
            _ => "",
        }
    }
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_is_listed_once_with_its_own_name() {
        for (index, operation) in Operation::ALL.into_iter().enumerate() {
            assert_eq!(operation as usize, index, "{operation:?}");
        }
        assert_eq!(
            Operation::ALL.map(Operation::as_str),
            [
                "vtCapture_create",
                "vtCapture_init",
                "vtCapture_preprocess",
                "vtCapture_planeInfo",
                "vtCapture_process",
                "vtCapture_currentCaptureBuffInfo",
                "vtCapture_stop",
                "vtCapture_postprocess",
                "vtCapture_finalize",
                "HAL_GAL_Init",
                "HAL_GAL_CreateSurface",
                "HAL_GAL_CaptureFrameBuffer",
                "HAL_GAL_DestroySurface",
            ],
        );
    }
}
