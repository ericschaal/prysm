use crate::session_lifecycle::OutputRelease;
use crate::worker::{Exit, guarded};
use prysm_ports::{
    ConfiguredOutput, Error, Escalation, FrameSource, Hangup, IdleMode, LedOutput, OpenFailure,
    OutputRelease as HandleRelease, Power, Result, Section, SessionPhase,
};

pub(super) struct IdleDecision {
    pub mode: OutputRelease,
    pub known_power: Option<Power>,
}

impl IdleDecision {
    pub const PRESERVE: Self = Self {
        mode: OutputRelease::Preserve,
        known_power: None,
    };

    fn handle_release(&self) -> HandleRelease {
        match self.mode {
            OutputRelease::Preserve => HandleRelease::Handoff,
            OutputRelease::Off => HandleRelease::Local,
            OutputRelease::Automatic => HandleRelease::Ordinary(IdleMode::Native),
        }
    }
}

pub(super) enum CaptureCleanup {
    NotOpened,
    Close(Box<dyn FrameSource>),
    Unconfirmed(Error),
}

impl CaptureCleanup {
    pub fn after_stream(source: Option<Box<dyn FrameSource>>, exit: &Exit) -> Self {
        match (source, exit) {
            (Some(source), _) => Self::Close(source),
            (None, Exit::Failed(error)) if error.escalation() == Escalation::Restart => {
                Self::Unconfirmed(error.clone())
            }
            (None, _) => Self::NotOpened,
        }
    }

    pub fn close(self) -> Result<()> {
        match self {
            Self::Close(source) => guarded("closing the source", || source.close()),
            Self::NotOpened => Ok(()),
            Self::Unconfirmed(error) => Err(error),
        }
    }
}

#[derive(Default)]
pub(super) struct SessionOutput {
    touched: Option<TouchedOutput>,
}

/// An open may have affected the device. The handle is absent after a failed
/// open or while a quiet stream has released it.
struct TouchedOutput {
    handle: Option<Box<dyn LedOutput>>,
}

impl SessionOutput {
    pub fn hold(&mut self, handle: Box<dyn LedOutput>) -> &mut Option<Box<dyn LedOutput>> {
        &mut self
            .touched
            .insert(TouchedOutput {
                handle: Some(handle),
            })
            .handle
    }

    pub fn open_failed(&mut self, failure: OpenFailure) -> Error {
        if matches!(failure, OpenFailure::Touched(_)) {
            self.touched = Some(TouchedOutput { handle: None });
        }
        failure.into_error().in_section(Section::Output)
    }

    pub fn cleanup_after(self, exit: &Exit) -> OutputCleanup {
        let fault = match exit {
            Exit::Failed(error @ Error::Faulted(SessionPhase::IdlingOutput)) => Some(error.clone()),
            _ => None,
        };
        OutputCleanup {
            touched: self.touched,
            fault,
        }
    }
}

pub(super) struct OutputCleanup {
    touched: Option<TouchedOutput>,
    fault: Option<Error>,
}

impl OutputCleanup {
    pub fn finish(self, device: &dyn ConfiguredOutput, decision: &IdleDecision) -> Result<()> {
        guarded("idling the output", move || {
            if let Some(fault) = self.fault {
                return Err(fault);
            }
            let needs_off = decision.mode == OutputRelease::Off
                && (self.touched.is_some() || decision.known_power != Some(Power::Off));
            let stop = Hangup::default();
            let released = match self.touched.and_then(|output| output.handle) {
                Some(held) => release_output(held, decision.handle_release(), &stop),
                None => Ok(()),
            };
            if needs_off && !matches!(released, Err(Error::Faulted(_))) {
                let off = device.set_power(Power::Off, &stop);
                released.and(off)
            } else {
                released
            }
        })
        .map_err(|error| error.in_section(Section::Output))
    }
}

pub(super) fn release_output(
    held: Box<dyn LedOutput>,
    release: HandleRelease,
    stop: &Hangup,
) -> Result<()> {
    guarded("releasing the output", || held.release(release, stop)).map_err(|error| {
        if matches!(error, Error::Panicked(_)) {
            tracing::warn!(%error, "output release was not confirmed");
            Error::Faulted(SessionPhase::IdlingOutput)
        } else {
            error
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{CallLog, FakeIdle, FakeOutputSettings, FakeOutputs};
    use prysm_core::EdgeColors;
    use prysm_ports::{AdapterSettings, OutputFactory, Registry};

    struct FailedRelease(CallLog);

    impl LedOutput for FailedRelease {
        fn write(&mut self, _: &EdgeColors) -> Result<()> {
            unreachable!("cleanup only")
        }

        fn release(self: Box<Self>, release: HandleRelease, _: &Hangup) -> Result<()> {
            assert_eq!(release, HandleRelease::Local);
            self.0.record("release");
            Err(Error::other("release failed"))
        }
    }

    fn device(log: &CallLog, idle: FakeIdle) -> std::sync::Arc<dyn ConfiguredOutput> {
        let mut registry = Registry::default();
        registry.add_output(FakeOutputs(log.clone()));
        registry
            .output(&AdapterSettings::new(
                FakeOutputs::KIND,
                &FakeOutputSettings {
                    name: "test".into(),
                    idle,
                    ..Default::default()
                },
            ))
            .unwrap()
    }

    const KNOWN_OFF: IdleDecision = IdleDecision {
        mode: OutputRelease::Off,
        known_power: Some(Power::Off),
    };

    #[test]
    fn a_failed_open_invalidates_known_off_only_if_it_may_have_reached_the_device() {
        for (failure, expected_off) in [
            (OpenFailure::Touched(Error::other("timed out")), 1),
            (OpenFailure::Untouched(Error::other("could not connect")), 0),
        ] {
            let log = CallLog::new();
            let device = device(&log, FakeIdle::Idles);
            let mut output = SessionOutput::default();
            let exit = Exit::Failed(output.open_failed(failure));
            output
                .cleanup_after(&exit)
                .finish(&*device, &KNOWN_OFF)
                .unwrap();
            assert_eq!(log.count("output.idle:test:Off"), expected_off);
        }
    }

    #[test]
    fn off_runs_after_a_release_error_and_preserves_the_first_failure() {
        let log = CallLog::new();
        let device = device(&log, FakeIdle::Fails);
        let mut output = SessionOutput::default();
        output.hold(Box::new(FailedRelease(log.clone())));
        let error = output
            .cleanup_after(&Exit::Stopped)
            .finish(&*device, &KNOWN_OFF)
            .unwrap_err();
        assert_eq!(log.events(), ["release", "output.idle:test:Off"]);
        assert_eq!(error.to_string(), "release failed");
        assert_eq!(error.section(), Some(Section::Output));
    }
}
