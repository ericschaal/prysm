use crate::feeds::{ColorsWatch, Feeds, PreviewWatch};
use crate::leds::OpenOutput;
use crate::meter::Meter;
use crate::session::{Session, SessionEnd};
use crate::stats::{Counters, Stats};
use prysm_core::Config;
use prysm_ports::{
    AdapterSettings, Calibration, Candidate, Error, IdleMode, Input, LightingTarget, Processing,
    Registry, Result, Revision, Section, SessionPhase, SessionState, Settings, SettingsStore,
    Snapshot, ValidationError,
};
use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

type Reply<T> = oneshot::Sender<Result<T>>;

enum Command {
    SetTarget(LightingTarget, Reply<()>),
    ReplaceSettings {
        expected: Revision,
        settings: Box<Settings>,
        reply: Reply<Revision>,
    },
    SetCalibration(Option<Calibration>, Reply<()>),
    TryProcessing(Box<Processing>, Reply<()>),
    SurfaceFailed(Error),
    Shutdown(Reply<()>),
}

/// A calibration pattern nobody has set again for this long is cleared, so a
/// client that vanished does not leave lighting unsmoothed for good.
pub const CALIBRATION_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Tried processing nobody tries again or saves for this long is dropped, so
/// a client that vanished mid-drag does not leave the lights unlike the
/// saved settings.
pub const TRIAL_TIMEOUT: Duration = Duration::from_secs(30);

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

    /// Light with `processing` instead of the saved processing, without
    /// saving it or changing the revision: for a control being dragged, whose
    /// final value is then saved with [`replace_settings`](Self::replace_settings).
    /// A save ends the trial, and so does [`TRIAL_TIMEOUT`] without another
    /// call; the lights then follow the saved settings again. It is not in the
    /// snapshot and applies whatever the target (to a session started meanwhile).
    ///
    /// # Errors
    /// Invalid processing, or a faulted or stopped engine.
    pub async fn try_processing(&self, processing: Processing) -> Result<()> {
        self.call(|reply| Command::TryProcessing(Box::new(processing), reply))
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
            trial: None,
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
            () = self.meter.tick() => Event::Measure,
        }
    }
}

struct Core {
    registry: Registry,
    store: Store,
    running: Option<Running>,
    /// When the snapshot's calibration is cleared; present exactly while it is.
    calibration_expires: Option<Instant>,
    /// Processing being tried instead of the saved one, and when it ends.
    trial: Option<(Processing, Instant)>,
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
                () = expiry(self.trial.as_ref().map(|(_, at)| *at)) => {
                    self.reconfigure(|core| core.trial = None);
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
                Some(Command::TryProcessing(processing, reply)) => {
                    let _ = reply.send(self.try_processing(*processing));
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

    /// What the session is doing. `running` is the one truth for whether a
    /// session runs: [`Self::start`] sets both it and the published
    /// `Running`, and [`Self::stop`] and [`Self::session_ended`] clear both;
    /// otherwise the snapshot says why none runs.
    fn session(&self) -> SessionState {
        let published = self.state.borrow().session;
        debug_assert_eq!(
            self.running.is_some(),
            published == SessionState::Running,
            "running and the published session state disagree"
        );
        published
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
        let current = self.session();
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

        let (target, output_changed, capture_changed) = {
            let snapshot = self.state.borrow();
            (
                snapshot.target,
                snapshot.settings.output != settings.output,
                snapshot.settings.capture != settings.capture,
            )
        };
        let adapters_changed = output_changed || capture_changed;
        let session = self.session();
        let revision = current.next();
        // Saved settings end a trial.
        let install = |core: &mut Self| {
            core.trial = None;
            core.state.send_modify(|s| {
                s.settings = settings;
                s.revision = revision;
            });
        };
        if session == SessionState::Running && !adapters_changed {
            // Same adapters: the running session takes the new processing.
            self.reconfigure(install);
        } else {
            install(self);
        }
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
            (capture, output, self.config())
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

    /// Stop the running session, if one runs (see [`Self::session`]):
    /// without one there is nothing to stop and the state stays as it is.
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

    fn try_processing(&mut self, processing: Processing) -> Result<()> {
        self.faulted()?;
        processing.validate()?;
        self.reconfigure(|core| core.trial = Some((processing, Instant::now() + TRIAL_TIMEOUT)));
        Ok(())
    }

    /// Publish `calibration`, restart its timeout and give a running session
    /// the processing config it implies.
    fn calibrate(&mut self, calibration: Option<Calibration>) {
        self.calibration_expires = calibration.map(|_| Instant::now() + CALIBRATION_TIMEOUT);
        self.reconfigure(|core| {
            core.state.send_if_modified(|s| {
                let changed = s.calibration != calibration;
                s.calibration = calibration;
                changed
            });
        });
    }

    /// Apply `change`, then give a running session the config that follows
    /// if it differs. Without a session there is nothing to compare.
    fn reconfigure(&mut self, change: impl FnOnce(&mut Self)) {
        if self.running.is_none() {
            change(self);
            return;
        }
        let before = self.config();
        change(self);
        let after = self.config();
        if let Some(running) = &self.running
            && after != before
        {
            running.session.configure(after);
        }
    }

    /// The processor config: the saved processing or the one being tried,
    /// adjusted while a calibration pattern shows.
    fn config(&self) -> Config {
        let snapshot = self.state.borrow();
        let processing = self
            .trial
            .as_ref()
            .map_or(&snapshot.settings.processing, |(processing, _)| processing);
        match snapshot.calibration {
            Some(calibration) => calibration.adjust(processing.config()),
            None => processing.config(),
        }
    }

    fn record(&self, result: &Result<()>) {
        let failure = result.as_ref().err().cloned();
        self.state.send_modify(|s| s.failure = failure);
    }

    async fn shutdown(&mut self) -> Result<()> {
        // Calibration ends at shutdown even on a faulted engine: the last
        // snapshot shows no pattern that nothing is showing any more.
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
