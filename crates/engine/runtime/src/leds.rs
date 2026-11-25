//! The session's hold on its output: opened at the first frame, released
//! after a quiet spell, waited for while the lights do not answer, left in
//! the stop's idle mode at the end.
use crate::stats::Stats;
use prysm_core::EdgeColors;
use prysm_ports::{
    AdapterSettings, DeviceReport, Error, Escalation, Hangup, IdleMode, LedOutput, OpenFailure,
    OutputIssue, Registry, Result,
};
use std::time::{Duration, Instant};

/// How often the request that puts the lights in the chosen mode is tried
/// again while they do not answer it ([`Escalation::Unanswered`]): the
/// session's open for screen sync, and the engine's idle for `Off` or the
/// lights' own effects (`Core::idled` in `engine.rs`). Without end, until
/// they answer, or a command (another target, a settings save, a stop or
/// the shutdown) ends the wait; every other failure of these requests
/// fails at once. The lights come back only by something outside Prysm
/// (power, network) at a time nothing announces, so one try every 5 s
/// reaches them 5 s or less after their return (for the open, at the first
/// frame then). A try costs one request, bounded by the adapter's own
/// limit: for WLED's open 1.5 s when the connect itself times out, up to
/// `OPEN_LIMIT` (3 s) when it connects but no reply comes and the device is
/// then turned off again; for its idle `IDLE_LIMIT` (1.5 s). The next try
/// is due 5 s after the last one returned. The session serves no frames
/// during an open: there is no output to serve them to; the engine serves
/// commands between tries, and those that come during one after it. A
/// quiet source tries nothing, and its quiet spell ends the wait
/// ([`Leds::quiet`]); its next frame tries again. A shutdown makes one
/// last try and waits for nothing.
pub const UNANSWERED_INTERVAL: Duration = Duration::from_secs(5);

/// The report while the lights are waited for: only that they do not
/// answer.
pub(crate) const UNREACHABLE: DeviceReport = DeviceReport {
    issue: Some(OutputIssue::Unreachable),
    brightness: None,
    led_count: None,
};

/// Log a try the lights did not answer: the first of a wait at `warn`,
/// later ones at `debug`.
pub(crate) fn log_unanswered(error: &Error, first: bool) {
    let error = prysm_ports::chain(error);
    if first {
        let every = UNANSWERED_INTERVAL.as_secs();
        tracing::warn!(%error, "the lights do not answer; trying again every {every} s");
    } else {
        tracing::debug!(%error, "the lights still do not answer");
    }
}

/// Log the lights answering after a wait of `waited`.
pub(crate) fn log_answered(waited: Duration) {
    tracing::info!(?waited, "the lights answer again");
}

/// The session's output as configured. Opened on the session thread at the
/// first frame, at the first frame after each release, and while the lights
/// do not answer at the first frame of each try; put in an idle
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

    fn open(&self, stop: &Hangup) -> Result<Box<dyn LedOutput>, OpenFailure> {
        self.registry.open_output(&self.settings, stop)
    }

    fn idle(&self, mode: IdleMode) -> Result<()> {
        self.registry.idle_output(&self.settings, mode)
    }
}

/// Whether the cut of `stop` ended the output call that failed with
/// `error`: the session's stop, not a failure, which the loop sees next;
/// the end then leaves the output in the stop's mode.
fn ended(error: &Error, stop: &Hangup) -> bool {
    let ended = stop.is_cut();
    if ended {
        tracing::debug!(error = %prysm_ports::chain(error), "the stop ended the output's call");
    }
    ended
}

/// What the device of an output the session does not hold shows, as far as
/// the session knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Device {
    /// Never opened by this session.
    Untouched,
    /// Idled in this mode and closed.
    Left(IdleMode),
    /// Any mode: a release failed, or an open that may have reached it did.
    Unknown,
}

