use crate::session::{ColorsWatch, Feeds, OpenOutput, PreviewWatch, Session, SessionEnd};
use crate::stats::{Counters, Stats, Timing};
use prysm_core::Config;
use prysm_ports::{
    AdapterSettings, Calibration, Candidate, Error, IdleMode, Input, Latency, LatencyFrom,
    LightingTarget, Registry, Result, Revision, Section, SessionPhase, SessionState, Settings,
    SettingsStore, Snapshot, ValidationError,
};
use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, Interval, MissedTickBehavior};

type Reply<T> = oneshot::Sender<Result<T>>;

enum Command {
    SetTarget(LightingTarget, Reply<()>),
    ReplaceSettings {
        expected: Revision,
        settings: Box<Settings>,
        reply: Reply<Revision>,
    },
    SetCalibration(Option<Calibration>, Reply<()>),
    SurfaceFailed(Error),
    Shutdown(Reply<()>),
}

/// A calibration pattern nobody has set again for this long is cleared, so a
/// client that vanished does not leave lighting unsmoothed for good.
pub const CALIBRATION_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Cloneable access to a running engine.
#[derive(Clone)]
pub struct EngineHandle {
    commands: mpsc::Sender<Command>,
    snapshot: watch::Receiver<Snapshot>,
    feeds: Feeds,
    stats: Arc<Stats>,
    /// For [`candidates`](Self::candidates), which needs no engine state.
    registry: Registry,
}

impl EngineHandle {
    /// # Errors
    /// Missing adapter settings, open failure, or a faulted or stopped engine.
    pub async fn set_target(&self, target: LightingTarget) -> Result<()> {
        self.call(|reply| Command::SetTarget(target, reply)).await
    }

    /// Validate, persist, then apply. Returns the new revision once persisted;
    /// a later apply failure is reported in the snapshot instead.
    ///
    /// # Errors
    /// Stale `expected`, invalid settings, a store failure, or a faulted engine.
    pub async fn replace_settings(
        &self,
        expected: Revision,
        settings: Settings,
    ) -> Result<Revision> {
        self.call(|reply| Command::ReplaceSettings {
            expected,
            settings: Box::new(settings),
            reply,
        })
        .await
    }

    /// Show a calibration pattern (`Some`), or end calibration (`None`).
    /// Setting one again, even unchanged, restarts its
    /// [`CALIBRATION_TIMEOUT`]. It is cleared when the target leaves
    /// `Processed` and at shutdown.
    ///
    /// # Errors
    /// A pattern while the target is not `Processed` (field `target`), or a
    /// faulted or stopped engine.
    pub async fn set_calibration(&self, calibration: Option<Calibration>) -> Result<()> {
        self.call(|reply| Command::SetCalibration(calibration, reply))
            .await
    }

    /// What the adapter registered for `kind` in `section` finds to choose
    /// from (devices present, devices on the network). The lookup runs off
    /// the async runtime and bypasses the engine's mailbox: it is not state,
    /// and the adapter bounds it to about two seconds.
    ///
    /// # Errors
    /// A kind not registered in `section` (field `<section>.kind`), or a
    /// panicking lookup.
    pub async fn candidates(&self, section: Section, kind: &str) -> Result<Vec<Candidate>> {
        let (registry, kind) = (self.registry.clone(), kind.to_owned());
        blocking(move || registry.candidates(section, &kind)).await?
    }

    #[must_use]
    pub fn snapshot(&self) -> watch::Receiver<Snapshot> {
        self.snapshot.clone()
    }

    /// Watch the latest frame and colors written by the running session. The
    /// session publishes previews only while at least one watch exists.
    #[must_use]
    pub fn preview(&self) -> PreviewWatch {
        self.feeds.preview.watch()
    }

    /// Watch the colors the running session last wrote to the LEDs. The
    /// session publishes them (a copy of the colors, never the frame) only
    /// while at least one watch exists, independently of preview watches.
    /// Empty while the LEDs show anything else: off, the device's own effects,
    /// or a source without frames.
    #[must_use]
    pub fn colors(&self) -> ColorsWatch {
        self.feeds.colors.watch()
    }

    /// Totals of frames, timings and opens since the process started; a few
    /// atomic loads, without waking the engine.
    #[must_use]
    pub fn stats(&self) -> Counters {
        self.stats.read()
    }

