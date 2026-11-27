//! The engine task owns settings and at most one session. Candidate and mode lookups bypass its mailbox.
mod control;
mod deadlines;
mod handle;
mod lookups;
mod settings;
mod shutdown;

pub use handle::EngineHandle;
pub use settings::{CALIBRATION_TIMEOUT, TRIAL_TIMEOUT};

use crate::feeds::Feeds;
use crate::intent::LightingIntent;
use crate::stats::Stats;
use control::Lighting;
use deadlines::Due;
use handle::{Command, Request};
use lookups::Lookups;
use prysm_ports::{
    Error, LightingMode, Processing, Registry, Result, SessionState, Settings, SettingsStore,
    Snapshot,
};
use settings::{Store, first_revision, lock};
use shutdown::blocking;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

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
        let (loaded, remembered) = blocking({
            let store = store.clone();
            move || {
                let mut store = lock(&store);
                (store.load(), store.load_mode())
            }
        })
        .await?;
        let remembered = remembered.unwrap_or_else(|error| {
            tracing::warn!(error = %prysm_ports::chain(&error), "the previous mode could not be read");
            None
        });
        let (loaded, unreadable) = match loaded {
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
        let supports_effects = loaded
            .output
            .as_ref()
            .is_none_or(|output| registry.supports_effects(output));
        let intent = LightingIntent::new(remembered, supports_effects);
        let ports = control::Ports::new(&registry, &loaded, unreadable.as_ref());
        let failure = unreadable.clone().or_else(|| ports.failure.clone());
        let lighting = Lighting::new(intent, ports);
        let persisted_mode = remembered;
        let (state, snapshot) = watch::channel(Snapshot {
            control: lighting.control(),
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
            lighting,
            persisted_mode,
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
}

/// The engine task's state: what [`Engine::start`] spawns, and the only
/// writer of the snapshot.
struct Core {
    registry: Registry,
    store: Store,
    /// Why the store could not be read at start; reported until a save
    /// replaces what it holds.
    unreadable: Option<Error>,
    lighting: Lighting,
    persisted_mode: Option<LightingMode>,
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
    async fn run(mut self, mut inbox: mpsc::Receiver<Command>) {
        self.check_output();
        loop {
            let deadlines = self.deadlines();
            let command = tokio::select! {
                biased;
                command = inbox.recv() => command,
                event = self.lighting.next() => {
                    self.lighting_event(event).await;
                    self.persist_mode().await;
                    continue;
                }
                due = deadlines.next() => {
                    match due {
                        Due::Calibration => self.calibrate(None),
                        Due::Trial => self.reconfigure(|core| core.trial = None),
                    }
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
                Some(Command::Request(request)) => match self.refusal() {
                    Some(error) => request.refuse(error),
                    None => self.handle(request).await,
                },
            }
        }
    }

    /// Why requests are refused unhandled: the engine is faulted, and only a
    /// restart recovers; or shutdown was asked for, and a request still
    /// queued would start what it stops.
    fn refusal(&self) -> Option<Error> {
        if let Some(phase) = self.lighting.fault() {
            return Some(Error::Faulted(phase));
        }
        self.abandon_at.borrow().is_some().then_some(Error::Closed)
    }

    async fn handle(&mut self, request: Request) {
        match request {
            Request::SetTarget(target, reply) => {
                self.set_target(target);
                self.persist_mode().await;
                let _ = reply.send(Ok(()));
            }
            Request::ResumeScreenSync(reply) => {
                self.resume_screen_sync();
                self.persist_mode().await;
                let _ = reply.send(Ok(()));
            }
            Request::PressPower(reply) => {
                self.press_power();
                self.persist_mode().await;
                let _ = reply.send(Ok(()));
            }
            Request::Pause(reply) => {
                self.pause_lighting();
                let _ = reply.send(Ok(()));
            }
            Request::Recheck(reply) => {
                self.recheck_lighting();
                let _ = reply.send(Ok(()));
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
                let result = self.replace_settings(expected, *settings).await;
                self.persist_mode().await;
                let _ = reply.send(result);
            }
        }
    }
}
