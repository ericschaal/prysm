use crate::session_lifecycle::OutputRelease;
use crate::stats::Stats;
use crate::worker::{Exit, guarded};
use prysm_ports::{
    ConfiguredOutput, Error, Escalation, FrameSource, Hangup, IdleMode, LedOutput, OpenFailure,
    OpenIntent, OutputObservation, OutputOpen, OutputRelease as HandleRelease, Power, Result,
    Section, SessionPhase,
};

#[derive(Debug)]
pub(crate) struct IdleDecision {
    pub mode: OutputRelease,
    pub known_power: Option<Power>,
}

impl IdleDecision {
    pub const PRESERVE: Self = Self {
        mode: OutputRelease::Preserve,
        known_power: None,
    };
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

pub(super) struct SessionOutput<'a> {
    device: &'a dyn ConfiguredOutput,
    stats: &'a Stats,
    touched: Option<TouchedOutput>,
}

#[derive(Default)]
struct TouchedOutput {
    handle: Option<Box<dyn LedOutput>>,
}

pub(super) enum Access<'a> {
    Streaming(&'a mut dyn LedOutput),
    Handoff(OutputObservation),
}

impl<'a> SessionOutput<'a> {
    pub fn new(device: &'a dyn ConfiguredOutput, stats: &'a Stats) -> Self {
        Self {
            device,
            stats,
            touched: None,
        }
    }

    pub fn open_authorized(&mut self, intent: OpenIntent, stop: &Hangup) -> Result<Access<'_>> {
        if stop.is_cut() {
            return Err(Error::adapter(
                "output open cancelled",
                prysm_ports::Stopping,
            ));
        }
        self.open(intent, stop)
    }

    pub fn streaming(&mut self, stop: &Hangup) -> Result<Access<'_>> {
        match self.touched {
            Some(TouchedOutput {
                handle: Some(ref mut handle),
            }) => Ok(Access::Streaming(handle.as_mut())),
            _ => self.open(OpenIntent::Resume, stop),
        }
    }

    fn open(&mut self, intent: OpenIntent, stop: &Hangup) -> Result<Access<'_>> {
        match self.device.open(intent, stop) {
            Ok(OutputOpen::Streaming(handle)) => {
                let held = self.touched.get_or_insert_default();
                let handle = held.handle.insert(handle);
                self.stats.output_opened();
                Ok(Access::Streaming(handle.as_mut()))
            }
            Ok(OutputOpen::Handoff(observation)) => Ok(Access::Handoff(observation)),
            Err(failure) => Err(self.open_failed(failure)),
        }
    }

    fn open_failed(&mut self, failure: OpenFailure) -> Error {
        if matches!(failure, OpenFailure::Touched(_)) {
            self.touched.get_or_insert_default();
        }
        failure.into_error().in_section(Section::Output)
    }

    pub fn held(&mut self) -> Option<&mut (dyn LedOutput + 'static)> {
        self.touched.as_mut()?.handle.as_deref_mut()
    }

    pub fn release_quiet(&mut self, stop: &Hangup) -> Result<()> {
        let released = self.release(HandleRelease::Ordinary(IdleMode::Native), stop);
        if let Err(error) = released
            && error.escalation() != Escalation::Unanswered
        {
            return Err(error.in_section(Section::Output));
        }
        Ok(())
    }

    fn release(&mut self, mode: HandleRelease, stop: &Hangup) -> Result<()> {
        let Some(handle) = self
            .touched
            .as_mut()
            .and_then(|output| output.handle.take())
        else {
            return Ok(());
        };
        match guarded("releasing the output", || handle.release(mode, stop)) {
            Err(error @ Error::Panicked(_)) => {
                tracing::warn!(%error, "output release was not confirmed");
                Err(Error::Faulted(SessionPhase::IdlingOutput))
            }
            result => result,
        }
    }

    pub fn cleanup_after(self, exit: &Exit) -> OutputCleanup<'a> {
        OutputCleanup {
            output: self,
            release_unconfirmed: matches!(
                exit,
                Exit::Failed(Error::Faulted(SessionPhase::IdlingOutput))
            ),
        }
    }

    fn turn_off(&mut self, known_power: Option<Power>, stop: &Hangup) -> Result<()> {
        if self.touched.is_none() && known_power == Some(Power::Off) {
            return Ok(());
        }
        let released = self.release(HandleRelease::Local, stop);
        if matches!(released, Err(Error::Faulted(_))) {
            return released;
        }
        let off = self.device.set_power(Power::Off, stop);
        released.and(off)
    }
}

