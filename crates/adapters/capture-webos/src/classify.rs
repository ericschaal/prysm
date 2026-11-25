//! The crate's one classification of failures: what each tap does after
//! each cause. Retried only where a reason says why the retry converges;
//! everything else waits for a change of the video signal or leaves this
//! layer ([`prysm_ports::Escalation`]).
use crate::source::Limits;
use prysm_capture_webos_sys as sys;
use prysm_ports::{Escalation, Fix, Schedule};

/// What the video tap does after a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recovery {
    /// Try again on a schedule, for this reason.
    Retry(Reason),
    /// Nothing tells when the TV will give this video except a change of
    /// the video signal: wait for one.
    Wait,
    /// Leaves this layer.
    Escalate(Escalation),
}

/// Why trying the video tap again converges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    /// The driver answers not ready while the TV's video pipeline starts,
    /// which it finishes within [`Limits::preparation`].
    DriverPreparing,
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

/// What a [`Reason`]'s retries end in once its schedule gives up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GiveUp {
    /// The source fails fast.
    Fail,
    /// [`Recovery::Wait`].
    Wait,
}

impl Reason {
    pub(crate) fn schedule(self, limits: &Limits) -> Schedule {
        match self {
            Self::DriverPreparing => Schedule::Within {
                every: limits.not_ready_retry,
                limit: limits.preparation,
            },
            Self::FreshDriver | Self::StalledDriver => Schedule::Once,
        }
    }

    pub(crate) fn gives_up(self) -> GiveUp {
        match self {
            Self::DriverPreparing | Self::FreshDriver => GiveUp::Fail,
            Self::StalledDriver => GiveUp::Wait,
        }
    }
}

impl Recovery {
    /// What the source's failure escalates as when this fault ends it: its
    /// own escalation, else fail fast (a retry that gave up).
    pub(crate) fn escalation(self) -> Escalation {
        match self {
            Self::Escalate(escalation) => escalation,
            Self::Retry(_) | Self::Wait => Escalation::FAIL_FAST,
        }
    }
}

/// What failed in the video tap.
#[derive(Debug)]
pub(crate) enum VideoCause<'a> {
    Native(&'a sys::Error),
    /// The frame pool could not allocate a frame.
    Allocation,
    /// The initialized driver gave no buffer for [`Limits::pending`].
    Stall,
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
        VideoCause::Native(E::Luna(_)) | VideoCause::SignalMask => {
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
        }) => Recovery::Wait,
        VideoCause::Native(
            E::Status { .. } | E::Exception(_) | E::Invalid(_) | E::Failed | E::Device { .. },
        )
        | VideoCause::Allocation => Recovery::Retry(Reason::FreshDriver),
        VideoCause::Stall => Recovery::Retry(Reason::StalledDriver),
    }
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

    /// What a status of each operation leads to; no wildcard, so a new
    /// operation needs its own arm here.
    fn status_recovery(operation: sys::Operation, code: i32) -> Recovery {
        use sys::Operation as O;
        let (retry, wait) = (Recovery::Retry(Reason::FreshDriver), Recovery::Wait);
        match operation {
            O::VtCurrentCaptureBuffInfo if code == 12 => wait,
            O::VtPlaneInfo if code == 13 => wait,
            O::VtCreate
            | O::VtInit
            | O::VtPreprocess
            | O::VtPlaneInfo
            | O::VtProcess
            | O::VtCurrentCaptureBuffInfo
            | O::VtStop
            | O::VtPostprocess
            | O::VtFinalize
            | O::HalInit
            | O::HalCreateSurface
            | O::HalCaptureFrameBuffer
            | O::HalDestroySurface => retry,
        }
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
        for operation in sys::Operation::ALL {
            for code in [1, 12, 13] {
                let recovery = status_recovery(operation, code);
                rows.push((status(operation, code), recovery, may_pass));
            }
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
                VideoCause::SignalMask,
                Recovery::Escalate(Escalation::FAIL_FAST),
            ),
        ] {
            assert_eq!(video(cause), recovery);
        }
        assert_eq!(ui(UiCause::Capture), UiRecovery::MayPass);
        assert_eq!(ui(UiCause::SignalMask), UiRecovery::Unsupported);
    }

    #[test]
    fn every_reason_has_its_schedule() {
        let limits = Limits::TV;
        for (reason, schedule) in [
            (
                Reason::DriverPreparing,
                Schedule::Within {
                    every: limits.not_ready_retry,
                    limit: limits.preparation,
                },
            ),
            (Reason::FreshDriver, Schedule::Once),
            (Reason::StalledDriver, Schedule::Once),
        ] {
            assert_eq!(reason.schedule(&limits), schedule, "{reason:?}");
        }
        for (reason, give_up) in [
            (Reason::DriverPreparing, GiveUp::Fail),
            (Reason::FreshDriver, GiveUp::Fail),
            (Reason::StalledDriver, GiveUp::Wait),
        ] {
            assert_eq!(reason.gives_up(), give_up, "{reason:?}");
        }
    }
}
