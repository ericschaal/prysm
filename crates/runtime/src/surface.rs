use crate::EngineHandle;
use futures::future::BoxFuture;
use prysm_ports::{Error, Result};
use std::future::Future;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinSet;

/// A way in or out of the runtime: web portal, TUI, TV UI bridge, CLI.
///
/// `H` is what the surface controls: an [`EngineHandle`] in a full host, `()`
/// under a [`Forwarder`](crate::Forwarder), which has no engine. A surface
/// failure is reported and does not stop lighting. Closures
/// `FnOnce(H, Shutdown) -> impl Future<Output = Result<()>>` are surfaces.
pub trait Surface<H = EngineHandle>: Send + 'static {
    fn run(self: Box<Self>, handle: H, shutdown: Shutdown) -> BoxFuture<'static, Result<()>>;
}

impl<H, F, Fut> Surface<H> for F
where
    F: FnOnce(H, Shutdown) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    fn run(self: Box<Self>, handle: H, shutdown: Shutdown) -> BoxFuture<'static, Result<()>> {
        Box::pin((*self)(handle, shutdown))
    }
}

/// Resolves once the host is shutting down. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Shutdown(watch::Receiver<bool>);

impl Shutdown {
    pub async fn requested(&self) {
        // A dropped sender also means shutdown.
        let _ = self.0.clone().wait_for(|requested| *requested).await;
    }

    #[must_use]
    pub fn is_requested(&self) -> bool {
        *self.0.borrow()
    }
}

/// Running surfaces of one host.
pub(crate) struct Surfaces {
    trigger: watch::Sender<bool>,
    tasks: JoinSet<Result<()>>,
}

impl Surfaces {
    pub(crate) fn spawn<H: Clone + 'static>(
        surfaces: Vec<Box<dyn Surface<H>>>,
        handle: &H,
    ) -> Self {
        let (trigger, receiver) = watch::channel(false);
        let mut tasks = JoinSet::new();
        for surface in surfaces {
            tasks.spawn(surface.run(handle.clone(), Shutdown(receiver.clone())));
        }
        Self { trigger, tasks }
    }

    /// The next surface failure; pending while every surface is healthy.
    pub(crate) async fn failure(&mut self) -> Error {
        loop {
            match self.tasks.join_next().await {
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(error))) => return error,
                Some(Err(_)) => return Error::Panicked("surface"),
                None => std::future::pending().await,
            }
        }
    }

    pub(crate) fn request_shutdown(&self) {
        self.trigger.send_replace(true);
    }

    /// Wait for surfaces to finish; abort those that outlive `deadline`.
    pub(crate) async fn finish(mut self, deadline: Duration) {
        self.request_shutdown();
        let drained = tokio::time::timeout(deadline, async {
            while self.tasks.join_next().await.is_some() {}
        })
        .await;
        if drained.is_err() {
            self.tasks.shutdown().await;
        }
    }
}
