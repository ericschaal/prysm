//! The session's hold on its output: opened at the first frame, released
//! after a quiet spell, left in the stop's idle mode at the end.
use crate::stats::Stats;
use prysm_core::EdgeColors;
use prysm_ports::{DeviceReport, IdleMode, LedOutput, Result};
use std::time::Instant;

/// Opens the session's output. Called on the session thread at the first
/// frame and at the first frame after each release, and at the end when the
/// final idle mode differs from the one the output was left in.
pub(crate) type OpenOutput = Box<dyn FnMut() -> Result<Box<dyn LedOutput>> + Send>;

pub(crate) enum Leds {
    /// Never opened by this session.
    Untouched,
    Held(Box<dyn LedOutput>),
    /// Idled in this mode and closed.
    Left(IdleMode),
    /// A release failed: the device may be in any mode.
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
        open: &mut OpenOutput,
        stats: &Stats,
    ) -> Result<Instant> {
        let began = if self.is_held() {
            ready
        } else {
            *self = Self::Held(open()?);
            stats.output_opened();
            Instant::now()
        };
        let Self::Held(output) = self else {
            unreachable!("held above")
        };
        let wrote = output.write(colors);
        let written = Instant::now();
        stats.output_write(written - began, wrote.is_ok());
        wrote.map(|()| written)
    }

    pub(crate) fn is_held(&self) -> bool {
        matches!(self, Self::Held(_))
    }

    /// What a held output last learned about its device.
    pub(crate) fn observe(&mut self) -> DeviceReport {
        match self {
            Self::Held(output) => DeviceReport {
                issue: output.observe(),
                brightness: output.brightness(),
                led_count: output.device_led_count(),
            },
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

    /// Leave the output in `mode`: idle it if held; otherwise open it only
    /// when it may be in another mode. An untouched output stays untouched
    /// for `Off`, the state lighting started from. Afterwards the output is
    /// `Left(mode)`, or `Unknown` if that failed, so it can be finished
    /// again in another mode.
    pub(crate) fn finish(&mut self, mode: IdleMode, open: &mut OpenOutput) -> Result<()> {
        match self {
            Self::Held(_) => self.release(mode),
            Self::Left(left) if *left == mode => Ok(()),
            Self::Untouched if mode == IdleMode::Off => Ok(()),
            Self::Untouched | Self::Left(_) | Self::Unknown => {
                *self = Self::Unknown;
                open()?.idle(mode)?;
                *self = Self::Left(mode);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::{Error, LedOutput};
    use std::sync::{Arc, Mutex};

    type Calls = Arc<Mutex<Vec<String>>>;

    struct Output {
        calls: Calls,
        fail_idle: bool,
    }

    impl LedOutput for Output {
        fn write(&mut self, _: &EdgeColors) -> Result<()> {
            Ok(())
        }

        fn idle(self: Box<Self>, mode: IdleMode) -> Result<()> {
            self.calls.lock().unwrap().push(format!("idle:{mode:?}"));
            if self.fail_idle {
                return Err(Error::other("cannot idle"));
            }
            Ok(())
        }
    }

    /// The state before `finish`, built with outputs recording into `calls`.
    #[derive(Debug, Clone, Copy)]
    enum Before {
        Untouched,
        Held,
        LeftOff,
        LeftNative,
        Unknown,
    }

    fn leds(from: Before, calls: &Calls, fail_idle: bool) -> Leds {
        match from {
            Before::Untouched => Leds::Untouched,
            Before::Held => Leds::Held(Box::new(Output {
                calls: calls.clone(),
                fail_idle,
            })),
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
    /// that cannot: the calls made and the state left.
    #[test]
    fn finish_leaves_every_state_in_the_asked_mode() {
        use IdleMode::{Native, Off};
        #[rustfmt::skip]
        let cases: [(Before, IdleMode, bool, &[&str], &str); 20] = [
            (Before::Untouched, Off, false, &[], "untouched"),
            (Before::Untouched, Native, false, &["open", "idle:Native"], "left Native"),
            (Before::Held, Off, false, &["idle:Off"], "left Off"),
            (Before::Held, Native, false, &["idle:Native"], "left Native"),
            (Before::LeftOff, Off, false, &[], "left Off"),
            (Before::LeftOff, Native, false, &["open", "idle:Native"], "left Native"),
            (Before::LeftNative, Off, false, &["open", "idle:Off"], "left Off"),
            (Before::LeftNative, Native, false, &[], "left Native"),
            (Before::Unknown, Off, false, &["open", "idle:Off"], "left Off"),
            (Before::Unknown, Native, false, &["open", "idle:Native"], "left Native"),
            (Before::Untouched, Off, true, &[], "untouched"),
            (Before::Untouched, Native, true, &["open", "idle:Native"], "unknown"),
            (Before::Held, Off, true, &["idle:Off"], "unknown"),
            (Before::Held, Native, true, &["idle:Native"], "unknown"),
            (Before::LeftOff, Off, true, &[], "left Off"),
            (Before::LeftOff, Native, true, &["open", "idle:Native"], "unknown"),
            (Before::LeftNative, Off, true, &["open", "idle:Off"], "unknown"),
            (Before::LeftNative, Native, true, &[], "left Native"),
            (Before::Unknown, Off, true, &["open", "idle:Off"], "unknown"),
            (Before::Unknown, Native, true, &["open", "idle:Native"], "unknown"),
        ];
        for (from, mode, fail_idle, calls, left) in cases {
            let made = Calls::default();
            let mut leds = leds(from, &made, fail_idle);
            let mut open: OpenOutput = Box::new({
                let made = made.clone();
                move || {
                    made.lock().unwrap().push("open".into());
                    Ok(Box::new(Output {
                        calls: made.clone(),
                        fail_idle,
                    }) as Box<dyn LedOutput>)
                }
            });
            let finished = leds.finish(mode, &mut open);
            let case = (from, mode, fail_idle);
            assert_eq!(*made.lock().unwrap(), calls, "{case:?}");
            assert_eq!(state(&leds), left, "{case:?}");
            assert_eq!(finished.is_ok(), left != "unknown", "{case:?}");
        }
    }
}
