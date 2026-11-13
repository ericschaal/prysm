//! Scripted adapters for tests and examples. Every call is recorded in a
//! shared [`CallLog`] as `"<adapter>.<call>:<name>"`.
use crate::{EngineHandle, Shutdown};
use prysm_core::{EdgeColors, Frame, PixelFormat};
use prysm_ports::{
    AdapterSettings, Candidate, Error, FrameSink, FrameSource, IdleMode, LedOutput, LightingTarget,
    OutputFactory, OutputIssue, Pull, Result, Settings, SettingsStore, SourceFactory,
    ValidationError,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct CallLog(Arc<Mutex<Vec<String>>>);

impl CallLog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, event: impl Into<String>) {
        let event = event.into();
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }

    #[must_use]
    pub fn events(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.count(event) >= times {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FakeSourceSettings {
    pub name: String,
    /// Fail `next_frame` after this many frames.
    pub fail_after: Option<u32>,
    /// Produce no frames after this many (`Some(0)`: none at all), for
    /// `quiet_for_ms` or, when that is 0, for good.
    pub quiet_after: Option<u32>,
    /// How long the quiet spell lasts; 0: forever.
    pub quiet_for_ms: u64,
    /// Report [`Pull::Ended`] after this many frames, like a file at its end.
    pub end_after: Option<u32>,
    pub fail_open: bool,
    /// Rejected by `validate`.
    pub invalid: bool,
    /// Block in `close` this long, to exceed a stop deadline.
    pub close_delay_ms: u64,
    /// `close` reports an unconfirmed release.
    pub fail_close: bool,
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
    /// Frame size; 0: 16 × 9. A large picture makes processing slow.
    pub width: u32,
    pub height: u32,
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
        if settings.fail_open {
            self.0
                .record(format!("source.open_failed:{}", settings.name));
            return Err(Error::other("fake source cannot open"));
        }
        Ok(Box::new(FakeSource::new(&self.0, settings.clone())))
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
                return Ok(Pull::Idle);
            }
            self.quiet_done = true;
        }
        let interval = match self.settings.frame_interval_ms {
            0 => 2,
            ms => ms,
        };
        thread::sleep(timeout.min(Duration::from_millis(interval)));
        self.frames += 1;
        let fill = match self.settings.blink {
            Some(other) if (self.frames / BLINK_FRAMES) % 2 == 1 => other,
            _ => self.settings.fill,
        };
        let captured = self
            .settings
            .captured_ago_ms
            .and_then(|ago| Instant::now().checked_sub(Duration::from_millis(ago)));
        let (width, height) = match (self.settings.width, self.settings.height) {
            (0, _) | (_, 0) => (16, 9),
            size => size,
        };
        Frame::fill(fill, width, height, PixelFormat::RGB24)
            .map(|frame| Pull::Frame(frame.with_captured(captured)))
            .map_err(|error| Error::adapter("fake frame", error))
    }

    fn close(self: Box<Self>) -> Result<()> {
        thread::sleep(Duration::from_millis(self.settings.close_delay_ms));
        self.log
            .record(format!("source.close:{}", self.settings.name));
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
    pub fail_open: bool,
    /// `idle` fails after recording the call, like an unreachable device.
    pub fail_idle: bool,
    /// Rejected by `validate`.
    pub invalid: bool,
    /// `observe` reports the device overridden after this many writes.
    pub overridden_after: Option<u32>,
    /// Each `write` takes this long, like a slow device.
    pub write_delay_ms: u64,
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

    fn open(&self, settings: &FakeOutputSettings) -> Result<Box<dyn LedOutput>> {
        if settings.fail_open {
            return Err(Error::other("fake output cannot open"));
        }
        self.0.record(format!("output.open:{}", settings.name));
        Ok(Box::new(FakeOutput {
            log: self.0.clone(),
            settings: settings.clone(),
            writes: 0,
        }))
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
            return Err(Error::other("fake output failed"));
        }
        self.writes += 1;
        self.log
            .record(format!("output.write:{}", self.settings.name));
        Ok(())
    }

    fn observe(&mut self) -> Option<OutputIssue> {
        self.settings
            .overridden_after
            .is_some_and(|n| self.writes >= n)
            .then_some(OutputIssue::Overridden)
    }

    fn idle(self: Box<Self>, mode: IdleMode) -> Result<()> {
        self.log
            .record(format!("output.idle:{}:{mode:?}", self.settings.name));
        if self.settings.fail_idle {
            return Err(Error::other("fake output cannot idle"));
        }
        Ok(())
    }
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
    engine.set_target(LightingTarget::Processed).await?;
    shutdown.requested().await;
    Ok(())
}

/// A surface that only waits for shutdown, under an engine or a forwarder.
///
/// # Errors
/// Never.
pub async fn until_shutdown<H>(_handle: H, shutdown: Shutdown) -> Result<()> {
    shutdown.requested().await;
    Ok(())
}