pub(crate) enum Leds {
    Held(Box<dyn LedOutput>),
    /// Not held: the next frame opens it.
    Closed(Device),
    /// The lights did not answer an open: frames are not written, and the
    /// first frame at or after `again` tries the open again
    /// ([`UNANSWERED_INTERVAL`]).
    Unanswered {
        device: Device,
        /// The frame whose open the lights did not answer first.
        since: Instant,
        again: Instant,
    },
}

/// What [`Leds::write`] did with a frame's colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Write {
    /// Written; the write returned at this time.
    Written(Instant),
    /// The stop ended the open: nothing written, and not a failure.
    Stopped,
    /// The lights do not answer: nothing written, and not a failure.
    Unanswered,
}

impl Write {
    pub(crate) fn written(self) -> Option<Instant> {
        match self {
            Self::Written(at) => Some(at),
            Self::Stopped | Self::Unanswered => None,
        }
    }
}

/// Write `colors` to `held`, timed from `began` and counted.
fn write_held(
    held: &mut dyn LedOutput,
    colors: &EdgeColors,
    began: Instant,
    stats: &Stats,
) -> Result<Write> {
    let wrote = held.write(colors);
    let at = Instant::now();
    stats.output_write(at - began, wrote.is_ok());
    wrote.map(|()| Write::Written(at))
}

impl Leds {
    /// Write `colors`, opening the output first if it is not held; the
    /// open ends when `stop` is cut, and is then not a failure. While the
    /// lights do not answer, the open is tried only at the first frame at
    /// or after the next try ([`UNANSWERED_INTERVAL`], counted from when
    /// the last try returned). The write starts at `ready` (or when the
    /// output finished opening) and its time and outcome are counted.
    /// Returns when the write returned.
    pub(crate) fn write(
        &mut self,
        colors: &EdgeColors,
        ready: Instant,
        output: &Configured,
        stats: &Stats,
        stop: &Hangup,
    ) -> Result<Write> {
        let (device, since) = match self {
            Self::Held(held) => return write_held(&mut **held, colors, ready, stats),
            Self::Unanswered { again, .. } if ready < *again => return Ok(Write::Unanswered),
            Self::Closed(device) => (*device, None),
            Self::Unanswered { device, since, .. } => (*device, Some(*since)),
        };
        let failure = match output.open(stop) {
            Ok(held) => {
                if let Some(since) = since {
                    log_answered(ready.saturating_duration_since(since));
                }
                stats.output_opened();
                // Held before the first write: the end idles it also when
                // that write panics, as the open may have reached the device.
                *self = Self::Held(held);
                return self.write(colors, Instant::now(), output, stats, stop);
            }
            Err(failure) => failure,
        };
        // A failed open that may have reached the device leaves its mode
        // unknown; one that reached nothing leaves it as it was.
        let device = match failure {
            OpenFailure::Touched(_) => Device::Unknown,
            OpenFailure::Untouched(_) => device,
        };
        let error = failure.into_error();
        let stopped = ended(&error, stop);
        if stopped || error.escalation() != Escalation::Unanswered {
            *self = Self::Closed(device);
            return if stopped {
                Ok(Write::Stopped)
            } else {
                Err(error)
            };
        }
        log_unanswered(&error, since.is_none());
        *self = Self::Unanswered {
            device,
            since: since.unwrap_or(ready),
            again: Instant::now() + UNANSWERED_INTERVAL,
        };
        Ok(Write::Unanswered)
    }

    pub(crate) fn is_held(&self) -> bool {
        matches!(self, Self::Held(_))
    }

    /// Not held and not waiting for the lights: a quiet spell has nothing
    /// to end.
    pub(crate) fn is_closed(&self) -> bool {
        matches!(self, Self::Closed(_))
    }

