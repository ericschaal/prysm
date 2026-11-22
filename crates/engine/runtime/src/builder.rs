use crate::surface::Surfaces;
use crate::{Engine, EngineHandle, Surface};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use prysm_ports::{
    Error, OutputFactory, Registry, Result, SOURCE_OPEN_LIMIT, SettingsStore, SourceFactory,
};
use std::future::Future;
use std::time::Duration;

/// How long a stopping session may take unless the host sets its own
/// ([`Builder::stop_deadline`]); the desktop hosts keep it.
pub const STOP_DEADLINE: Duration = Duration::from_secs(10);

/// The longest an engine command takes with [`STOP_DEADLINE`]: replacing a
/// running session stops it and then opens the new source (the output opens
/// later, on the session thread). Every other command does less: a stop, an
/// idle within an output's own bound, or nothing blocking. The portal sets
/// no deadline of its own; a client waits longer than this. A host with a
/// shorter stop deadline (the TV's) stays below it.
///
/// It is one command's own time, not its wait behind others in the
/// engine's mailbox, which runs one at a time: a command queued behind a
/// slow one takes up to this per command ahead of it. A client's limit
/// (the portal's 30 s) therefore covers one slow command and not a queue
/// of them; past it, the client reads the state for the outcome.
pub const COMMAND_LIMIT: Duration = STOP_DEADLINE.saturating_add(SOURCE_OPEN_LIMIT);
const _: () = assert!(
    COMMAND_LIMIT.as_millis() < 30_000,
    "the portal's client waits 30 s for a command (frontend/packages/application-client/http.ts)"
);

/// Entry point for a full host: `Prysm::builder(store).source(..).output(..).surface(..).run(..)`.
pub struct Prysm;

impl Prysm {
    pub fn builder(store: impl SettingsStore + 'static) -> Builder {
        Builder {
            store: Box::new(store),
            registry: Registry::default(),
            surfaces: Vec::new(),
            stop_deadline: STOP_DEADLINE,
            shutdown_grace: None,
        }
    }
}

pub struct Builder {
    store: Box<dyn SettingsStore>,
    registry: Registry,
    surfaces: Vec<Box<dyn Surface>>,
    stop_deadline: Duration,
    shutdown_grace: Option<Duration>,
}

impl Builder {
    #[must_use]
    pub fn source(mut self, factory: impl SourceFactory) -> Self {
        self.registry.add_source(factory);
        self
    }

    #[must_use]
    pub fn output(mut self, factory: impl OutputFactory) -> Self {
        self.registry.add_output(factory);
        self
    }

    #[must_use]
    pub fn surface(mut self, surface: impl Surface) -> Self {
        self.surfaces.push(Box::new(surface));
        self
    }

    /// How long a stopping session (and each surface at shutdown) may take.
    #[must_use]
    pub fn stop_deadline(mut self, deadline: Duration) -> Self {
        self.stop_deadline = deadline;
        self
    }

    /// How long shutdown waits for blocking adapter or settings calls before
    /// abandoning them (see [`Engine::shutdown_grace`]); the stop deadline
    /// unless set.
    #[must_use]
    pub fn shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = Some(grace);
        self
    }

    /// Load the settings and validate them against the registered adapters,
    /// without starting the engine, a surface or any device.
    ///
    /// # Errors
    /// The store failed to load or holds nothing, or the settings are invalid.
    pub fn check(mut self) -> Result<()> {
        let settings = self
            .store
            .load()?
            .ok_or_else(|| Error::other("no settings saved yet"))?;
        self.registry.validate(&settings)
    }

    /// Run until `shutdown` resolves, then stop the session, idle the output and
    /// join everything. Surface failures are reported in the snapshot.
    ///
    /// # Errors
    /// Settings could not be loaded, or the session could not be stopped cleanly.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let (engine, owner): (EngineHandle, Engine) =
            Engine::start(self.registry, self.store, self.stop_deadline).await?;
        let owner = owner.shutdown_grace(self.shutdown_grace.unwrap_or(self.stop_deadline));
        let mut surfaces = Surfaces::spawn(self.surfaces, &engine);
        // Reports waiting for room in the engine's mailbox, so a full one
        // holds up neither the watch for shutdown nor the next failure.
        let mut reports = FuturesUnordered::new();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                failure = surfaces.failure() => {
                    let engine = engine.clone();
                    reports.push(async move { engine.report_surface_failure(failure).await });
                }
                Some(()) = reports.next() => {}
            }
        }
        surfaces.request_shutdown();
        // The waiting reports reach the snapshot before the engine ends. Its
        // shutdown grace runs from here, and queued requests are refused,
        // so waiting for room is bounded by that grace.
        owner.begin_shutdown();
        reports.collect::<()>().await;
        let result = owner.shutdown().await;
        surfaces.finish(self.stop_deadline).await;
        result
    }
}