    /// How many preview watches exist.
    #[must_use]
    pub fn preview_viewers(&self) -> usize {
        self.feeds.preview.watchers()
    }

    /// How many colors watches exist.
    #[must_use]
    pub fn colors_viewers(&self) -> usize {
        self.feeds.colors.watchers()
    }

    pub(crate) async fn report_surface_failure(&self, error: Error) {
        let _ = self.commands.send(Command::SurfaceFailed(error)).await;
    }

    async fn call<T>(&self, command: impl FnOnce(Reply<T>) -> Command) -> Result<T> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(command(reply))
            .await
            .map_err(|_| Error::Closed)?;
        result.await.map_err(|_| Error::Closed)?
    }
}

/// The running engine task: the only mailbox, owning settings and at most one
/// session. Call [`Engine::shutdown`] to stop lighting and end the task.
pub struct Engine {
    commands: mpsc::Sender<Command>,
    task: JoinHandle<()>,
}

impl Engine {
    /// Load settings and start the engine task. Loaded settings that no longer
    /// validate are kept and reported in the snapshot.
    ///
    /// # Errors
    /// The store failed to load.
    pub async fn start(
        registry: Registry,
        store: Box<dyn SettingsStore>,
        stop_deadline: Duration,
    ) -> Result<(EngineHandle, Self)> {
        let store: Store = Arc::new(Mutex::new(store));
        let loaded = blocking({
            let store = store.clone();
            move || lock(&store).load()
        })
        .await??
        .unwrap_or_default();
        let failure = registry.validate(&loaded).err();
        let (state, snapshot) = watch::channel(Snapshot {
            settings: loaded,
            revision: first_revision(),
            failure,
            ..Snapshot::default()
        });
        let feeds = Feeds::default();
        let stats = Arc::new(Stats::default());
        let (commands, inbox) = mpsc::channel(16);
        let core = Core {
            registry: registry.clone(),
            store,
            running: None,
            calibration_expires: None,
            feeds: feeds.clone(),
            stats: stats.clone(),
            stop_deadline,
            state,
        };
        let task = tokio::spawn(core.run(inbox));
        let handle = EngineHandle {
            commands: commands.clone(),
            snapshot,
            feeds,
            stats,
            registry,
        };
        Ok((handle, Self { commands, task }))
    }

    /// Stop the session (output idles `Off`), join it and end the task.
    ///
    /// # Errors
    /// The engine is faulted, or stopping the session failed.
    pub async fn shutdown(self) -> Result<()> {
        let (reply, result) = oneshot::channel();
        let sent = self.commands.send(Command::Shutdown(reply)).await;
        let result = match sent {
            Ok(()) => result.await.unwrap_or(Err(Error::Closed)),
            Err(_) => Err(Error::Closed),
        };
        let _ = self.task.await;
        result
    }
}

/// A random start per process, so a revision a client kept from before a
/// restart is stale rather than matching. Below 2^52: browsers read revisions
/// as exact JSON numbers, and `Revision::next` has 2^52 steps before either limit.
fn first_revision() -> Revision {
    Revision(RandomState::new().hash_one(0_u8) >> 12)
}

/// A source whose release is unconfirmed may still hold its device, so no new
/// session can be trusted to open it: only a process restart recovers.
const UNRELEASED: SessionState = SessionState::Faulted(SessionPhase::ClosingSource);

type Store = Arc<Mutex<Box<dyn SettingsStore>>>;

fn lock(store: &Store) -> std::sync::MutexGuard<'_, Box<dyn SettingsStore>> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Run blocking adapter code (open, idle, store I/O) off the async runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Error::Panicked("blocking adapter call"))
}

/// How often [`Snapshot::input`] is measured and republished.
const MEASURE: Duration = Duration::from_secs(1);

/// Frame-rate measurement of the running session: the session thread only
/// counts ([`Stats`]); the engine samples the count every [`MEASURE`].
struct Meter {
    interval: Interval,
    /// The totals at the last sample.
    last: Counters,
    last_captured: Timing,
    at: Instant,
}

impl Meter {
    /// Measure from the totals when the session starts.
    fn new(stats: &Stats) -> Self {
        let mut interval = tokio::time::interval_at(Instant::now() + MEASURE, MEASURE);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self {
            interval,
            last: stats.read(),
            last_captured: stats.captured(),
            at: Instant::now(),
        }
    }