pub(super) struct OutputCleanup<'a> {
    output: SessionOutput<'a>,
    release_unconfirmed: bool,
}

impl OutputCleanup<'_> {
    pub fn finish(self, decision: &IdleDecision) -> Result<()> {
        guarded("idling the output", move || {
            let mut output = self.output;
            if self.release_unconfirmed {
                return Err(Error::Faulted(SessionPhase::IdlingOutput));
            }
            let stop = Hangup::default();
            match decision.mode {
                OutputRelease::Preserve => output.release(HandleRelease::Handoff, &stop),
                OutputRelease::Automatic => {
                    output.release(HandleRelease::Ordinary(IdleMode::Native), &stop)
                }
                OutputRelease::Off => output.turn_off(decision.known_power, &stop),
            }
        })
        .map_err(|error| error.in_section(Section::Output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{CallLog, FakeIdle, FakeOutputSettings, FakeOutputs};
    use prysm_core::EdgeColors;
    use prysm_ports::{AdapterSettings, OutputFactory, Registry};

    struct FailedRelease(CallLog);

    impl Drop for FailedRelease {
        fn drop(&mut self) {
            self.0.record("drop handle");
        }
    }

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
    fn a_stop_after_permission_is_received_skips_the_adapter_entirely() {
        let log = CallLog::new();
        let device = device(&log, FakeIdle::Idles);
        let stats = Stats::default();
        let mut output = SessionOutput::new(&*device, &stats);
        let stop = Hangup::default();
        let (authorize, reply) = tokio::sync::oneshot::channel();
        authorize.send(OpenIntent::Explicit).unwrap();
        let permission = reply.blocking_recv().unwrap();
        stop.cut();
        let Err(cancelled) = output.open_authorized(permission, &stop) else {
            panic!("a cut must prevent opening the adapter");
        };
        assert!(cancelled.cause::<prysm_ports::Stopping>().is_some());
        output
            .cleanup_after(&Exit::Stopped)
            .finish(&KNOWN_OFF)
            .unwrap();
        assert!(log.events().is_empty(), "{:?}", log.events());
    }

    #[test]
    fn a_failed_open_invalidates_known_off_only_if_it_may_have_reached_the_device() {
        for (failure, expected_off) in [
            (OpenFailure::Touched(Error::other("timed out")), 1),
            (OpenFailure::Untouched(Error::other("could not connect")), 0),
        ] {
            let log = CallLog::new();
            let device = device(&log, FakeIdle::Idles);
            let stats = Stats::default();
            let mut output = SessionOutput::new(&*device, &stats);
            let exit = Exit::Failed(output.open_failed(failure));
            output.cleanup_after(&exit).finish(&KNOWN_OFF).unwrap();
            assert_eq!(log.count("output.idle:test:Off"), expected_off);
        }
    }

    #[test]
    fn unconfirmed_release_holds_the_handle_until_finish_and_skips_off() {
        let log = CallLog::new();
        let device = device(&log, FakeIdle::Idles);
        let stats = Stats::default();
        let mut output = SessionOutput::new(&*device, &stats);
        output.touched = Some(TouchedOutput {
            handle: Some(Box::new(FailedRelease(log.clone()))),
        });

        let exit = Exit::Failed(Error::Faulted(SessionPhase::IdlingOutput));
        let cleanup = output.cleanup_after(&exit);
        assert!(log.events().is_empty());
        assert!(matches!(
            cleanup.finish(&KNOWN_OFF),
            Err(Error::Faulted(SessionPhase::IdlingOutput))
        ));
        assert_eq!(log.events(), ["drop handle"]);
    }

    #[test]
    fn off_runs_after_a_release_error_and_preserves_the_first_failure() {
        let log = CallLog::new();
        let device = device(&log, FakeIdle::Fails);
        let stats = Stats::default();
        let mut output = SessionOutput::new(&*device, &stats);
        output.touched = Some(TouchedOutput {
            handle: Some(Box::new(FailedRelease(log.clone()))),
        });
        let error = output
            .cleanup_after(&Exit::Stopped)
            .finish(&KNOWN_OFF)
            .unwrap_err();
        assert_eq!(
            log.events(),
            ["release", "drop handle", "output.idle:test:Off"]
        );
        assert_eq!(error.to_string(), "release failed");
        assert_eq!(error.section(), Some(Section::Output));
    }
}
