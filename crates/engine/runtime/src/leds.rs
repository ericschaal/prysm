//! The session's hold on its output: opened at the first frame, released
//! after a quiet spell, left in the stop's idle mode at the end.
use crate::stats::Stats;
use prysm_core::EdgeColors;
use prysm_ports::{
    AdapterSettings, DeviceReport, IdleMode, LedOutput, OpenFailure, Registry, Result,
};
use std::time::Instant;

/// The session's output as configured. Opened on the session thread at the
/// first frame and at the first frame after each release; put in an idle
/// mode without being opened at the end, when that mode differs from the
/// one the output was left in (see [`Registry::idle_output`]).
pub(crate) struct Configured {
    registry: Registry,
    settings: AdapterSettings,
}

impl Configured {
    pub(crate) fn new(registry: Registry, settings: AdapterSettings) -> Self {
        Self { registry, settings }
    }

    fn open(&self) -> Result<Box<dyn LedOutput>, OpenFailure> {
        self.registry.open_output(&self.settings)
    }

    fn idle(&self, mode: IdleMode) -> Result<()> {
        self.registry.idle_output(&self.settings, mode)
    }
}

pub(crate) enum Leds {
    /// Never opened by this session.
    Untouched,
    Held(Box<dyn LedOutput>),
    /// Idled in this mode and closed.
    Left(IdleMode),
    /// The device may be in any mode: a release failed, or an open that
    /// may have reached it did.
    Unknown,
}

impl Leds {
    /// Write `colors`, opening the output first if it is not held. The write
    /// starts at `ready` (or when the output finished opening) and its time
    /// and outcome are counted. Returns when the write returned.
    pub(crate) fn write(
        &mut self,
        colors: &EdgeColors,
        ready: Instant,
        output: &Configured,
        stats: &Stats,
    ) -> Result<Instant> {
        let began = if self.is_held() {
            ready
        } else {
            *self = Self::Held(match output.open() {
                Ok(held) => held,
                // Nothing reached the device: it is as this state says.
                Err(OpenFailure::Untouched(error)) => return Err(error),
                Err(OpenFailure::Touched(error)) => {
                    *self = Self::Unknown;
                    return Err(error);
                }
            });
            stats.output_opened();
            Instant::now()
        };
        let Self::Held(held) = self else {
            unreachable!("held above")
        };
        let wrote = held.write(colors);
        let written = Instant::now();
        stats.output_write(written - began, wrote.is_ok());
        wrote.map(|()| written)
    }

    pub(crate) fn is_held(&self) -> bool {
        matches!(self, Self::Held(_))
    }

    /// What a held output last learned about its device.
    pub(crate) fn report(&mut self) -> DeviceReport {
        match self {
            Self::Held(output) => output.report(),
            Self::Untouched | Self::Left(_) | Self::Unknown => DeviceReport::default(),
        }
    }

    /// Idle a held output in `mode` and close it.
    pub(crate) fn release(&mut self, mode: IdleMode) -> Result<()> {
        let Self::Held(output) = std::mem::replace(self, Self::Unknown) else {
            return Ok(());
        };
        output.idle(mode)?;
        *self = Self::Left(mode);
        Ok(())
    }