    /// The rates and the mean frame span since the last sample, to one
    /// decimal. The span starts at capture when every frame flushed since
    /// had a capture time, else at receipt.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "per-second frame counts and spans are far below f64 precision and f32 range"
    )]
    fn sample(&mut self, stats: &Stats) -> Input {
        let (counters, captured) = (stats.read(), stats.captured());
        let now = Instant::now();
        let elapsed = now.duration_since(self.at).as_secs_f64();
        let received = counters
            .frames_received
            .saturating_sub(self.last.frames_received);
        let sent = counters.frames_sent.saturating_sub(self.last.frames_sent);
        let flushed = counters
            .flushed
            .count
            .saturating_sub(self.last.flushed.count);
        let (span, from) =
            if flushed > 0 && captured.count.saturating_sub(self.last_captured.count) == flushed {
                (
                    captured.total.saturating_sub(self.last_captured.total),
                    LatencyFrom::Capture,
                )
            } else {
                (
                    counters
                        .flushed
                        .total
                        .saturating_sub(self.last.flushed.total),
                    LatencyFrom::Received,
                )
            };
        self.last = counters;
        self.last_captured = captured;
        self.at = now;
        let tenth = |value: f64| ((value * 10.0).round() / 10.0) as f32;
        let rate = |counted: u64| {
            if elapsed > 0.0 {
                tenth(counted as f64 / elapsed)
            } else {
                0.0
            }
        };
        Input {
            fps: rate(received),
            receiving: received > 0,
            sent_fps: rate(sent),
            latency: (flushed > 0).then(|| Latency {
                ms: tenth(span.as_secs_f64() * 1000.0 / flushed as f64),
                from,
            }),
        }
    }
}

/// The session and the measurement of its frame rate.
struct Running {
    session: Session,
    meter: Meter,
}

/// What a running session needs from the engine.
enum Event {
    Ended(Result<SessionEnd>),
    Measure,
}

impl Running {
    /// The session ended on its own, or it is time to measure. Cancel-safe.
    async fn next(&mut self) -> Event {
        tokio::select! {
            end = self.session.finished() => Event::Ended(end),
            _ = self.meter.interval.tick() => Event::Measure,
        }
    }
}

struct Core {
    registry: Registry,
    store: Store,
    running: Option<Running>,
    /// When the snapshot's calibration is cleared; present exactly while it is.
    calibration_expires: Option<Instant>,
    feeds: Feeds,
    stats: Arc<Stats>,
    stop_deadline: Duration,
    state: watch::Sender<Snapshot>,
}

impl Core {
    async fn run(mut self, mut inbox: mpsc::Receiver<Command>) {
        loop {
            let command = tokio::select! {
                command = inbox.recv() => command,
                event = next(&mut self.running) => {
                    match event {
                        Event::Ended(end) => self.session_ended(end).await,
                        Event::Measure => self.measure(),
                    }
                    continue;
                }
                () = expiry(self.calibration_expires) => {
                    self.calibrate(None);
                    continue;
                }
            };
            match command {
                None => {
                    let _ = self.shutdown().await;
                    return;
                }
                Some(Command::Shutdown(reply)) => {
                    let _ = reply.send(self.shutdown().await);
                    return;
                }
                Some(Command::SurfaceFailed(error)) => {
                    self.state.send_modify(|s| s.surface_failures.push(error));
                }
                Some(Command::SetTarget(target, reply)) => {
                    let _ = reply.send(self.set_target(target).await);
                }
                Some(Command::SetCalibration(calibration, reply)) => {
                    let _ = reply.send(self.set_calibration(calibration));
                }
                Some(Command::ReplaceSettings {
                    expected,
                    settings,
                    reply,
                }) => {
                    let _ = reply.send(self.replace_settings(expected, *settings).await);
                }
            }
        }
    }

    /// Refuse commands once a session is faulted: only a restart recovers.
    ///
    /// # Errors
    /// [`Error::Faulted`] with the phase the session got stuck in.
    fn faulted(&self) -> Result<()> {
        match self.state.borrow().session {
            SessionState::Faulted(phase) => Err(Error::Faulted(phase)),
            _ => Ok(()),
        }
    }

    fn measure(&mut self) {
        let Some(running) = &mut self.running else {
            return;
        };
        let input = Some(running.meter.sample(&self.stats));
        let issue = running.session.output_issue();
        self.state.send_if_modified(|s| {
            let changed = s.input != input || s.output_issue != issue;
            s.input = input;
            s.output_issue = issue;
            changed
        });
    }

