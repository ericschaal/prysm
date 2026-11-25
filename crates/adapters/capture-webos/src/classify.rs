//! The crate's one classification of failures: what each tap does after
//! each cause. Retried only where a reason says why the retry converges;
//! everything else waits for a change of the video signal or leaves this
//! layer ([`prysm_ports::Escalation`]).
use prysm_capture_webos_sys as sys;
use prysm_ports::{Escalation, Fix};

/// What the video tap does after a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recovery {
    /// Open a new driver, for this reason.
    Retry(Reason),
    /// Nothing tells when the TV will give this video except a change of
    /// the video signal: wait for one.
    Withheld,
    /// Leaves this layer.
    Escalate(Escalation),
}

/// Why trying the video tap again converges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    /// A driver that failed may hold broken state that a new one does not:
    /// one new driver tells, and a second failure with no frame or change
    /// of the video signal between proves it is not that.
    FreshDriver,
    /// An initialized driver that gets no buffer may be broken the same
    /// way: one new driver tells. A second stall with no frame or change of
    /// the video signal between is the TV withholding the picture
    /// (protected content, say), which only a change of the video ends.
    StalledDriver,
}

/// What a [`Reason`]'s retry ends in once its new driver failed too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GiveUp {
    /// The source fails fast.
    Fail,
    /// [`Recovery::Withheld`].
    Withheld,
}

impl Reason {
    pub(crate) fn gives_up(self) -> GiveUp {
        match self {
            Self::FreshDriver => GiveUp::Fail,
            Self::StalledDriver => GiveUp::Withheld,
        }
    }
}

impl Recovery {
    /// What the source's failure escalates as when this fault ends it: its
    /// own escalation, else fail fast (a retry that gave up).
    pub(crate) fn escalation(self) -> Escalation {
        match self {
            Self::Escalate(escalation) => escalation,
            Self::Retry(_) | Self::Withheld => Escalation::FAIL_FAST,
        }
    }
}

/// What failed in the video tap.
#[derive(Debug)]
pub(crate) enum VideoCause<'a> {
    Native(&'a sys::Error),
    /// The frame pool could not allocate a frame.
    Allocation,
    /// The initialized driver gave no buffer for
    /// [`crate::source::Limits::pending`].
    Stall,
    /// The driver stayed not ready for [`crate::source::Limits::preparation`],
    /// within which the TV's video pipeline starts: waiting longer does not
    /// converge.
    Unprepared,
    /// The capture thread could not block its signals.
    SignalMask,
}

/// The video tap's rules, one row per cause. A status is matched by its
/// operation and code: the same number means different things per
/// operation. `vtCapture_init` 1 and 17 and the buffer's 2 are answers, not
/// errors ([`crate::source::Acquired`]).
pub(crate) fn video(cause: VideoCause<'_>) -> Recovery {
    use sys::{Error as E, Operation as O};
    match cause {
        VideoCause::Native(E::Load { .. }) => Recovery::Escalate(Escalation::Ask(Fix::Unsupported)),
        VideoCause::Native(E::Permission(_)) => Recovery::Escalate(Escalation::Ask(Fix::SetUp)),
        VideoCause::Native(E::Create(_) | E::Registered(_)) => {
            Recovery::Escalate(Escalation::Restart)
        }
        VideoCause::Native(E::Luna(_)) | VideoCause::Unprepared | VideoCause::SignalMask => {
            Recovery::Escalate(Escalation::FAIL_FAST)
        }
        // The TV's resource policy took the capture (seen at standby, with
        // the signal changing in the same instant).
        VideoCause::Native(E::Status {
            operation: O::VtCurrentCaptureBuffInfo,
            code: 12,
        })
        // No video plane to capture (seen at start while the video is
        // unknown, and for ever under AirPlay mirroring).
        | VideoCause::Native(E::Status {
            operation: O::VtPlaneInfo,
            code: 13,
        }) => Recovery::Withheld,
        VideoCause::Native(
            E::Status { .. } | E::Exception(_) | E::Invalid(_) | E::Failed | E::Device { .. },
        )
        | VideoCause::Allocation => Recovery::Retry(Reason::FreshDriver),
        VideoCause::Stall => Recovery::Retry(Reason::StalledDriver),
    }
}

/// What a release that did not confirm escalates as, for either tap: the
/// driver may still hold the device, which only a new process frees.
pub(crate) fn unconfirmed_release() -> Escalation {
    Escalation::Restart
}

/// What the TV UI tap does after a fault. It never fails the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiRecovery {
    /// Cannot work in this process: not opened again in this source.
    Unsupported,
    /// May pass: opened again once the video or its context changes.
    MayPass,
}

/// What failed in the TV UI tap.
#[derive(Debug)]
pub(crate) enum UiCause<'a> {
    Open(&'a sys::Error),
    /// Capturing or reading the plane, or allocating its frame.
    Capture,
    /// The capture thread could not block its signals.
    SignalMask,
}

