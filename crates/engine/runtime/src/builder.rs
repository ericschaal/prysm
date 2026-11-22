use crate::surface::Surfaces;
use crate::{Engine, EngineHandle, Surface};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use prysm_ports::{Error, OutputFactory, Registry, Result, SettingsStore, SourceFactory};
use std::future::Future;
use std::time::Duration;

const STOP_DEADLINE: Duration = Duration::from_secs(10);

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