    /// End a quiet spell: a held output is idled in `mode` and closed, as
    /// [`Leds::release`] says; a wait for the lights ends with nothing
    /// tried, its device as the wait left it. Either way the next frame
    /// opens the output. An idle the lights did not answer
    /// ([`Escalation::Unanswered`]) is no failure, as it is none for an
    /// open: the output is closed with its device `Unknown`, which the
    /// stop's idle then sets. Any other failed idle fails.
    pub(crate) fn quiet(&mut self, mode: IdleMode, stop: &Hangup) -> Result<()> {
        let device = match self {
            Self::Held(_) => {
                return self.release(mode, stop).or_else(|error| {
                    if error.escalation() != Escalation::Unanswered {
                        return Err(error);
                    }
                    let error = prysm_ports::chain(&error);
                    tracing::warn!(%error, "the lights did not answer their release");
                    Ok(())
                });
            }
            Self::Unanswered { device, .. } => *device,
            Self::Closed(_) => return Ok(()),
        };
        *self = Self::Closed(device);
        Ok(())
    }

    /// What a held output last learned about its device; while waiting for
    /// the lights, that they do not answer.
    pub(crate) fn report(&mut self) -> DeviceReport {
        match self {
            Self::Held(output) => output.report(),
            Self::Unanswered { .. } => UNREACHABLE,
            Self::Closed(_) => DeviceReport::default(),
        }
    }

    /// Idle a held output in `mode` and close it; the idle ends when `stop`
    /// is cut, and is then not a failure, the device `Unknown`. Anything
    /// else is left as it is.
    fn release(&mut self, mode: IdleMode, stop: &Hangup) -> Result<()> {
        if !self.is_held() {
            return Ok(());
        }
        let Self::Held(output) = std::mem::replace(self, Self::Closed(Device::Unknown)) else {
            unreachable!("held above")
        };
        if let Err(error) = output.idle(mode, stop) {
            return if ended(&error, stop) {
                Ok(())
            } else {
                Err(error)
            };
        }
        *self = Self::Closed(Device::Left(mode));
        Ok(())
    }

    /// Leave the output in `mode`: idle it if held; otherwise put it in
    /// `mode` without opening it, only when its device may be in another
    /// mode. An untouched device stays untouched for `Off`, the state
    /// lighting started from. A wait for the lights ends here, its device
    /// finished as the wait says it was left. Afterwards the device is
    /// `Left(mode)`, or `Unknown` if that failed, so it can be finished
    /// again in another mode. Nothing cuts it: this is the stop's own idle.
    pub(crate) fn finish(&mut self, mode: IdleMode, output: &Configured) -> Result<()> {
        let device = match self {
            Self::Held(_) => return self.release(mode, &Hangup::default()),
            Self::Closed(device) | Self::Unanswered { device, .. } => *device,
        };
        match device {
            Device::Left(left) if left == mode => {}
            Device::Untouched if mode == IdleMode::Off => {}
            Device::Untouched | Device::Left(_) | Device::Unknown => {
                // Set before the idle, not after: the session catches a
                // panic in it and may finish these leds again in another
                // mode, which must not trust the mode they had before.
                *self = Self::Closed(Device::Unknown);
                output.idle(mode)?;
                *self = Self::Closed(Device::Left(mode));
                return Ok(());
            }
        }
        *self = Self::Closed(device);
        Ok(())
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
        idle: Idles,
        opens: Opens,
    }

