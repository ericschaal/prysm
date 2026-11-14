use crate::surface::Surfaces;
use crate::{Engine, EngineHandle, Surface};
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
        }
    }
}

pub struct Builder {
    store: Box<dyn SettingsStore>,
    registry: Registry,
    surfaces: Vec<Box<dyn Surface>>,
    stop_deadline: Duration,
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
        let mut surfaces = Surfaces::spawn(self.surfaces, &engine);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                failure = surfaces.failure() => engine.report_surface_failure(failure).await,
            }
        }
        surfaces.request_shutdown();
        let result = owner.shutdown().await;
        surfaces.finish(self.stop_deadline).await;
        result
    }
}
