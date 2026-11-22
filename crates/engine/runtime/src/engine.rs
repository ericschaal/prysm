use crate::feeds::{ColorsWatch, Feeds, PreviewWatch};
use crate::leds::Configured;
use crate::meter::Meter;
use crate::session::{Session, SessionEnd, Stop};
use crate::stats::{Counters, Stats};
use crate::worker::Exit;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use prysm_core::Config;
use prysm_ports::{
    AdapterSettings, CalibrationPattern, Candidate, DeviceReport, Error, IdleMode, Input,
    LightingTarget, Modes, Processing, Registry, Result, Revision, Section, SessionPhase,
    SessionState, Settings, SettingsStore, Snapshot, ValidationError,
};
use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

type Reply<T> = oneshot::Sender<Result<T>>;

enum Command {
    Request(Request),
    SurfaceFailed(Error),
    Shutdown(Reply<()>),
}

/// What a client asks of the lighting or the settings; refused unhandled
/// once the engine is faulted or shutting down (see [`Core::refusal`]).
enum Request {
    SetTarget(LightingTarget, Reply<()>),
    ReplaceSettings {
        expected: Revision,
        settings: Box<Settings>,
        reply: Reply<Revision>,
    },
    SetCalibration(Option<CalibrationPattern>, Reply<()>),
    TryProcessing(Box<Processing>, Reply<()>),
}

impl Request {
    /// Answer with `error` without acting.
    fn refuse(self, error: Error) {
        match self {
            Self::SetTarget(_, reply)
            | Self::SetCalibration(_, reply)
            | Self::TryProcessing(_, reply) => {
                let _ = reply.send(Err(error));
            }
            Self::ReplaceSettings { reply, .. } => {
                let _ = reply.send(Err(error));
            }
        }
    }
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
    /// For [`candidates`](Self::candidates) and
    /// [`capture_modes`](Self::capture_modes), which need no engine state.
    registry: Registry,
    candidates: Lookups<(Section, String), Vec<Candidate>>,
    modes: Lookups<AdapterSettings, Modes>,
}

impl EngineHandle {
    /// # Errors
    /// Missing adapter settings, open failure, or a faulted or stopped engine.
    pub async fn set_target(&self, target: LightingTarget) -> Result<()> {
        self.call(|reply| Request::SetTarget(target, reply)).await
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
        self.call(|reply| Request::ReplaceSettings {
            expected,
            settings: Box::new(settings),
            reply,
        })
        .await
    }

    /// Show a calibration pattern (`Some`), or end calibration (`None`).
    /// Setting one again, even unchanged, restarts its
    /// [`CALIBRATION_TIMEOUT`]. It is cleared when the target leaves
    /// `ScreenSync` and at shutdown.
    ///
    /// # Errors
    /// [`Error::NeedsScreenSync`] for a pattern while the target is not
    /// `ScreenSync`, or a faulted or stopped engine.
    pub async fn set_calibration(&self, calibration: Option<CalibrationPattern>) -> Result<()> {
        self.call(|reply| Request::SetCalibration(calibration, reply))
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
        self.call(|reply| Request::TryProcessing(Box::new(processing), reply))
            .await
    }

    /// What the adapter registered for `kind` in `section` finds to choose
    /// from (devices present, devices on the network). The lookup runs off
    /// the async runtime and bypasses the engine's mailbox: it is not state,
    /// and the adapter bounds it to about two seconds. One runs per section
    /// and kind at a time: who asks while it runs gets its answer.
    ///
    /// # Errors
    /// A kind not registered in `section` (field `<section>.kind`), or a
    /// panicking lookup.
    pub async fn candidates(&self, section: Section, kind: &str) -> Result<Vec<Candidate>> {
        let (registry, kind) = (self.registry.clone(), kind.to_owned());
        self.candidates
            .run((section, kind.clone()), move || {
                registry.candidates(section, &kind)
            })
            .await
    }

    /// What the capture `settings` select delivers (see
    /// [`SourceFactory::modes`](prysm_ports::SourceFactory::modes)). Like
    /// [`candidates`](Self::candidates), it runs off the async runtime,
    /// bypasses the engine's mailbox, and runs once at a time for the same
    /// settings, whose askers share its answer.
    ///
    /// # Errors
    /// An unregistered kind (field `capture.kind`), undecodable settings, or
    /// a panicking lookup.
    pub async fn capture_modes(&self, settings: AdapterSettings) -> Result<Modes> {
        let registry = self.registry.clone();
        self.modes
            .run(settings.clone(), move || registry.capture_modes(&settings))
            .await
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

    /// Totals of frames, timings and opens since the process started, copied
    /// under a short lock without waking the engine.
    #[must_use]
    pub fn stats(&self) -> Counters {
        self.stats.read()
    }

    /// How many preview watches exist.
    #[cfg(any(test, feature = "testkit"))]
    #[must_use]
    pub fn preview_viewers(&self) -> usize {
        self.feeds.preview.watchers()
    }

    /// How many colors watches exist.
    #[cfg(any(test, feature = "testkit"))]
    #[must_use]
    pub fn colors_viewers(&self) -> usize {
        self.feeds.colors.watchers()
    }

    /// Report a failed surface, once the mailbox has room.
    pub(crate) async fn report_surface_failure(&self, error: Error) {
        let _ = self.commands.send(Command::SurfaceFailed(error)).await;
    }

    async fn call<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Request(request(reply)))
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
    /// When blocking calls are abandoned; set by [`Self::shutdown`].
    abandon_at: watch::Sender<Option<Instant>>,
    shutdown_grace: Duration,
}