    /// How the recorder's idles go.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    enum Idles {
        #[default]
        Yes,
        Fail,
        /// Fail as lights that do not answer.
        Unanswered,
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
        /// Fails before reaching the device, which does not answer.
        Unanswered,
        /// Fails after a request may have reached the device, which then
        /// did not answer.
        TouchedUnanswered,
        /// As `Unanswered`, after [`SLOW`].
        SlowUnanswered,
    }

    /// How long a slow open takes.
    const SLOW: Duration = Duration::from_millis(20);

    /// The recorder's output with these failures, recording into `made`.
    fn configured(made: &Calls, fails: Fails) -> Configured {
        let mut registry = Registry::default();
        registry.add_output(Recorder(made.clone()));
        Configured::new(registry, AdapterSettings::new(Recorder::KIND, &fails))
    }

    fn opening(opens: Opens) -> Fails {
        Fails {
            idle: Idles::Yes,
            opens,
        }
    }

    /// Records opens, idles of what it opened (`idle:<mode>`), and modes set
    /// without opening (`set:<mode>`).
    struct Recorder(Calls);

    impl Recorder {
        fn idled(calls: &Calls, call: String, fails: Fails) -> Result<()> {
            calls.lock().unwrap().push(call);
            match fails.idle {
                Idles::Yes => Ok(()),
                Idles::Fail => Err(Error::other("cannot idle")),
                Idles::Unanswered => Err(Error::other_escalating(
                    "cannot idle",
                    Escalation::Unanswered,
                )),
            }
        }
    }

    impl OutputFactory for Recorder {
        const KIND: &'static str = "recorder";
        type Settings = Fails;

        fn validate(&self, _: &Fails) -> Result<(), ValidationError> {
            Ok(())
        }

        fn open(&self, fails: &Fails, _: &Hangup) -> Result<Box<dyn LedOutput>, OpenFailure> {
            self.0.lock().unwrap().push("open".into());
            let failed = |escalation| Error::other_escalating("cannot open", escalation);
            let (fast, unanswered) = (Escalation::FAIL_FAST, Escalation::Unanswered);
            match fails.opens {
                Opens::Yes => {}
                Opens::Untouched => return Err(OpenFailure::Untouched(failed(fast))),
                Opens::Touched => return Err(OpenFailure::Touched(failed(fast))),
                Opens::Unanswered => return Err(OpenFailure::Untouched(failed(unanswered))),
                Opens::TouchedUnanswered => {
                    return Err(OpenFailure::Touched(failed(unanswered)));
                }
                Opens::SlowUnanswered => {
                    std::thread::sleep(SLOW);
                    return Err(OpenFailure::Untouched(failed(unanswered)));
                }
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

        fn idle(self: Box<Self>, mode: IdleMode, _: &Hangup) -> Result<()> {
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
        /// Waiting for lights it never reached.
        WaitingUntouched,
        /// Waiting for lights an open may have reached.
        WaitingUnknown,
    }

    fn waiting(device: Device) -> Leds {
        Leds::Unanswered {
            device,
            since: Instant::now(),
            again: Instant::now() + UNANSWERED_INTERVAL,
        }
    }

    fn leds(from: Before, output: &Configured) -> Leds {
        match from {
            Before::Untouched => Leds::Closed(Device::Untouched),
            Before::Held => Leds::Held(output.open(&Hangup::default()).unwrap()),
            Before::LeftOff => Leds::Closed(Device::Left(IdleMode::Off)),
            Before::LeftNative => Leds::Closed(Device::Left(IdleMode::Native)),
            Before::Unknown => Leds::Closed(Device::Unknown),
            Before::WaitingUntouched => waiting(Device::Untouched),
            Before::WaitingUnknown => waiting(Device::Unknown),
        }
    }

    /// What a [`Leds`] is, without its output or times.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum View {
        Held,
        Closed(Device),
        Waiting(Device),
    }

    const UNTOUCHED: View = View::Closed(Device::Untouched);
    const UNKNOWN: View = View::Closed(Device::Unknown);
    const LEFT_OFF: View = View::Closed(Device::Left(IdleMode::Off));
    const LEFT_NATIVE: View = View::Closed(Device::Left(IdleMode::Native));

    fn view(leds: &Leds) -> View {
        match leds {
            Leds::Held(_) => View::Held,
            Leds::Closed(device) => View::Closed(*device),
            Leds::Unanswered { device, .. } => View::Waiting(*device),
        }
    }

    /// Every state against every mode, with outputs that idle and outputs
    /// that cannot: the calls made and the state left. Only a held output is
    /// idled through itself; nothing is opened to idle. A wait for the
    /// lights is finished as the device it waited with.
    #[test]
    fn finish_leaves_every_state_in_the_asked_mode() {
        use IdleMode::{Native, Off};
        #[rustfmt::skip]
        let cases: [(Before, IdleMode, bool, &[&str], View); 28] = [
            (Before::WaitingUntouched, Off, false, &[], UNTOUCHED),
            (Before::WaitingUntouched, Native, false, &["set:Native"], LEFT_NATIVE),
            (Before::WaitingUnknown, Off, false, &["set:Off"], LEFT_OFF),
            (Before::WaitingUnknown, Native, false, &["set:Native"], LEFT_NATIVE),
            (Before::WaitingUntouched, Off, true, &[], UNTOUCHED),
            (Before::WaitingUntouched, Native, true, &["set:Native"], UNKNOWN),
            (Before::WaitingUnknown, Off, true, &["set:Off"], UNKNOWN),
            (Before::WaitingUnknown, Native, true, &["set:Native"], UNKNOWN),
            (Before::Untouched, Off, false, &[], UNTOUCHED),
            (Before::Untouched, Native, false, &["set:Native"], LEFT_NATIVE),
            (Before::Held, Off, false, &["idle:Off"], LEFT_OFF),
            (Before::Held, Native, false, &["idle:Native"], LEFT_NATIVE),
            (Before::LeftOff, Off, false, &[], LEFT_OFF),
            (Before::LeftOff, Native, false, &["set:Native"], LEFT_NATIVE),
            (Before::LeftNative, Off, false, &["set:Off"], LEFT_OFF),
            (Before::LeftNative, Native, false, &[], LEFT_NATIVE),
            (Before::Unknown, Off, false, &["set:Off"], LEFT_OFF),
            (Before::Unknown, Native, false, &["set:Native"], LEFT_NATIVE),
            (Before::Untouched, Off, true, &[], UNTOUCHED),
            (Before::Untouched, Native, true, &["set:Native"], UNKNOWN),
            (Before::Held, Off, true, &["idle:Off"], UNKNOWN),
            (Before::Held, Native, true, &["idle:Native"], UNKNOWN),
            (Before::LeftOff, Off, true, &[], LEFT_OFF),
            (Before::LeftOff, Native, true, &["set:Native"], UNKNOWN),
            (Before::LeftNative, Off, true, &["set:Off"], UNKNOWN),
            (Before::LeftNative, Native, true, &[], LEFT_NATIVE),
            (Before::Unknown, Off, true, &["set:Off"], UNKNOWN),
            (Before::Unknown, Native, true, &["set:Native"], UNKNOWN),
        ];
        for (from, mode, fail_idle, calls, left) in cases {
            let made = Calls::default();
            let fails = Fails {
                idle: if fail_idle { Idles::Fail } else { Idles::Yes },
                opens: Opens::Yes,
            };
            let output = configured(&made, fails);
            let mut leds = leds(from, &output);
            made.lock().unwrap().clear();
            let finished = leds.finish(mode, &output);
            let case = (from, mode, fail_idle);
            assert_eq!(*made.lock().unwrap(), calls, "{case:?}");
            assert_eq!(view(&leds), left, "{case:?}");
            assert_eq!(finished.is_ok(), left != UNKNOWN, "{case:?}");
        }
    }

    /// The state before, how the open fails, whether the lights are waited
    /// for, the stop's mode, the state after, and the calls of the finish.
    type FailedOpen = (Before, Opens, bool, IdleMode, View, &'static [&'static str]);

    /// A failed open that may have reached the device (WLED's turns it off
    /// again, or may not) leaves its mode unknown, whatever it was left in
    /// before: the end then sets the stop's mode instead of trusting the
    /// old one. One that reached nothing leaves the state as it was. Lights
    /// that do not answer are waited for, with what the open left: not a
    /// failure (`true`); any other failed open fails at once.
    #[test]
    fn a_failed_open_says_what_it_left_the_device_in() {
        use IdleMode::{Native, Off};
        use Opens::{Touched, TouchedUnanswered, Unanswered, Untouched};
        #[rustfmt::skip]
        let cases: [FailedOpen; 10] = [
            (Before::Untouched, Touched, false, Off, UNKNOWN, &["set:Off"]),
            (Before::LeftOff, Touched, false, Off, UNKNOWN, &["set:Off"]),
            (Before::LeftNative, Touched, false, Native, UNKNOWN, &["set:Native"]),
            (Before::Untouched, Untouched, false, Off, UNTOUCHED, &[]),
            (Before::LeftOff, Untouched, false, Off, LEFT_OFF, &[]),
            (Before::LeftNative, Untouched, false, Native, LEFT_NATIVE, &[]),
            (Before::Untouched, Unanswered, true, Off, View::Waiting(Device::Untouched), &[]),
            (Before::LeftNative, Unanswered, true, Native, View::Waiting(Device::Left(Native)), &[]),
            (Before::Untouched, TouchedUnanswered, true, Off, View::Waiting(Device::Unknown), &["set:Off"]),
            (Before::LeftOff, TouchedUnanswered, true, Off, View::Waiting(Device::Unknown), &["set:Off"]),
        ];
        for (from, opens, waits, mode, after, calls) in cases {
            let made = Calls::default();
            let output = configured(&made, opening(opens));
            let mut leds = leds(from, &output);
            let stats = Stats::default();
            let wrote = leds.write(
                &EdgeColors::default(),
                Instant::now(),
                &output,
                &stats,
                &Hangup::default(),
            );
            let case = (from, opens);
            assert_eq!(wrote.ok(), waits.then_some(Write::Unanswered), "{case:?}");
            assert_eq!(view(&leds), after, "{case:?}");
            made.lock().unwrap().clear();
            leds.finish(mode, &output).unwrap();
            assert_eq!(*made.lock().unwrap(), calls, "{case:?}");
        }
    }

    /// Writes at `ready` times after the last failed try returned (after
    /// `start` without one) against `output`, each step's outcome (`None`:
    /// written), its opens and the issue reported.
    type Step<'a> = (&'a Configured, Duration, Option<Write>, usize, bool);

    fn run(leds: &mut Leds, made: &Calls, start: Instant, steps: &[Step<'_>]) {
        let stats = Stats::default();
        for (n, &(output, after, outcome, opens, unreachable)) in steps.iter().enumerate() {
            made.lock().unwrap().clear();
            let returned = match leds {
                Leds::Unanswered { again, .. } => *again - UNANSWERED_INTERVAL,
                Leds::Held(_) | Leds::Closed(_) => start,
            };
            let wrote = leds
                .write(
                    &EdgeColors::default(),
                    returned + after,
                    output,
                    &stats,
                    &Hangup::default(),
                )
                .unwrap();
            let wrote = (wrote.written().is_none()).then_some(wrote);
            assert_eq!(wrote, outcome, "step {n}");
            assert_eq!(made.lock().unwrap().len(), opens, "step {n}");
            let issue = unreachable.then_some(OutputIssue::Unreachable);
            assert_eq!(leds.report().issue, issue, "step {n}");
        }
    }

    /// Lights that do not answer: no failure and `Unreachable` reported; no
    /// open before the interval, one at it; once they answer the colors are
    /// written and the issue clears.
    #[test]
    fn unanswered_lights_are_tried_again_every_interval_until_they_answer() {
        let made = Calls::default();
        let (silent, answering) = (
            configured(&made, opening(Opens::Unanswered)),
            configured(&made, opening(Opens::Yes)),
        );
        let ms = Duration::from_millis;
        let wait = Some(Write::Unanswered);
        let mut leds = Leds::Closed(Device::Untouched);
        let start = Instant::now();
        #[rustfmt::skip]
        run(&mut leds, &made, start, &[
            (&silent, ms(0), wait, 1, true),
            (&silent, ms(4_999), wait, 0, true),
            (&silent, ms(5_000), wait, 1, true),
            (&answering, ms(4_999), wait, 0, true),
            (&answering, ms(5_000), None, 1, false),
            (&answering, ms(5_001), None, 0, false),
        ]);
        assert!(leds.is_held());
    }

    /// The next try is due the interval after a slow try returned, not
    /// after the frame that made it; the wait still counts from that frame.
    #[test]
    fn the_next_try_counts_from_the_last_return() {
        let made = Calls::default();
        let output = configured(&made, opening(Opens::SlowUnanswered));
        let mut leds = Leds::Closed(Device::Untouched);
        let ready = Instant::now();
        let wrote = leds.write(
            &EdgeColors::default(),
            ready,
            &output,
            &Stats::default(),
            &Hangup::default(),
        );
        let after = Instant::now();
        assert_eq!(wrote.unwrap(), Write::Unanswered);
        let Leds::Unanswered { since, again, .. } = leds else {
            panic!("waiting for the lights");
        };
        assert_eq!(since, ready);
        assert!(again >= ready + SLOW + UNANSWERED_INTERVAL);
        assert!(again <= after + UNANSWERED_INTERVAL);
    }

    /// A quiet spell's end from each state: the calls made, the state left,
    /// and whether it failed. A held output is idled; lights that do not
    /// answer that idle fail nothing, as they fail no open, and leave the
    /// device unknown. A wait for the lights ends with nothing tried. No
    /// report is left: the session shows its own state.
    #[test]
    fn a_quiet_spell_ends_holds_and_waits() {
        use Idles::{Fail, Unanswered, Yes};
        #[rustfmt::skip]
        let cases: [(Before, Idles, &[&str], View, bool); 7] = [
            (Before::Held, Yes, &["idle:Native"], LEFT_NATIVE, true),
            (Before::Held, Unanswered, &["idle:Native"], UNKNOWN, true),
            (Before::Held, Fail, &["idle:Native"], UNKNOWN, false),
            (Before::WaitingUntouched, Yes, &[], UNTOUCHED, true),
            (Before::WaitingUnknown, Yes, &[], UNKNOWN, true),
            (Before::Untouched, Yes, &[], UNTOUCHED, true),
            (Before::LeftOff, Yes, &[], LEFT_OFF, true),
        ];
        for (from, idle, calls, left, fine) in cases {
            let made = Calls::default();
            let fails = Fails {
                idle,
                opens: Opens::Yes,
            };
            let output = configured(&made, fails);
            let mut leds = leds(from, &output);
            made.lock().unwrap().clear();
            let quiet = leds.quiet(IdleMode::Native, &Hangup::default());
            let case = (from, idle);
            assert_eq!(*made.lock().unwrap(), calls, "{case:?}");
            assert_eq!(view(&leds), left, "{case:?}");
            assert_eq!(quiet.is_ok(), fine, "{case:?}");
            assert_eq!(leds.report(), DeviceReport::default(), "{case:?}");
        }
    }

    /// Lights that never answer never fail the session: one open per
    /// interval, for as long as frames come.
    #[test]
    fn unanswered_lights_are_waited_for_without_end() {
        let made = Calls::default();
        let silent = configured(&made, opening(Opens::TouchedUnanswered));
        let wait = Some(Write::Unanswered);
        let steps: Vec<Step<'_>> = (0..1_000)
            .flat_map(|_| {
                [
                    (&silent, UNANSWERED_INTERVAL, wait, 1, true),
                    (&silent, UNANSWERED_INTERVAL / 2, wait, 0, true),
                ]
            })
            .collect();
        let mut leds = Leds::Closed(Device::Untouched);
        run(&mut leds, &made, Instant::now(), &steps);
        assert_eq!(view(&leds), View::Waiting(Device::Unknown));
    }
}