/// The TV UI tap's rules: a missing library, symbol or `/dev/gfx` cannot
/// heal in this process; any other failure may pass.
pub(crate) fn ui(cause: UiCause<'_>) -> UiRecovery {
    use sys::Error as E;
    match cause {
        UiCause::Open(E::Load { .. }) | UiCause::SignalMask => UiRecovery::Unsupported,
        UiCause::Open(E::Device { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            UiRecovery::Unsupported
        }
        UiCause::Open(
            E::Device { .. }
            | E::Permission(_)
            | E::Registered(_)
            | E::Luna(_)
            | E::Create(_)
            | E::Status { .. }
            | E::Exception(_)
            | E::Invalid(_)
            | E::Failed,
        )
        | UiCause::Capture => UiRecovery::MayPass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_webos_luna_sys::Error as Luna;
    use std::io::ErrorKind;

    fn status(operation: sys::Operation, code: i32) -> sys::Error {
        sys::Error::Status { operation, code }
    }

    fn device(kind: ErrorKind) -> sys::Error {
        sys::Error::Device {
            path: "/dev/gfx",
            source: std::io::Error::from(kind),
        }
    }

    fn luna(error: Luna) -> sys::Error {
        sys::Error::from(error)
    }

    /// LG's library is missing on the development host; `None` on a TV.
    fn load() -> Option<sys::Error> {
        if std::path::Path::new("/dev/gfx").exists() {
            return None; // On a TV this would open real hardware.
        }
        sys::UiCapture::open(320, 180).err()
    }

    /// One row per `sys::Error` variant, and per operation for the status
    /// codes that have their own rule somewhere, for both taps.
    fn rows() -> Vec<(sys::Error, Recovery, UiRecovery)> {
        let retry = Recovery::Retry(Reason::FreshDriver);
        let (unsupported, may_pass) = (UiRecovery::Unsupported, UiRecovery::MayPass);
        let message = || "failed".to_owned();
        let mut rows = vec![
            (
                luna(Luna::Denied {
                    name: "a".into(),
                    message: message(),
                }),
                Recovery::Escalate(Escalation::Ask(Fix::SetUp)),
                may_pass,
            ),
            (
                luna(Luna::Unremoved {
                    name: "a".into(),
                    message: message(),
                }),
                Recovery::Escalate(Escalation::Restart),
                may_pass,
            ),
            (
                luna(Luna::Luna {
                    operation: "LSRegister",
                    message: message(),
                }),
                Recovery::Escalate(Escalation::FAIL_FAST),
                may_pass,
            ),
            (
                sys::Error::Create(Box::new(status(sys::Operation::VtCreate, 1))),
                Recovery::Escalate(Escalation::Restart),
                may_pass,
            ),
            (sys::Error::Exception("boom".into()), retry, may_pass),
            (sys::Error::Invalid("stride"), retry, may_pass),
            (sys::Error::Failed, retry, may_pass),
            (device(ErrorKind::NotFound), retry, unsupported),
            (device(ErrorKind::OutOfMemory), retry, may_pass),
        ];
        // A status code means something only for its operation: 12 of the
        // buffer is the TV's resource policy taking the capture (seen at
        // standby), 13 of the plane is no video plane (seen at start and
        // under AirPlay mirroring). Any other status opens a new driver.
        use sys::Operation as O;
        let withheld = Recovery::Withheld;
        rows.extend([
            (status(O::VtCurrentCaptureBuffInfo, 12), withheld, may_pass),
            (status(O::VtPlaneInfo, 13), withheld, may_pass),
            (status(O::VtCurrentCaptureBuffInfo, 13), retry, may_pass),
            (status(O::VtPlaneInfo, 12), retry, may_pass),
            (status(O::VtInit, 11), retry, may_pass),
            (status(O::VtProcess, 12), retry, may_pass),
            (status(O::HalInit, -1), retry, may_pass),
        ]);
        for operation in O::ALL {
            rows.push((status(operation, 1), retry, may_pass));
        }
        if let Some(load) = load() {
            rows.push((
                load,
                Recovery::Escalate(Escalation::Ask(Fix::Unsupported)),
                unsupported,
            ));
        }
        rows
    }

    #[test]
    fn every_native_error_has_its_row() {
        for (error, recovery, ui_recovery) in rows() {
            assert_eq!(video(VideoCause::Native(&error)), recovery, "{error}");
            assert_eq!(ui(UiCause::Open(&error)), ui_recovery, "{error}");
        }
    }

    /// The source's error carries the row's escalation, and only a
    /// restart is what the hosts' `needs_new_process` reads.
    #[test]
    fn a_native_fault_escalates_as_its_row_says() {
        for (error, recovery, _) in rows() {
            let shown = error.to_string();
            let fault = crate::backend::fault("cannot open webOS video capture", error);
            assert_eq!(fault.recovery, recovery, "{shown}");
            assert_eq!(fault.error.escalation(), recovery.escalation(), "{shown}");
            assert_eq!(
                crate::needs_new_process(&fault.error),
                recovery == Recovery::Escalate(Escalation::Restart),
                "{shown}"
            );
        }
    }

    #[test]
    fn every_other_cause_has_its_row() {
        for (cause, recovery) in [
            (VideoCause::Allocation, Recovery::Retry(Reason::FreshDriver)),
            (VideoCause::Stall, Recovery::Retry(Reason::StalledDriver)),
            (
                VideoCause::Unprepared,
                Recovery::Escalate(Escalation::FAIL_FAST),
            ),
            (
                VideoCause::SignalMask,
                Recovery::Escalate(Escalation::FAIL_FAST),
            ),
        ] {
            assert_eq!(video(cause), recovery);
        }
        assert_eq!(unconfirmed_release(), Escalation::Restart);
        assert_eq!(ui(UiCause::Capture), UiRecovery::MayPass);
        assert_eq!(ui(UiCause::SignalMask), UiRecovery::Unsupported);
    }
}