impl Engine {
    /// Load settings and start the engine task. Loaded settings that no longer
    /// validate are kept and reported in the snapshot. A store that cannot
    /// be read starts unconfigured, its failure reported until a save
    /// replaces what it holds (see [`SettingsStore::load`]).
    ///
    /// # Errors
    /// The load panicked.
    pub async fn start(
        registry: Registry,
        store: Box<dyn SettingsStore>,
        stop_deadline: Duration,
    ) -> Result<(EngineHandle, Self)> {
        let store: Store = Arc::new(Mutex::new(store));
        let (loaded, unreadable) = match blocking({
            let store = store.clone();
            move || lock(&store).load()
        })
        .await?
        {
            Ok(loaded) => (loaded.unwrap_or_default(), None),
            Err(error) => {
                tracing::warn!(
                    "starting without settings until a save: {}",
                    prysm_ports::chain(&error)
                );
                (
                    Settings::default(),
                    Some(Error::UnusableSettings(Arc::new(error))),
                )
            }
        };
        let failure = unreadable
            .clone()
            .or_else(|| registry.validate(&loaded).err());
        let (state, snapshot) = watch::channel(Snapshot {
            session: SessionState::Stopped { failure },
            settings: loaded,
            revision: first_revision(),
            ..Snapshot::default()
        });
        let feeds = Feeds::default();
        let stats = Arc::new(Stats::default());
        let (commands, inbox) = mpsc::channel(16);
        let (abandon_at, abandon_when) = watch::channel(None);
        let core = Core {
            registry: registry.clone(),
            store,
            unreadable,
            lighting: Lighting::Stopped,
            calibration_expires: None,
            trial: None,
            feeds: feeds.clone(),
            stats: stats.clone(),
            stop_deadline,
            abandon_at: abandon_when,
            abandoned: None,
            state,
        };
        let task = tokio::spawn(core.run(inbox));
        let handle = EngineHandle {
            commands: commands.clone(),
            snapshot,
            feeds,
            stats,
            registry,
            candidates: Lookups::default(),
            modes: Lookups::default(),
        };
        let engine = Self {
            commands,
            task,
            abandon_at,
            shutdown_grace: stop_deadline,
        };
        Ok((handle, engine))
    }

    /// How long [`Self::shutdown`] waits for blocking adapter or settings
    /// calls before abandoning them; the stop deadline unless set.
    #[must_use]
    pub fn shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }

    /// Stop the session (output idles `Off`), join it and end the task.
    /// Requests still queued, and any sent from now on, are refused with
    /// [`Error::Closed`] in their order, so none starts what shutdown stops.
    /// Blocking adapter or settings calls still running when the shutdown
    /// grace has passed since this call (or [`Self::begin_shutdown`]) are
    /// abandoned and left running on their threads; later ones fail at once.
    ///
    /// # Errors
    /// The engine is faulted, stopping the session failed, or a blocking
    /// call was abandoned.
    pub async fn shutdown(self) -> Result<()> {
        self.begin_shutdown();
        let (reply, result) = oneshot::channel();
        let sent = self.commands.send(Command::Shutdown(reply)).await;
        let result = match sent {
            Ok(()) => result.await.unwrap_or(Err(Error::Closed)),
            Err(_) => Err(Error::Closed),
        };
        let _ = self.task.await;
        result
    }

    /// What [`Self::shutdown`] starts with: from the first call, queued
    /// requests are refused and the shutdown grace runs; the engine still
    /// takes commands such as surface failures until the shutdown itself.
    pub(crate) fn begin_shutdown(&self) {
        let at = Instant::now() + self.shutdown_grace;
        self.abandon_at.send_if_modified(|abandon_at| {
            let first = abandon_at.is_none();
            if first {
                *abandon_at = Some(at);
            }
            first
        });
    }
}

/// A lookup's answer, for every asker that waits for it.
type Answer<T> = Shared<BoxFuture<'static, Result<T>>>;

/// The blocking lookups running, by what they look up. A lookup runs once
/// at a time per key: who asks while it runs waits for its answer. It keeps
/// its key until its blocking call returns, also when nobody waits any
/// more, so no client can stack up blocking threads for the same lookup.
struct Lookups<K, T>(Arc<Mutex<InFlight<K, T>>>);

