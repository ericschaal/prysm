use crate::{PreviewSlot, PreviewWatch, Session, SessionEnd};
use prysm_ports::{
    AdapterSettings, Error, IdleMode, LightingTarget, Registry, Result, Revision, SessionPhase,
    SessionState, Settings, SettingsStore, Snapshot, ValidationError,
};
use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

type Reply<T> = oneshot::Sender<Result<T>>;

enum Command {
    SetTarget(LightingTarget, Reply<()>),
    ReplaceSettings {
        expected: Revision,
        settings: Box<Settings>,
        reply: Reply<Revision>,
    },
    SurfaceFailed(Error),
    Shutdown(Reply<()>),
}

/// Cloneable access to a running engine.
#[derive(Clone)]
pub struct EngineHandle {
    commands: mpsc::Sender<Command>,
    snapshot: watch::Receiver<Snapshot>,
    preview: Arc<PreviewSlot>,
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

    #[must_use]
    pub fn snapshot(&self) -> watch::Receiver<Snapshot> {
        self.snapshot.clone()
    }

    /// Watch the latest frame and colors written by the running session. The
    /// session publishes previews only while at least one watch exists.
    #[must_use]
    pub fn preview(&self) -> PreviewWatch {
        self.preview.watch()
    }

    /// How many preview watches exist.
    #[must_use]
    pub fn preview_viewers(&self) -> usize {
        self.preview.viewers()
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
        let preview = Arc::new(PreviewSlot::default());
        let (commands, inbox) = mpsc::channel(16);
        let core = Core {
            registry,
            store,
            session: None,
            preview: preview.clone(),
            stop_deadline,
            state,
        };
        let task = tokio::spawn(core.run(inbox));
        let handle = EngineHandle {
            commands: commands.clone(),
            snapshot,
            preview,
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

struct Core {
    registry: Registry,
    store: Store,
    session: Option<Session>,
    preview: Arc<PreviewSlot>,
    stop_deadline: Duration,
    state: watch::Sender<Snapshot>,
}

impl Core {
    async fn run(mut self, mut inbox: mpsc::Receiver<Command>) {
        loop {
            let command = tokio::select! {
                command = inbox.recv() => command,
                end = finished(&mut self.session) => {
                    self.session_ended(end).await;
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
                    let result = match self.faulted() {
                        Some(fault) => Err(fault),
                        None => self.set_target(target).await,
                    };
                    let _ = reply.send(result);
                }
                Some(Command::ReplaceSettings {
                    expected,
                    settings,
                    reply,
                }) => {
                    let result = match self.faulted() {
                        Some(fault) => Err(fault),
                        None => self.replace_settings(expected, *settings).await,
                    };
                    let _ = reply.send(result);
                }
            }
        }
    }

    fn faulted(&self) -> Option<Error> {
        match self.state.borrow().session {
            SessionState::Faulted(phase) => Some(Error::Faulted(phase)),
            _ => None,
        }
    }

    async fn session_ended(&mut self, end: Result<SessionEnd>) {
        if let Some(session) = self.session.take() {
            session.join().await;
        }
        let session = match &end {
            Ok(end) if end.released.is_err() => UNRELEASED,
            _ => SessionState::Stopped,
        };
        let failure = end.and_then(SessionEnd::result).err();
        self.state.send_modify(|s| {
            s.session = session;
            s.failure = failure;
        });
    }

    async fn set_target(&mut self, target: LightingTarget) -> Result<()> {
        let current = self.state.borrow().session;
        self.state.send_modify(|s| s.target = target);
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
            // failure: the new settings get the next attempt.
            SessionState::Stopped if target == LightingTarget::Processed => self.start().await,
            SessionState::Running if adapters_changed => {
                // Same path as Off followed by Processed.
                match self.stop(IdleMode::Off).await {
                    Ok(()) => self.start().await,
                    Err(error) => Err(error),
                }
            }
            SessionState::Running if previous.processing != settings.processing => {
                if let Some(session) = &self.session {
                    session.configure(settings.processing.config());
                }
                Ok(())
            }
            SessionState::Native if output_changed => self.idle_output(IdleMode::Native).await,
            _ => Ok(()),
        };
        self.record(&applied);
        Ok(revision)
    }

    async fn start(&mut self) -> Result<()> {
        let (capture, output, config) = {
            let snapshot = self.state.borrow();
            let settings = &snapshot.settings;
            (
                configured(settings.capture.as_ref(), "capture")?,
                configured(settings.output.as_ref(), "output")?,
                settings.processing.config(),
            )
        };
        let registry = self.registry.clone();
        let (source, output) = blocking(move || {
            let source = registry.open_source(&capture)?;
            match registry.open_output(&output) {
                Ok(output) => Ok((source, output)),
                Err(error) => {
                    // Release what was opened; the open failure is the one reported.
                    let _ = source.close();
                    Err(error)
                }
            }
        })
        .await??;
        self.session = Some(Session::start(
            source,
            output,
            config,
            self.preview.clone(),
        )?);
        self.state
            .send_modify(|s| s.session = SessionState::Running);
        Ok(())
    }

    async fn stop(&mut self, mode: IdleMode) -> Result<()> {
        let Some(session) = self.session.take() else {
            return Ok(());
        };
        let result = session.stop(mode, self.stop_deadline).await;
        let state = match &result {
            Err(Error::Faulted(phase)) => SessionState::Faulted(*phase),
            Ok(end) if end.released.is_err() => UNRELEASED,
            Ok(end) if mode == IdleMode::Native && end.idled.is_ok() => SessionState::Native,
            _ => SessionState::Stopped,
        };
        self.state.send_modify(|s| s.session = state);
        result.and_then(SessionEnd::result)
    }

    /// DeviceNative has no session: open the output only to put it in `mode`.
    async fn idle_output(&mut self, mode: IdleMode) -> Result<()> {
        let output = configured(self.state.borrow().settings.output.as_ref(), "output");
        let registry = self.registry.clone();
        let result = match output {
            Ok(output) => blocking(move || registry.open_output(&output)?.idle(mode)).await?,
            Err(error) => Err(error),
        };
        let state = match (&result, mode) {
            (Ok(()), IdleMode::Native) => SessionState::Native,
            _ => SessionState::Stopped,
        };
        self.state.send_modify(|s| s.session = state);
        result
    }

    fn record(&self, result: &Result<()>) {
        let failure = result.as_ref().err().cloned();
        self.state.send_modify(|s| s.failure = failure);
    }

    async fn shutdown(&mut self) -> Result<()> {
        if let Some(fault) = self.faulted() {
            return Err(fault);
        }
        let result = self.stop(IdleMode::Off).await;
        self.record(&result);
        result
    }
}

fn configured(settings: Option<&AdapterSettings>, section: &str) -> Result<AdapterSettings> {
    settings
        .cloned()
        .ok_or_else(|| ValidationError::new(section, "not configured").into())
}

async fn finished(session: &mut Option<Session>) -> Result<SessionEnd> {
    match session {
        Some(session) => session.finished().await,
        None => std::future::pending().await,
    }
}
