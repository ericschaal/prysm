//! The native calls that answer a status code.

/// A native call of `libvtcapture` or `libhalgal` that answers a status
/// code; [`crate::Error::Status`] names the one that failed. One set for
/// both libraries, as both report through the one [`crate::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    VtCreate,
    VtInit,
    VtPreprocess,
    VtPlaneInfo,
    VtProcess,
    VtCurrentCaptureBuffInfo,
    VtStop,
    VtPostprocess,
    VtFinalize,
    HalInit,
    HalCreateSurface,
    HalCaptureFrameBuffer,
    HalDestroySurface,
}

impl Operation {
    /// Every operation, in declaration order.
    pub const ALL: [Self; 13] = [
        Self::VtCreate,
        Self::VtInit,
        Self::VtPreprocess,
        Self::VtPlaneInfo,
        Self::VtProcess,
        Self::VtCurrentCaptureBuffInfo,
        Self::VtStop,
        Self::VtPostprocess,
        Self::VtFinalize,
        Self::HalInit,
        Self::HalCreateSurface,
        Self::HalCaptureFrameBuffer,
        Self::HalDestroySurface,
    ];

    /// The native function's name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VtCreate => "vtCapture_create",
            Self::VtInit => "vtCapture_init",
            Self::VtPreprocess => "vtCapture_preprocess",
            Self::VtPlaneInfo => "vtCapture_planeInfo",
            Self::VtProcess => "vtCapture_process",
            Self::VtCurrentCaptureBuffInfo => "vtCapture_currentCaptureBuffInfo",
            Self::VtStop => "vtCapture_stop",
            Self::VtPostprocess => "vtCapture_postprocess",
            Self::VtFinalize => "vtCapture_finalize",
            Self::HalInit => "HAL_GAL_Init",
            Self::HalCreateSurface => "HAL_GAL_CreateSurface",
            Self::HalCaptureFrameBuffer => "HAL_GAL_CaptureFrameBuffer",
            Self::HalDestroySurface => "HAL_GAL_DestroySurface",
        }
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

    /// Its position in [`Operation::ALL`]. No wildcard: a new operation
    /// does not compile until it is given its position here, which is in
    /// `ALL` too.
    const fn position(operation: Operation) -> usize {
        match operation {
            Operation::VtCreate => 0,
            Operation::VtInit => 1,
            Operation::VtPreprocess => 2,
            Operation::VtPlaneInfo => 3,
            Operation::VtProcess => 4,
            Operation::VtCurrentCaptureBuffInfo => 5,
            Operation::VtStop => 6,
            Operation::VtPostprocess => 7,
            Operation::VtFinalize => 8,
            Operation::HalInit => 9,
            Operation::HalCreateSurface => 10,
            Operation::HalCaptureFrameBuffer => 11,
            Operation::HalDestroySurface => 12,
        }
    }

    #[test]
    fn every_operation_is_listed_once_with_its_own_name() {
        for (index, operation) in Operation::ALL.into_iter().enumerate() {
            assert_eq!(position(operation), index, "{operation:?}");
            assert_eq!(operation as usize, index, "{operation:?}");
        }
        let mut names: Vec<_> = Operation::ALL.map(Operation::as_str).into();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Operation::ALL.len());
    }
}