    /// The session stopped on its own: a failure, or its source ended. An
    /// ended source is completion: the target becomes `Off`, so lighting does
    /// not restart (a file would replay) until asked again.
    async fn session_ended(&mut self, end: Result<SessionEnd>) {
        if let Some(running) = self.running.take() {
            running.session.join().await;
        }
        let session = match &end {
            Ok(end) if end.released.is_err() => UNRELEASED,
            _ => SessionState::Stopped,
        };
        let ended = end.as_ref().is_ok_and(|end| end.ended);
        let failure = end.and_then(SessionEnd::result).err();
        self.state.send_modify(|s| {
            s.session = session;
            s.input = None;
            s.output_issue = None;
            s.failure = failure;
            if ended {
                s.target = LightingTarget::Off;
            }
        });
        if ended {
            self.calibrate(None);
        }
    }

    async fn set_target(&mut self, target: LightingTarget) -> Result<()> {
        self.faulted()?;
        let current = self.state.borrow().session;
        self.state.send_modify(|s| s.target = target);
        if target != LightingTarget::Processed {
            self.calibrate(None);
        }
        let result = match (target, current) {
            (LightingTarget::Processed, SessionState::Running) => Ok(()),
            (LightingTarget::Processed, _) => self.start().await,
            (LightingTarget::DeviceNative, SessionState::Running) => {
                self.stop(IdleMode::Native).await
            }
            (LightingTarget::DeviceNative, SessionState::Native) => Ok(()),
            (LightingTarget::DeviceNative, _) => self.idle_output(IdleMode::Native).await,
            (LightingTarget::Off, SessionState::Running) => self.stop(IdleMode::Off).await,
            (LightingTarget::Off, SessionState::Native) => self.idle_output(IdleMode::Off).await,
            (LightingTarget::Off, _) => Ok(()),
        };
        self.record(&result);
        result
    }

    async fn replace_settings(
        &mut self,
        expected: Revision,
        settings: Settings,
    ) -> Result<Revision> {
        self.faulted()?;
        let current = self.state.borrow().revision;
        if expected != current {
            return Err(Error::StaleRevision { expected, current });
        }
        self.registry.validate(&settings)?;
        let store = self.store.clone();
        let saved = settings.clone();
        blocking(move || lock(&store).save(&saved)).await??;

        let revision = current.next();
        let (previous, target, session) = {
            let mut previous = None;
            self.state.send_modify(|s| {
                previous = Some(std::mem::replace(&mut s.settings, settings.clone()));
                s.revision = revision;
            });
            let snapshot = self.state.borrow();
            (
                previous.unwrap_or_default(),
                snapshot.target,
                snapshot.session,
            )
        };
        let output_changed = previous.output != settings.output;
        let adapters_changed = output_changed || previous.capture != settings.capture;
        let applied = match session {
            // Processed was requested but failed to start or ended in a
            // failure: the new settings get the next attempt. A start that
            // failed from the device's own effects leaves the session
            // `Native` (the lights keep them); that is the same request.
            // Doing nothing there would clear the failure below while the
            // target still says `Processed`: an unexplained state.
            SessionState::Stopped | SessionState::Native if target == LightingTarget::Processed => {
                self.start().await
            }
            SessionState::Running if adapters_changed => {
                // Same path as Off followed by Processed.
                match self.stop(IdleMode::Off).await {
                    Ok(()) => self.start().await,
                    Err(error) => Err(error),
                }
            }
            SessionState::Running if previous.processing != settings.processing => {
                if let Some(running) = &self.running {
                    running.session.configure(effective(&self.state.borrow()));
                }
                Ok(())
            }
            SessionState::Native if output_changed => self.idle_output(IdleMode::Native).await,
            _ => Ok(()),
        };
        self.record(&applied);
        Ok(revision)
    }