    /// Leave the output in `mode`: idle it if held; otherwise put it in
    /// `mode` without opening it, only when it may be in another mode. An
    /// untouched output stays untouched for `Off`, the state lighting
    /// started from. Afterwards the output is `Left(mode)`, or `Unknown` if
    /// that failed, so it can be finished again in another mode.
    pub(crate) fn finish(&mut self, mode: IdleMode, output: &Configured) -> Result<()> {
        match self {
            Self::Held(_) => self.release(mode),
            Self::Left(left) if *left == mode => Ok(()),
            Self::Untouched if mode == IdleMode::Off => Ok(()),
            Self::Untouched | Self::Left(_) | Self::Unknown => {
                *self = Self::Unknown;
                output.idle(mode)?;
                *self = Self::Left(mode);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::{Error, LedOutput, OutputFactory, ValidationError};
    use serde::{Deserialize, Serialize};
    use std::sync::{Arc, Mutex};

    type Calls = Arc<Mutex<Vec<String>>>;

    #[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
    struct Fails {
        idle: bool,
        opens: Opens,
    }

    /// How the recorder's open goes.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    enum Opens {
        #[default]
        Yes,
        /// Fails before reaching the device.
        Untouched,
        /// Fails after a request may have reached it.
        Touched,
    }

    /// Records opens, idles of what it opened (`idle:<mode>`), and modes set
    /// without opening (`set:<mode>`).
    struct Recorder(Calls);

    impl Recorder {
        fn idled(calls: &Calls, call: String, fails: Fails) -> Result<()> {
            calls.lock().unwrap().push(call);
            if fails.idle {
                return Err(Error::other("cannot idle"));
            }
            Ok(())
        }
    }

    impl OutputFactory for Recorder {
        const KIND: &'static str = "recorder";
        type Settings = Fails;

        fn validate(&self, _: &Fails) -> Result<(), ValidationError> {
            Ok(())
        }

        fn open(&self, fails: &Fails) -> Result<Box<dyn LedOutput>, OpenFailure> {
            self.0.lock().unwrap().push("open".into());
            let failed = || Error::other("cannot open");
            match fails.opens {
                Opens::Yes => {}
                Opens::Untouched => return Err(OpenFailure::Untouched(failed())),
                Opens::Touched => return Err(OpenFailure::Touched(failed())),
            }
            Ok(Box::new(Output {
                calls: self.0.clone(),
                fails: *fails,
            }))
        }

        fn idle(&self, fails: &Fails, mode: IdleMode) -> Result<()> {
            Self::idled(&self.0, format!("set:{mode:?}"), *fails)
        }
    }

    struct Output {
        calls: Calls,
        fails: Fails,
    }

    impl LedOutput for Output {
        fn write(&mut self, _: &EdgeColors) -> Result<()> {
            Ok(())
        }

        fn idle(self: Box<Self>, mode: IdleMode) -> Result<()> {
            Recorder::idled(&self.calls, format!("idle:{mode:?}"), self.fails)
        }
    }

    /// The state before `finish`.
    #[derive(Debug, Clone, Copy)]
    enum Before {
        Untouched,
        Held,
        LeftOff,
        LeftNative,
        Unknown,
    }

    fn leds(from: Before, output: &Configured) -> Leds {
        match from {
            Before::Untouched => Leds::Untouched,
            Before::Held => Leds::Held(output.open().unwrap()),
            Before::LeftOff => Leds::Left(IdleMode::Off),
            Before::LeftNative => Leds::Left(IdleMode::Native),
            Before::Unknown => Leds::Unknown,
        }
    }

    fn state(leds: &Leds) -> String {
        match leds {
            Leds::Untouched => "untouched".into(),
            Leds::Held(_) => "held".into(),
            Leds::Left(mode) => format!("left {mode:?}"),
            Leds::Unknown => "unknown".into(),
        }
    }

    /// Every state against every mode, with outputs that idle and outputs
    /// that cannot: the calls made and the state left. Only a held output is
    /// idled through itself; nothing is opened to idle.
    #[test]
    fn finish_leaves_every_state_in_the_asked_mode() {
        use IdleMode::{Native, Off};
        #[rustfmt::skip]
        let cases: [(Before, IdleMode, bool, &[&str], &str); 20] = [
            (Before::Untouched, Off, false, &[], "untouched"),
            (Before::Untouched, Native, false, &["set:Native"], "left Native"),
            (Before::Held, Off, false, &["idle:Off"], "left Off"),
            (Before::Held, Native, false, &["idle:Native"], "left Native"),
            (Before::LeftOff, Off, false, &[], "left Off"),
            (Before::LeftOff, Native, false, &["set:Native"], "left Native"),
            (Before::LeftNative, Off, false, &["set:Off"], "left Off"),
            (Before::LeftNative, Native, false, &[], "left Native"),
            (Before::Unknown, Off, false, &["set:Off"], "left Off"),
            (Before::Unknown, Native, false, &["set:Native"], "left Native"),
            (Before::Untouched, Off, true, &[], "untouched"),
            (Before::Untouched, Native, true, &["set:Native"], "unknown"),
            (Before::Held, Off, true, &["idle:Off"], "unknown"),
            (Before::Held, Native, true, &["idle:Native"], "unknown"),
            (Before::LeftOff, Off, true, &[], "left Off"),
            (Before::LeftOff, Native, true, &["set:Native"], "unknown"),
            (Before::LeftNative, Off, true, &["set:Off"], "unknown"),
            (Before::LeftNative, Native, true, &[], "left Native"),
            (Before::Unknown, Off, true, &["set:Off"], "unknown"),
            (Before::Unknown, Native, true, &["set:Native"], "unknown"),
        ];
        for (from, mode, fail_idle, calls, left) in cases {
            let made = Calls::default();
            let mut registry = Registry::default();
            registry.add_output(Recorder(made.clone()));
            let output = Configured::new(
                registry,
                AdapterSettings::new(
                    Recorder::KIND,
                    &Fails {
                        idle: fail_idle,
                        opens: Opens::Yes,
                    },
                ),
            );
            let mut leds = leds(from, &output);
            made.lock().unwrap().clear();
            let finished = leds.finish(mode, &output);
            let case = (from, mode, fail_idle);
            assert_eq!(*made.lock().unwrap(), calls, "{case:?}");
            assert_eq!(state(&leds), left, "{case:?}");
            assert_eq!(finished.is_ok(), left != "unknown", "{case:?}");
        }
    }

    /// A failed open that may have reached the device (WLED's turns it off
    /// again, or may not) leaves its mode unknown, whatever it was left in
    /// before: the end then sets the stop's mode instead of trusting the
    /// old one. One that reached nothing leaves the state as it was.
    #[test]
    fn a_failed_open_says_what_it_left_the_device_in() {
        use IdleMode::{Native, Off};
        #[rustfmt::skip]
        let cases: [(Before, Opens, IdleMode, &str, &[&str]); 6] = [
            (Before::Untouched, Opens::Touched, Off, "unknown", &["set:Off"]),
            (Before::LeftOff, Opens::Touched, Off, "unknown", &["set:Off"]),
            (Before::LeftNative, Opens::Touched, Native, "unknown", &["set:Native"]),
            (Before::Untouched, Opens::Untouched, Off, "untouched", &[]),
            (Before::LeftOff, Opens::Untouched, Off, "left Off", &[]),
            (Before::LeftNative, Opens::Untouched, Native, "left Native", &[]),
        ];
        for (from, opens, mode, after, calls) in cases {
            let made = Calls::default();
            let mut registry = Registry::default();
            registry.add_output(Recorder(made.clone()));
            let output = Configured::new(
                registry,
                AdapterSettings::new(Recorder::KIND, &Fails { idle: false, opens }),
            );
            let mut leds = leds(from, &output);
            let stats = Stats::default();
            let wrote = leds.write(&EdgeColors::default(), Instant::now(), &output, &stats);
            let case = (from, opens);
            assert!(wrote.is_err(), "{case:?}");
            assert_eq!(state(&leds), after, "{case:?}");
            made.lock().unwrap().clear();
            leds.finish(mode, &output).unwrap();
            assert_eq!(*made.lock().unwrap(), calls, "{case:?}");
        }
    }
}