/// Each running lookup's key and answer.
type InFlight<K, T> = Vec<(K, Answer<T>)>;

impl<K, T> Default for Lookups<K, T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<K, T> Clone for Lookups<K, T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K, T> Lookups<K, T>
where
    K: Clone + PartialEq + Send + 'static,
    T: Clone + Send + Sync + 'static,
{
    /// The answer of the lookup running for `key`, or of `work` started for
    /// it now on a blocking thread.
    fn run(&self, key: K, work: impl FnOnce() -> Result<T> + Send + 'static) -> Answer<T> {
        let mut running = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, answer)) = running.iter().find(|(running, _)| *running == key) {
            return answer.clone();
        }
        let ended = Ended {
            lookups: self.clone(),
            key: key.clone(),
        };
        // Started by its first poll, outside the lock, then on its thread
        // to its end whoever still waits.
        let answer = async move {
            tokio::task::spawn_blocking(move || {
                let answer = work();
                drop(ended);
                answer
            })
            .await
            .map_err(|_| Error::Panicked("blocking adapter call"))?
        }
        .boxed()
        .shared();
        running.push((key, answer.clone()));
        answer
    }
}

/// Removes a lookup that returned, on its blocking thread: the next ask
/// starts a new one.
struct Ended<K: PartialEq, T> {
    lookups: Lookups<K, T>,
    key: K,
}

impl<K: PartialEq, T> Drop for Ended<K, T> {
    fn drop(&mut self) {
        self.lookups
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(running, _)| *running != self.key);
    }
}

/// A random start per process, so a revision a client kept from before a
/// restart is stale rather than matching. Below 2^52: browsers read revisions
/// as exact JSON numbers, and `Revision::next` has 2^52 steps before either limit.
fn first_revision() -> Revision {
    Revision(RandomState::new().hash_one(0_u8) >> 12)
}

/// What the lights are doing. The one truth for whether a session runs:
/// [`Core::set_lighting`] publishes it.
#[expect(
    clippy::large_enum_variant,
    reason = "the engine holds exactly one, never moved per frame"
)]
enum Lighting {
    /// No session; the output (if any was touched) is idle.
    Stopped,
    /// The output was put in its own effects, without a session.
    Effects,
    ScreenSync(Running),
    /// Only shutdown is accepted, and only a process restart recovers.
    Faulted(SessionPhase),
}

impl Lighting {
    /// The published state, carrying `failure` where it can: screen sync
    /// runs on valid settings and has reached its target, and a fault says
    /// only its phase, so neither carries one.
    fn state(&self, failure: Option<Error>) -> SessionState {
        match self {
            Self::Stopped => SessionState::Stopped { failure },
            Self::Effects => SessionState::Effects { failure },
            Self::ScreenSync(_) => SessionState::ScreenSync,
            Self::Faulted(phase) => SessionState::Faulted(*phase),
        }
    }

    /// A source whose release is unconfirmed may still hold its device, so
    /// no new session can be trusted to open it.
    const UNRELEASED: Self = Self::Faulted(SessionPhase::SourceUnreleased);

    /// What a session's end leaves, from what its thread reported.
    fn left_by(end: &SessionEnd) -> Self {
        match &end.left {
            _ if end.released.is_err() => Self::UNRELEASED,
            Ok(IdleMode::Native) => Self::Effects,
            // An idle after the join that did not return in time.
            Err(Error::Faulted(phase)) => Self::Faulted(*phase),
            Ok(IdleMode::Off) | Err(_) => Self::Stopped,
        }
    }

    /// What a session thread that did not report its end leaves.
    fn failed(error: &Error) -> Self {
        match error {
            Error::Faulted(phase) => Self::Faulted(*phase),
            // The thread died without reporting, so its source may still be held.
            _ => Self::UNRELEASED,
        }
    }
}

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
    /// Why the store could not be read at start; reported until a save
    /// replaces what it holds.
    unreadable: Option<Error>,
    lighting: Lighting,
    /// When the snapshot's calibration is cleared; present exactly while it is.
    calibration_expires: Option<Instant>,
    /// Processing being tried instead of the saved one, and when it ends.
    trial: Option<(Processing, Instant)>,
    feeds: Feeds,
    stats: Arc<Stats>,
    stop_deadline: Duration,
    /// When blocking calls are abandoned: `None` until [`Engine::shutdown`].
    abandon_at: watch::Receiver<Option<Instant>>,
    /// The first blocking call abandoned at shutdown; shutdown reports it.
    abandoned: Option<Error>,
    state: watch::Sender<Snapshot>,
}

