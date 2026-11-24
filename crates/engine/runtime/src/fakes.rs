//! Scripted adapters for tests and examples. Every call is recorded in a
//! shared [`CallLog`] as `"<adapter>.<call>:<name>"`.
use crate::{EngineHandle, Shutdown};
use prysm_core::{EdgeColors, Frame, PixelFormat};
use prysm_ports::{
    AdapterSettings, Candidate, DeviceBrightness, DeviceReport, Error, FrameSink, FrameSource,
    Hangup, IdleMode, LedOutput, LightingTarget, Mode, Modes, NoPicture, OpenFailure,
    OutputFactory, OutputIssue, Pull, Result, Settings, SettingsStore, SourceFactory, Span,
    Stopping, ValidationError,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// The calls recorded so far, in order; waiters wake at each new one.
#[derive(Debug, Clone)]
pub struct CallLog(Arc<watch::Sender<Vec<String>>>);

impl Default for CallLog {
    fn default() -> Self {
        Self(Arc::new(watch::Sender::new(Vec::new())))
    }
}

impl CallLog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Block this thread, which is not the async runtime's, until `event`
    /// was recorded.
    pub fn block_until(&self, event: &str) {
        let mut calls = self.0.subscribe();
        let _ =
            futures::executor::block_on(calls.wait_for(|events| events.iter().any(|e| e == event)));
    }

    pub fn record(&self, event: impl Into<String>) {
        let event = event.into();
        self.0.send_modify(|events| events.push(event));
    }

    #[must_use]
    pub fn events(&self) -> Vec<String> {
        self.0.borrow().clone()
    }

    #[must_use]
    pub fn count(&self, event: &str) -> usize {
        self.events().iter().filter(|e| *e == event).count()
    }

    #[must_use]
    pub fn position(&self, event: &str) -> Option<usize> {
        self.events().iter().position(|e| e == event)
    }

    #[must_use]
    pub fn last_position(&self, event: &str) -> Option<usize> {
        self.events().iter().rposition(|e| e == event)
    }

    /// Wait up to five seconds for `event` to have happened `times` times.
    pub async fn wait_for(&self, event: &str, times: usize) -> bool {
        self.wait_until(|events| events.iter().filter(|e| *e == event).count() >= times)
            .await
    }

    /// Wait up to five seconds, woken at each call, for `done` to hold for
    /// the calls so far; whether it did. The five seconds are tokio's: under
    /// a paused clock they pass as soon as the runtime idles while a fake
    /// works on its own thread, so wait with the clock running.
    pub async fn wait_until(&self, done: impl Fn(&[String]) -> bool) -> bool {
        let mut calls = self.0.subscribe();
        let happened = calls.wait_for(|events| done(events));
        tokio::time::timeout(Duration::from_secs(5), happened)
            .await
            .is_ok()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FakeSourceSettings {
    pub name: String,
    /// Fail `next_frame` after this many frames.
    pub fail_after: Option<u32>,
    /// With `fail_after`: record `source.failing:<name>`, then wait for this
    /// event in the log before failing.
    pub fail_at: Option<String>,
    /// Produce no frames after this many (`Some(0)`: none at all), for
    /// `quiet_for_ms` or, when that is 0, for good.
    pub quiet_after: Option<u32>,
    /// How long the quiet spell lasts; 0: forever.
    pub quiet_for_ms: u64,
    /// Why the quiet spell gives no picture.
    pub no_picture: NoPicture,
    /// Report [`Pull::Ended`] after this many frames, like a file at its end.
    pub end_after: Option<u32>,
    pub fail_open: bool,
    /// Block in `open` this long, like a device that does not answer.
    pub open_delay_ms: u64,
    /// Block in `modes`, after recording the call, until the log holds this
    /// event: a gate the test opens with [`CallLog::record`].
    pub modes_until: Option<String>,
    /// Rejected by `validate`.
    pub invalid: bool,
    /// Block in `close` this long, to exceed a stop deadline.
    pub close_delay_ms: u64,
    /// `close` reports an unconfirmed release.
    pub fail_close: bool,
    /// `close` panics, like an adapter with a broken invariant.
    pub panic_close: bool,
    /// Byte every frame pixel channel is filled with; 0 (black) by default.
    /// A gray frame lets tests observe the color correction in the preview.
    pub fill: u8,
    /// Switch between `fill` and this every [`BLINK_FRAMES`] frames, so tests
    /// can observe smoothing in the preview.
    pub blink: Option<u8>,
    /// Frames carry a capture time this long before the source hands them over.
    pub captured_ago_ms: Option<u64>,
    /// Hand over a frame this often; 0: every 2 ms.
    pub frame_interval_ms: u64,
    /// Hand over frames without waiting between them, so a test of a
    /// saturated loop does not depend on how punctual a short sleep is.
    pub immediate: bool,
    /// Frame size; 0: 16 × 9. A large picture makes processing slow.
    pub width: u32,
    pub height: u32,
}

impl FakeSourceSettings {
    /// `width` × `height`, or 16 × 9 when either is 0.
    fn frame_size(&self) -> (u32, u32) {
        match (self.width, self.height) {
            (0, _) | (_, 0) => (16, 9),
            size => size,
        }
    }
}

/// Frames per `blink` phase of a [`FakeSource`].
pub const BLINK_FRAMES: u32 = 25;

/// Registers kind `"fake"`.
pub struct FakeSources(pub CallLog);

impl SourceFactory for FakeSources {
    const KIND: &'static str = "fake";
    type Settings = FakeSourceSettings;

    fn validate(&self, settings: &FakeSourceSettings) -> Result<(), ValidationError> {
        if settings.invalid {
            return Err(ValidationError::new("invalid", "fake rejects it"));
        }
        Ok(())
    }

    fn open(&self, settings: &FakeSourceSettings) -> Result<Box<dyn FrameSource>> {
        if settings.open_delay_ms > 0 {
            self.0.record(format!("source.opening:{}", settings.name));
            thread::sleep(Duration::from_millis(settings.open_delay_ms));
        }
        if settings.fail_open {
            self.0
                .record(format!("source.open_failed:{}", settings.name));
            return Err(Error::other("fake source cannot open"));
        }
        Ok(Box::new(FakeSource::new(&self.0, settings.clone())))
    }

    /// The frame size the settings give, at 1 to 60 fps.
    fn modes(&self, settings: &FakeSourceSettings) -> Modes {
        self.0.record("source.modes");
        if let Some(gate) = &settings.modes_until {
            self.0.block_until(gate);
        }
        let (width, height) = settings.frame_size();
        Modes::from(Mode::new(
            Span::exactly(width),
            Span::exactly(height),
            Span::new(1, 60, 1).expect("a span"),
        ))
    }
}

/// Produces a small black frame every two milliseconds.
pub struct FakeSource {
    log: CallLog,
    settings: FakeSourceSettings,
    frames: u32,
    /// When the quiet spell began; `None` before it, or after it ended.
    quiet_since: Option<Instant>,
    quiet_done: bool,
}

impl FakeSource {
    #[must_use]
    pub fn new(log: &CallLog, settings: FakeSourceSettings) -> Self {
        log.record(format!("source.open:{}", settings.name));
        Self {
            log: log.clone(),
            settings,
            frames: 0,
            quiet_since: None,
            quiet_done: false,
        }
    }
}

impl FrameSource for FakeSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        if self.settings.fail_after.is_some_and(|n| self.frames >= n) {
            if let Some(gate) = &self.settings.fail_at {
                self.log
                    .record(format!("source.failing:{}", self.settings.name));
                self.log.block_until(gate);
            }
            return Err(Error::other("fake source failed"));
        }
        if self.settings.end_after.is_some_and(|n| self.frames >= n) {
            return Ok(Pull::Ended);
        }
        if !self.quiet_done && self.settings.quiet_after.is_some_and(|n| self.frames >= n) {
            let since = *self.quiet_since.get_or_insert_with(Instant::now);
            let length = Duration::from_millis(self.settings.quiet_for_ms);
            if self.settings.quiet_for_ms == 0 || since.elapsed() < length {
                thread::sleep(timeout);
                return Ok(Pull::NoPicture(self.settings.no_picture));
            }
            self.quiet_done = true;
        }
        let interval = match self.settings.frame_interval_ms {
            0 => 2,
            ms => ms,
        };
        if !self.settings.immediate {
            thread::sleep(timeout.min(Duration::from_millis(interval)));
        }
        self.frames += 1;
        let fill = match self.settings.blink {
            Some(other) if (self.frames / BLINK_FRAMES) % 2 == 1 => other,
            _ => self.settings.fill,
        };
        let captured = self
            .settings
            .captured_ago_ms
            .and_then(|ago| Instant::now().checked_sub(Duration::from_millis(ago)));
        let (width, height) = self.settings.frame_size();
        Frame::fill(fill, width, height, PixelFormat::RGB24)
            .map(|frame| Pull::Frame(frame.with_captured(captured)))
            .map_err(|error| Error::adapter("fake frame", error))
    }

    fn close(self: Box<Self>) -> Result<()> {
        thread::sleep(Duration::from_millis(self.settings.close_delay_ms));
        self.log
            .record(format!("source.close:{}", self.settings.name));
        assert!(!self.settings.panic_close, "fake source panics in close");
        if self.settings.fail_close {
            return Err(Error::other("fake source release is unconfirmed"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FakeOutputSettings {
    pub name: String,
    /// Fail `write` after this many writes.
    pub fail_after: Option<u32>,
    /// With `fail_after`: record `output.failing:<name>`, then wait for
    /// this event in the log before failing.
    pub fail_at: Option<String>,
    pub fail_open: bool,
    /// `idle` fails after recording the call, like an unreachable device.
    pub fail_idle: bool,
    /// `idle` panics after recording the call.
    pub panic_idle: bool,
    /// Rejected by `validate`.
    pub invalid: bool,
    /// `report` says the device is overridden after this many writes.
    pub overridden_after: Option<u32>,
    /// `report` gives the device's own brightness at this percent once a
    /// frame was written.
    pub brightness: Option<u8>,
    /// `report` gives this LED count once a frame was written.
    pub device_led_count: Option<u32>,
    /// Each `write` takes this long, like a slow device.
    pub write_delay_ms: u64,
    /// `idle` takes this long after recording the call.
    pub idle_delay_ms: u64,
    /// `idle` in `Native` takes this much longer.
    pub native_idle_delay_ms: u64,
    /// `open` records `output.opening:<name>` and waits for the session's
    /// stop to cut it, then ends as this says.
    pub open_until_stop: Option<AtStop>,
    /// The held output's `idle` in `Native` (a release after a quiet
    /// spell), after recording the call, waits for the session's stop to
    /// cut it, then fails.
    pub release_until_stop: bool,
}

/// How a fake open that waits for the stop ends at the cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtStop {
    /// It fails before anything reached the device.
    Untouched,
    /// It fails after a request may have reached it.
    Touched,
    /// It opens: the stop came as it finished.
    Opened,
}

/// Longer than any test waits: a fake waiting for a cut that never comes
/// fails the test by its bound, not by this.
const UNTIL_STOP: Duration = Duration::from_secs(60);

/// The failure of a fake call the stop ended, recorded as
/// `output.<call>_stopped:<name>`.
fn stopped(log: &CallLog, call: &str, name: &str) -> Error {
    log.record(format!("output.{call}_stopped:{name}"));
    Error::adapter(format!("fake output's {call} ended"), Stopping)
}

/// Registers kind `"fake"`.
pub struct FakeOutputs(pub CallLog);

impl OutputFactory for FakeOutputs {
    const KIND: &'static str = "fake";
    type Settings = FakeOutputSettings;

    fn validate(&self, settings: &FakeOutputSettings) -> Result<(), ValidationError> {
        if settings.invalid {
            return Err(ValidationError::new("invalid", "fake rejects it"));
        }
        Ok(())
    }

    /// `fail_open` fails before anything reaches the fake device.
    fn open(
        &self,
        settings: &FakeOutputSettings,
        stop: &Hangup,
    ) -> Result<Box<dyn LedOutput>, OpenFailure> {
        if let Some(at_stop) = settings.open_until_stop {
            self.0.record(format!("output.opening:{}", settings.name));
            let _ = stop.wait(UNTIL_STOP);
            let failed = || stopped(&self.0, "open", &settings.name);
            match at_stop {
                AtStop::Untouched => return Err(OpenFailure::Untouched(failed())),
                AtStop::Touched => return Err(OpenFailure::Touched(failed())),
                AtStop::Opened => {}
            }
        }
        if settings.fail_open {
            self.0
                .record(format!("output.open_failed:{}", settings.name));
            return Err(OpenFailure::Untouched(Error::other(
                "fake output cannot open",
            )));
        }
        self.0.record(format!("output.open:{}", settings.name));
        Ok(Box::new(FakeOutput {
            log: self.0.clone(),
            settings: settings.clone(),
            writes: 0,
        }))
    }

    /// Records `output.idle:<name>:<mode>` as the output's own idle does,
    /// without opening anything.
    fn idle(&self, settings: &FakeOutputSettings, mode: IdleMode) -> Result<()> {
        idle(&self.0, settings, mode)
    }

    /// One fake device, `{ "name": "found" }`.
    fn candidates(&self) -> Vec<Candidate> {
        #[derive(Serialize)]
        struct Found {
            name: &'static str,
        }
        self.0.record("output.candidates");
        vec![Candidate::new("Fake lights", &Found { name: "found" }).detail("fake")]
    }
}

pub struct FakeOutput {
    log: CallLog,
    settings: FakeOutputSettings,
    writes: u32,
}

impl LedOutput for FakeOutput {
    fn write(&mut self, _colors: &EdgeColors) -> Result<()> {
        if self.settings.write_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.settings.write_delay_ms));
        }
        if self.settings.fail_after.is_some_and(|n| self.writes >= n) {
            if let Some(gate) = &self.settings.fail_at {
                self.log
                    .record(format!("output.failing:{}", self.settings.name));
                self.log.block_until(gate);
            }
            return Err(Error::other("fake output failed"));
        }
        self.writes += 1;
        self.log
            .record(format!("output.write:{}", self.settings.name));
        Ok(())
    }

    fn report(&mut self) -> DeviceReport {
        let written = self.writes > 0;
        DeviceReport {
            issue: self
                .settings
                .overridden_after
                .is_some_and(|n| self.writes >= n)
                .then_some(OutputIssue::Overridden),
            brightness: self
                .settings
                .brightness
                .filter(|_| written)
                .and_then(DeviceBrightness::from_percent),
            led_count: self.settings.device_led_count.filter(|_| written),
        }
    }

    fn idle(self: Box<Self>, mode: IdleMode, stop: &Hangup) -> Result<()> {
        if self.settings.release_until_stop && mode == IdleMode::Native {
            self.log
                .record(format!("output.idle:{}:{mode:?}", self.settings.name));
            let _ = stop.wait(UNTIL_STOP);
            return Err(stopped(&self.log, "idle", &self.settings.name));
        }
        idle(&self.log, &self.settings, mode)
    }
}

/// A fake device put in `mode`, held or not.
fn idle(log: &CallLog, settings: &FakeOutputSettings, mode: IdleMode) -> Result<()> {
    log.record(format!("output.idle:{}:{mode:?}", settings.name));
    assert!(!settings.panic_idle, "fake output panics in idle");
    let native = match mode {
        IdleMode::Native => settings.native_idle_delay_ms,
        IdleMode::Off => 0,
    };
    thread::sleep(Duration::from_millis(settings.idle_delay_ms + native));
    if settings.fail_idle {
        return Err(Error::other("fake output cannot idle"));
    }
    Ok(())
}

/// Records `"sink.send"` per frame.
pub struct FakeSink(pub CallLog);

impl FrameSink for FakeSink {
    fn send(&mut self, _frame: &Frame) -> Result<()> {
        self.0.record("sink.send");
        Ok(())
    }
}

/// In-memory settings; clones share the stored value.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore(Arc<Mutex<Option<Settings>>>);

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with(settings: Settings) -> Self {
        Self(Arc::new(Mutex::new(Some(settings))))
    }

    #[must_use]
    pub fn saved(&self) -> Option<Settings> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl SettingsStore for MemoryStore {
    fn load(&mut self) -> Result<Option<Settings>> {
        Ok(self.saved())
    }

    fn save(&mut self, settings: &Settings) -> Result<()> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(settings.clone());
        Ok(())
    }
}

/// Settings selecting the fake source and output with default behaviour.
#[must_use]
pub fn settings() -> Settings {
    Settings {
        capture: Some(AdapterSettings::new(
            FakeSources::KIND,
            &FakeSourceSettings::default(),
        )),
        output: Some(AdapterSettings::new(
            FakeOutputs::KIND,
            &FakeOutputSettings::default(),
        )),
        ..Settings::default()
    }
}

/// A surface that turns lighting on and waits for shutdown.
///
/// # Errors
/// Lighting could not start.
pub async fn light_until_shutdown(engine: EngineHandle, shutdown: Shutdown) -> Result<()> {
    engine.set_target(LightingTarget::ScreenSync).await?;
    shutdown.requested().await;
    Ok(())
}