    /// Open the source and start a session. The output is validated here
    /// but opened by the session at its first frame, so a source without
    /// frames never takes the device over.
    async fn start(&mut self) -> Result<()> {
        let (capture, output, config) = {
            let snapshot = self.state.borrow();
            let settings = &snapshot.settings;
            let capture = configured(settings.capture.as_ref(), Section::Capture)?;
            let output = configured(settings.output.as_ref(), Section::Output)?;
            self.registry.validate(settings)?;
            (capture, output, effective(&snapshot))
        };
        let registry = self.registry.clone();
        let source = blocking(move || registry.open_source(&capture))
            .await?
            .map_err(|error| error.in_section(Section::Capture))?;
        self.stats.source_opened();
        let registry = self.registry.clone();
        let open_output: OpenOutput = Box::new(move || registry.open_output(&output));
        let session = match Session::start(
            source,
            open_output,
            config,
            self.feeds.clone(),
            self.stats.clone(),
        ) {
            Ok(session) => session,
            Err((error, source)) => {
                // The session never ran: release the source here instead.
                let released = blocking(move || source.close())
                    .await
                    .and_then(|closed| closed);
                if released.is_err() {
                    self.state.send_modify(|s| s.session = UNRELEASED);
                }
                return Err(error);
            }
        };
        self.running = Some(Running {
            session,
            meter: Meter::new(&self.stats),
        });
        self.state.send_modify(|s| {
            s.session = SessionState::Running;
            s.input = Some(Input::default());
        });
        Ok(())
    }

    async fn stop(&mut self, mode: IdleMode) -> Result<()> {
        let Some(running) = self.running.take() else {
            return Ok(());
        };
        let result = running.session.stop(mode, self.stop_deadline).await;
        let state = match &result {
            Err(Error::Faulted(phase)) => SessionState::Faulted(*phase),
            Ok(end) if end.released.is_err() => UNRELEASED,
            Ok(end) if mode == IdleMode::Native && end.idled.is_ok() => SessionState::Native,
            _ => SessionState::Stopped,
        };
        self.state.send_modify(|s| {
            s.session = state;
            s.input = None;
            s.output_issue = None;
        });
        result.and_then(SessionEnd::result)
    }

    /// DeviceNative has no session: open the output only to put it in `mode`.
    async fn idle_output(&mut self, mode: IdleMode) -> Result<()> {
        let output = configured(
            self.state.borrow().settings.output.as_ref(),
            Section::Output,
        );
        let registry = self.registry.clone();
        let result = match output {
            Ok(output) => blocking(move || registry.open_output(&output)?.idle(mode))
                .await?
                .map_err(|error| error.in_section(Section::Output)),
            Err(error) => Err(error),
        };
        let state = match (&result, mode) {
            (Ok(()), IdleMode::Native) => SessionState::Native,
            _ => SessionState::Stopped,
        };
        self.state.send_modify(|s| s.session = state);
        result
    }

    fn set_calibration(&mut self, calibration: Option<Calibration>) -> Result<()> {
        self.faulted()?;
        if calibration.is_some() && self.state.borrow().target != LightingTarget::Processed {
            return Err(Error::NotProcessed);
        }
        self.calibrate(calibration);
        Ok(())
    }

    /// Publish `calibration`, restart its timeout and give a running session
    /// the processing config it implies.
    fn calibrate(&mut self, calibration: Option<Calibration>) {
        self.calibration_expires = calibration.map(|_| Instant::now() + CALIBRATION_TIMEOUT);
        let before = effective(&self.state.borrow());
        self.state.send_if_modified(|s| {
            let changed = s.calibration != calibration;
            s.calibration = calibration;
            changed
        });
        let after = effective(&self.state.borrow());
        if let Some(running) = &self.running
            && after != before
        {
            running.session.configure(after);
        }
    }

    fn record(&self, result: &Result<()>) {
        let failure = result.as_ref().err().cloned();
        self.state.send_modify(|s| s.failure = failure);
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.calibrate(None);
        self.faulted()?;
        let result = self.stop(IdleMode::Off).await;
        self.record(&result);
        result
    }
}

fn configured(settings: Option<&AdapterSettings>, section: Section) -> Result<AdapterSettings> {
    settings
        .cloned()
        .ok_or_else(|| ValidationError::new(section.as_str(), "not configured").into())
}

/// The processor config: the settings, adjusted while a calibration pattern shows.
fn effective(snapshot: &Snapshot) -> Config {
    let config = snapshot.settings.processing.config();
    match snapshot.calibration {
        Some(calibration) => calibration.adjust(config),
        None => config,
    }
}

async fn expiry(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn next(running: &mut Option<Running>) -> Event {
    match running {
        Some(running) => running.next().await,
        None => std::future::pending().await,
    }
}