impl Core {
    /// Run blocking adapter or store code off the async runtime. A call still
    /// running at the shutdown's one deadline is abandoned, left running on
    /// its thread; once one was, later calls fail without starting. So calls
    /// that never return cannot keep the engine from the session's ordered
    /// stop.
    ///
    /// # Errors
    /// The call panicked, or was abandoned or not started.
    async fn run_blocking<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T> {
        if let Some(abandoned) = &self.abandoned {
            return Err(abandoned.clone());
        }
        let mut abandon_at = self.abandon_at.clone();
        let abandon = async move {
            let at = abandon_at
                .wait_for(Option::is_some)
                .await
                .ok()
                .and_then(|at| *at);
            match at {
                Some(at) => tokio::time::sleep_until(at).await,
                // The engine was dropped without a shutdown: nothing waits.
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            done = blocking(work) => done,
            () = abandon => {
                let error = Error::other(
                    "an adapter or settings call did not return within the shutdown \
                     grace, so it was abandoned",
                );
                self.abandoned = Some(error.clone());
                Err(error)
            }
        }
    }

    async fn run(mut self, mut inbox: mpsc::Receiver<Command>) {
        loop {
            let command = tokio::select! {
                command = inbox.recv() => command,
                event = next(&mut self.lighting) => {
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
                Some(Command::Request(request)) => {
                    self.settle().await;
                    match self.refusal() {
                        Some(error) => request.refuse(error),
                        None => self.handle(request).await,
                    }
                }
            }
        }
    }

    /// Handle the end of a session whose loop already ended on its own, as
    /// if the end had come before what is being handled now: a request or
    /// shutdown then meets what the end left, whichever arrived first. A
    /// session still running when the request arrives is the request's.
    async fn settle(&mut self) {
        if !matches!(&self.lighting, Lighting::ScreenSync(running) if running.session.has_ended()) {
            return;
        }
        let Some(running) = self.take_running() else {
            return;
        };
        // The thread is releasing what it holds: wait for it within the
        // stop deadline. `Off` is the mode it leaves the output in.
        let end = match running
            .session
            .stop(IdleMode::Off, self.stop_deadline)
            .await
        {
            Ok(Stop::AsAsked(end) | Stop::OnItsOwn(end)) => Ok(end),
            Err(error) => Err(error),
        };
        let result = self.ended(end);
        log_unpublished(&self.lighting, &result);
    }

    /// Why requests are refused unhandled: the engine is faulted, and only a
    /// restart recovers; or shutdown was asked for, and a request still
    /// queued would start what it stops.
    fn refusal(&self) -> Option<Error> {
        if let Lighting::Faulted(phase) = self.lighting {
            return Some(Error::Faulted(phase));
        }
        self.abandon_at.borrow().is_some().then_some(Error::Closed)
    }

    async fn handle(&mut self, request: Request) {
        match request {
            Request::SetTarget(target, reply) => {
                let _ = reply.send(self.set_target(target).await);
            }
            Request::SetCalibration(calibration, reply) => {
                let _ = reply.send(self.set_calibration(calibration));
            }
            Request::TryProcessing(processing, reply) => {
                let _ = reply.send(self.try_processing(*processing));
            }
            Request::ReplaceSettings {
                expected,
                settings,
                reply,
            } => {
                let _ = reply.send(self.replace_settings(expected, *settings).await);
            }
        }
    }

    /// Publish `lighting` without a failure: a command path, which then
    /// records its own ([`Self::record`]), so no earlier failure is shown
    /// beside the new state.
    fn set_lighting(&mut self, lighting: Lighting) {
        self.set_lighting_and(lighting, None, |_| {});
    }

    /// The only writer of the snapshot's `session`, with `failure` where it
    /// can carry one, publishing `change` in the same snapshot. It also
    /// resets `input` and `device`, which [`Self::measure`] updates while a
    /// session runs.
    fn set_lighting_and(
        &mut self,
        lighting: Lighting,
        failure: Option<Error>,
        change: impl FnOnce(&mut Snapshot),
    ) {
        let session = lighting.state(failure);
        let input = matches!(lighting, Lighting::ScreenSync(_)).then(Input::default);
        self.lighting = lighting;
        self.state.send_modify(|s| {
            s.session = session;
            s.input = input;
            s.device = DeviceReport::default();
            change(s);
        });
    }

    /// The running session, taken out of the lighting, which is left
    /// `Stopped` (unpublished) until the caller sets what the end leaves.
    fn take_running(&mut self) -> Option<Running> {
        match std::mem::replace(&mut self.lighting, Lighting::Stopped) {
            Lighting::ScreenSync(running) => Some(running),
            other => {
                self.lighting = other;
                None
            }
        }
    }

    fn measure(&mut self) {
        let Lighting::ScreenSync(running) = &mut self.lighting else {
            return;
        };
        let input = Some(running.meter.sample(&self.stats));
        let device = running.session.observed();
        self.state.send_if_modified(|s| {
            let changed = s.input != input || s.device != device;
            s.input = input;
            s.device = device;
            changed
        });
    }

    /// The session ended on its own and reported it.
    async fn session_ended(&mut self, end: Result<SessionEnd>) {
        if let Some(running) = self.take_running() {
            running.session.join().await;
        }
        let result = self.ended(end);
        log_unpublished(&self.lighting, &result);
    }

    /// A session ended on its own: a failure, or its source ended; seen when
    /// it ended, or by a stop that came after. Publish what it left with its
    /// failure, and return that failure. An ended source is completion: a
    /// `ScreenSync` target becomes `Off`, so lighting does not restart (a
    /// file would replay) until asked again; any other target was set by a
    /// command, and stands.
    fn ended(&mut self, end: Result<SessionEnd>) -> Result<()> {
        let (lighting, ended) = match &end {
            Ok(end) => (Lighting::left_by(end), matches!(end.exit, Exit::Ended)),
            Err(error) => (Lighting::failed(error), false),
        };
        // The result is the loop's failure: a release that also failed (why
        // the source is unreleased) is said only here.
        if let Some(release) = end.as_ref().ok().and_then(hidden_release) {
            tracing::warn!(
                error = %prysm_ports::chain(release),
                "the source of a session that failed on its own was not released"
            );
        }
        let result = end.and_then(SessionEnd::result);
        let failure = result.as_ref().err().cloned();
        let complete = ended && self.state.borrow().target == LightingTarget::ScreenSync;
        self.set_lighting_and(lighting, failure, |s| {
            if complete {
                s.target = LightingTarget::Off;
            }
        });
        if complete {
            self.calibrate(None);
        }
        result
    }

    async fn set_target(&mut self, target: LightingTarget) -> Result<()> {
        self.state.send_modify(|s| s.target = target);
        if target != LightingTarget::ScreenSync {
            self.calibrate(None);
        }
        let result = if target == LightingTarget::Off && self.off_may_not_be_off().is_some() {
            // Asked again while the lights may not be off: switch them off
            // again.
            self.idle_output(IdleMode::Off).await
        } else {
            self.reconcile(false, false).await
        };
        self.record(&result);
        result
    }

    /// No session, and the published failure is the output's: the lights
    /// may not be off.
    fn off_may_not_be_off(&self) -> Option<Error> {
        if !matches!(self.lighting, Lighting::Stopped) {
            return None;
        }
        self.state
            .borrow()
            .session
            .failure()
            .filter(|failure| failure.section() == Some(Section::Output))
            .cloned()
    }

    async fn replace_settings(
        &mut self,
        expected: Revision,
        settings: Settings,
    ) -> Result<Revision> {
        let current = self.state.borrow().revision;
        if expected != current {
            return Err(Error::StaleRevision { expected, current });
        }
        self.registry.validate(&settings)?;
        let store = self.store.clone();
        let saved = settings.clone();
        self.run_blocking(move || lock(&store).save(&saved))
            .await??;
        self.unreadable = None;

        let (output_changed, capture_changed) = {
            let snapshot = self.state.borrow();
            (
                snapshot.settings.output != settings.output,
                snapshot.settings.capture != settings.capture,
            )
        };
        let adapters_changed = output_changed || capture_changed;
        let revision = current.next();
        // Saved settings end a trial.
        let install = |core: &mut Self| {
            core.trial = None;
            core.state.send_modify(|s| {
                s.settings = settings;
                s.revision = revision;
            });
        };
        if matches!(self.lighting, Lighting::ScreenSync(_)) && !adapters_changed {
            // Same adapters: the running session takes the new processing.
            self.reconfigure(install);
        } else {
            install(self);
        }
        let applied = self.reconcile(adapters_changed, output_changed).await;
        self.record(&applied);
        Ok(revision)
    }

    /// Drive the session towards the target. A target that is not reached
    /// (a failed start or idle) is attempted again at the next call, so every
    /// settings save retries it. `restart` replaces a running session (its
    /// adapters changed); `output_changed` puts an output showing its effects
    /// in that mode again (it may be another device).
    async fn reconcile(&mut self, restart: bool, output_changed: bool) -> Result<()> {
        let target = self.state.borrow().target;
        match (target, &self.lighting) {
            // Requests are refused before they get here (see
            // `Self::refusal`); this keeps the match total.
            (_, Lighting::Faulted(phase)) => Err(Error::Faulted(*phase)),
            (LightingTarget::ScreenSync, Lighting::ScreenSync(_)) if restart => {
                // Same path as Off followed by ScreenSync. The new session
                // starts even when idling the old output failed (that device
                // may be the one being replaced), but not over a source whose
                // release is unconfirmed, nor after a source that had ended
                // on its own: that turned the target `Off`.
                let stopped = self.stop(IdleMode::Off).await;
                let target = self.state.borrow().target;
                if target != LightingTarget::ScreenSync
                    || !matches!(self.lighting, Lighting::Stopped)
                {
                    return stopped;
                }
                // What the old session's stop left is replaced by the new
                // session: its failure is logged, not published.
                if let Err(error) = &stopped {
                    tracing::warn!(
                        section = ?error.section(),
                        error = %prysm_ports::chain(error),
                        "the replaced session did not stop cleanly"
                    );
                }
                self.start().await
            }
            (LightingTarget::ScreenSync, Lighting::ScreenSync(_)) => Ok(()),
            // Also from `Effects`: a start that failed from the device's own
            // effects leaves them showing, and the target says `ScreenSync`.
            (LightingTarget::ScreenSync, _) => self.start().await,
            (LightingTarget::Effects, Lighting::ScreenSync(_)) => self.stop(IdleMode::Native).await,
            (LightingTarget::Effects, Lighting::Effects) if !output_changed => Ok(()),
            (LightingTarget::Effects, _) => self.idle_output(IdleMode::Native).await,
            (LightingTarget::Off, Lighting::ScreenSync(_)) => self.stop(IdleMode::Off).await,
            (LightingTarget::Off, Lighting::Effects) => self.idle_output(IdleMode::Off).await,
            // A command that does nothing about the output keeps a failure
            // that says the lights may not be off: it is still true. Other
            // lights make it no longer so.
            (LightingTarget::Off, Lighting::Stopped) if !output_changed => {
                self.off_may_not_be_off().map_or(Ok(()), Err)
            }
            (LightingTarget::Off, Lighting::Stopped) => Ok(()),
        }
    }

    /// Open the source and start a session. The output is validated here
    /// but opened by the session at its first frame, so a source without
    /// frames never takes the device over.
    async fn start(&mut self) -> Result<()> {
        let (capture, output) = {
            let snapshot = self.state.borrow();
            let settings = &snapshot.settings;
            let capture = self.configured(settings.capture.as_ref(), Section::Capture)?;
            let output = self.configured(settings.output.as_ref(), Section::Output)?;
            self.registry.validate(settings)?;
            (capture, output)
        };
        // Outside the borrow above: `config` borrows the state itself.
        let config = self.config();
        let registry = self.registry.clone();
        let source = self
            .run_blocking(move || registry.open_source(&capture))
            .await?
            .map_err(|error| error.in_section(Section::Capture))?;
        self.stats.source_opened();
        let output = Configured::new(self.registry.clone(), output);
        let session = match Session::start(
            source,
            output,
            config,
            self.feeds.clone(),
            self.stats.clone(),
        ) {
            Ok(session) => session,
            Err((error, source)) => {
                // The session never ran: release the source here instead.
                let released = self
                    .run_blocking(move || source.close())
                    .await
                    .and_then(|closed| closed);
                if let Err(release) = &released {
                    // The fault says only its phase: its cause is logged.
                    tracing::warn!(
                        error = %prysm_ports::chain(release),
                        "the source of a session that could not start was not released"
                    );
                    self.set_lighting(Lighting::UNRELEASED);
                }
                return Err(error);
            }
        };
        self.set_lighting(Lighting::ScreenSync(Running {
            session,
            meter: Meter::new(&self.stats),
        }));
        Ok(())
    }

    /// Stop the running session, if one runs: without one there is nothing
    /// to stop and the lighting stays as it is. A session that ended on its
    /// own after the request began (one that had ended before was settled
    /// first, see [`Self::settle`]) ended while being stopped. Stopped for
    /// `Off`, its end is published ([`Self::ended`]) and the reply is its
    /// failures in order, then the output's: with the lights off, they say
    /// why the lights may not be. Stopped for the lights' own effects, the
    /// effects replace what it ended with, which is logged: the reply is
    /// the output's idle (see [`replaced_by_effects`]). An unconfirmed
    /// release wins the state, then an idle that did not return in time.
    async fn stop(&mut self, mode: IdleMode) -> Result<()> {
        let Some(running) = self.take_running() else {
            return Ok(());
        };
        match running.session.stop(mode, self.stop_deadline).await {
            Err(error) => {
                self.set_lighting(Lighting::failed(&error));
                Err(error)
            }
            Ok(Stop::AsAsked(end)) => {
                self.set_lighting(Lighting::left_by(&end));
                end.result()
            }
            Ok(Stop::OnItsOwn(end)) if mode == IdleMode::Native => {
                let (lighting, reply) = replaced_by_effects(end);
                self.set_lighting(lighting);
                reply
            }
            // What the output was left in is the end's, idled again or not.
            Ok(Stop::OnItsOwn(end)) => self.ended(Ok(end)),
        }
    }

    /// Effects have no session: put the output in `mode` without
    /// opening it.
    async fn idle_output(&mut self, mode: IdleMode) -> Result<()> {
        let output = self.configured(
            self.state.borrow().settings.output.as_ref(),
            Section::Output,
        );
        let registry = self.registry.clone();
        let result = match output {
            Ok(output) => self
                .run_blocking(move || registry.idle_output(&output, mode))
                .await
                .and_then(|idled| idled.map_err(|error| error.in_section(Section::Output))),
            Err(error) => Err(error),
        };
        self.set_lighting(match (&result, mode) {
            (Ok(()), IdleMode::Native) => Lighting::Effects,
            _ => Lighting::Stopped,
        });
        result
    }

    fn set_calibration(&mut self, calibration: Option<CalibrationPattern>) -> Result<()> {
        if calibration.is_some() && self.state.borrow().target != LightingTarget::ScreenSync {
            return Err(Error::NeedsScreenSync);
        }
        self.calibrate(calibration);
        Ok(())
    }

    fn try_processing(&mut self, processing: Processing) -> Result<()> {
        processing.validate()?;
        self.reconfigure(|core| core.trial = Some((processing, Instant::now() + TRIAL_TIMEOUT)));
        Ok(())
    }

    /// Publish `calibration`, restart its timeout and give a running session
    /// the processing config it implies.
    fn calibrate(&mut self, calibration: Option<CalibrationPattern>) {
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
        if !matches!(self.lighting, Lighting::ScreenSync(_)) {
            change(self);
            return;
        }
        let before = self.config();
        change(self);
        let after = self.config();
        if let Lighting::ScreenSync(running) = &self.lighting
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

    /// `section`'s adapter settings. Missing, they are not configured; while
    /// the store could not be read, its failure says why.
    fn configured(
        &self,
        settings: Option<&AdapterSettings>,
        section: Section,
    ) -> Result<AdapterSettings> {
        match (settings, &self.unreadable) {
            (Some(settings), _) => Ok(settings.clone()),
            (None, Some(unreadable)) => Err(unreadable.clone()),
            (None, None) => Err(ValidationError::new(section.as_str(), "not configured").into()),
        }
    }

    /// Publish `result` as the session's failure. Without one, a store that
    /// could not be read, or saved settings that do not validate (loaded
    /// that way), stay reported until replaced. A fault carries no failure,
    /// so what ended in it is logged ([`unpublished`]). Running screen sync
    /// is reached only by a command that succeeded.
    fn record(&self, result: &Result<()>) {
        debug_assert!(
            !(result.is_err() && matches!(self.lighting, Lighting::ScreenSync(_))),
            "a failure beside running screen sync: {result:?}"
        );
        log_unpublished(&self.lighting, result);
        self.state.send_modify(|s| {
            let failure = match result {
                Err(error) => Some(error.clone()),
                Ok(()) => self
                    .unreadable
                    .clone()
                    .or_else(|| self.registry.validate(&s.settings).err()),
            };
            s.session = self.lighting.state(failure);
        });
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.settle().await;
        // Calibration ends at shutdown even on a faulted engine: the last
        // snapshot shows no pattern that nothing is showing any more.
        self.calibrate(None);
        if let Lighting::Faulted(phase) = self.lighting {
            return Err(Error::Faulted(phase));
        }
        let stopped = self.stop(IdleMode::Off).await;
        let result = stopped.and(self.abandoned.take().map_or(Ok(()), Err));
        self.record(&result);
        result
    }
}

async fn expiry(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn next(lighting: &mut Lighting) -> Event {
    match lighting {
        Lighting::ScreenSync(running) => running.next().await,
        Lighting::Stopped | Lighting::Effects | Lighting::Faulted(_) => {
            std::future::pending().await
        }
    }
}

/// The error a fault leaves unpublished: what ended in it, when that is not
/// the fault itself. A fault's state carries only its phase.
fn unpublished<'a>(
    lighting: &Lighting,
    result: &'a Result<()>,
) -> Option<(SessionPhase, &'a Error)> {
    match (lighting, result) {
        (Lighting::Faulted(phase), Err(error)) if !matches!(error, Error::Faulted(_)) => {
            Some((*phase, error))
        }
        _ => None,
    }
}

/// The release failure a session end's result does not carry: the loop's
/// own failure comes first ([`SessionEnd::result`]).
fn hidden_release(end: &SessionEnd) -> Option<&Error> {
    match (&end.exit, &end.released) {
        (Exit::Failed(_), Err(release)) => Some(release),
        _ => None,
    }
}

/// Log what [`unpublished`] finds, so a fault's cause is never lost.
fn log_unpublished(lighting: &Lighting, result: &Result<()>) {
    if let Some((phase, error)) = unpublished(lighting, result) {
        tracing::warn!(
            %phase,
            error = %prysm_ports::chain(error),
            "lighting faulted after this failure"
        );
    }
}

/// A session that ended on its own while being stopped for the lights' own
/// effects: what it leaves, and the reply. The effects replace what the
/// session ended with, so its failure is logged, not published; the reply
/// is whether the effects came on, unless the source's release is
/// unconfirmed.
fn replaced_by_effects(end: SessionEnd) -> (Lighting, Result<()>) {
    let lighting = Lighting::left_by(&end);
    if let Exit::Failed(error) = &end.exit {
        tracing::warn!(
            section = ?error.section(),
            error = %prysm_ports::chain(error),
            "the session failed on its own as the lights' own effects replaced it"
        );
    }
    let reply = end.released.and(end.left.map(drop));
    (lighting, reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure() -> Error {
        Error::other("cannot idle").in_section(Section::Output)
    }

    /// A release failure behind the loop's own failure is found, once; any
    /// other end has none to add.
    #[test]
    fn a_release_failure_behind_a_loop_failure_is_found() {
        let end = |exit: Exit, released: Result<()>| SessionEnd {
            exit,
            released,
            left: Ok(IdleMode::Off),
        };
        let release = || Err(Error::other("cannot close"));
        let failed = || Exit::Failed(Error::other("source failed"));
        let found = |end: &SessionEnd| hidden_release(end).map(ToString::to_string);
        assert_eq!(
            found(&end(failed(), release())).as_deref(),
            Some("cannot close")
        );
        assert_eq!(found(&end(failed(), Ok(()))), None);
        assert_eq!(
            found(&end(Exit::Stopped, release())),
            None,
            "it is the result"
        );
        assert_eq!(
            found(&end(Exit::Ended, release())),
            None,
            "it is the result"
        );
    }

    /// What a fault leaves unpublished is logged; a failure a stopped or
    /// effects state carries, a fault's own error and success are not.
    #[test]
    fn a_fault_logs_the_error_it_ended_in() {
        use SessionPhase::{ClosingSource, SourceUnreleased};
        let release = || Err(Error::other("cannot close").in_section(Section::Capture));
        let cases: [(Lighting, Result<()>, Option<SessionPhase>); 6] = [
            (Lighting::UNRELEASED, release(), Some(SourceUnreleased)),
            (
                Lighting::Faulted(ClosingSource),
                Err(Error::Panicked("session thread")),
                Some(ClosingSource),
            ),
            (
                Lighting::UNRELEASED,
                Err(Error::Faulted(SourceUnreleased)),
                None,
            ),
            (Lighting::UNRELEASED, Ok(()), None),
            (Lighting::Stopped, release(), None),
            (Lighting::Effects, release(), None),
        ];
        for (lighting, result, logged) in cases {
            let found = unpublished(&lighting, &result).map(|(phase, _)| phase);
            assert_eq!(found, logged, "{result:?}");
        }
    }

    /// Every way a session can end, against what it leaves, and against
    /// what a stop for the lights' own effects leaves and replies: never the
    /// session's own failure.
    #[test]
    fn every_end_leaves_one_state_and_effects_replace_its_failure() {
        use SessionPhase::{IdlingOutput, SourceUnreleased};
        let stopped = || SessionState::default();
        let effects = || SessionState::Effects { failure: None };
        let idling = Error::Faulted(IdlingOutput);
        // (exit failed, released, left, state left, effects reply is ok)
        let cases: [(bool, bool, Result<IdleMode>, SessionState, bool); 10] = [
            (false, true, Ok(IdleMode::Off), stopped(), true),
            (false, true, Ok(IdleMode::Native), effects(), true),
            (false, true, Err(failure()), stopped(), false),
            (
                false,
                true,
                Err(idling.clone()),
                SessionState::Faulted(IdlingOutput),
                false,
            ),
            (
                false,
                false,
                Ok(IdleMode::Native),
                SessionState::Faulted(SourceUnreleased),
                false,
            ),
            (true, true, Ok(IdleMode::Off), stopped(), true),
            (true, true, Ok(IdleMode::Native), effects(), true),
            (true, true, Err(failure()), stopped(), false),
            (
                true,
                true,
                Err(idling),
                SessionState::Faulted(IdlingOutput),
                false,
            ),
            (
                true,
                false,
                Ok(IdleMode::Off),
                SessionState::Faulted(SourceUnreleased),
                false,
            ),
        ];
        for (failed, released, left, expected, reply_ok) in cases {
            let end = || SessionEnd {
                exit: if failed {
                    Exit::Failed(Error::other("source failed"))
                } else {
                    Exit::Stopped
                },
                released: if released {
                    Ok(())
                } else {
                    Err(Error::other("cannot close"))
                },
                left: left.clone(),
            };
            let row = format!("failed {failed}, released {released}, left {left:?}");
            let state = Lighting::left_by(&end()).state(None);
            assert_eq!(state.to_string(), expected.to_string(), "{row}");
            let (lighting, reply) = replaced_by_effects(end());
            assert_eq!(
                lighting.state(None).to_string(),
                expected.to_string(),
                "{row}"
            );
            assert_eq!(reply.is_ok(), reply_ok, "{row}: {reply:?}");
            if let Err(error) = reply {
                assert_ne!(error.to_string(), "source failed", "{row}");
            }
        }
    }

    /// A stopped or effects state carries the failure it is given; a fault
    /// carries none.
    #[test]
    fn only_stopped_and_effects_carry_a_failure() {
        for (lighting, carries) in [
            (Lighting::Stopped, true),
            (Lighting::Effects, true),
            (Lighting::UNRELEASED, false),
        ] {
            let state = lighting.state(Some(failure()));
            assert_eq!(state.failure().is_some(), carries, "{state}");
        }
    }
}
