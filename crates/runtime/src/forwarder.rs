use crate::Surface;
use crate::surface::Surfaces;
use crate::worker::Worker;
use prysm_ports::{Error, FrameSink, FrameSource, Result, SessionPhase};
use std::future::Future;
use std::time::Duration;

const STOP_DEADLINE: Duration = Duration::from_secs(5);

/// A capture-only host: the session loop with a [`FrameSink`] in place of
/// processor and output. No engine, no settings; surfaces get `()`.
pub struct Forwarder {
    source: Box<dyn FrameSource>,
    sink: Box<dyn FrameSink>,
    surfaces: Vec<Box<dyn Surface<()>>>,
    stop_deadline: Duration,
}

impl Forwarder {
    pub fn new(source: impl FrameSource + 'static, sink: impl FrameSink + 'static) -> Self {
        Self {
            source: Box::new(source),
            sink: Box::new(sink),
            surfaces: Vec::new(),
            stop_deadline: STOP_DEADLINE,
        }
    }

    #[must_use]
    pub fn surface(mut self, surface: impl Surface<()>) -> Self {
        self.surfaces.push(Box::new(surface));
        self
    }

    #[must_use]
    pub fn stop_deadline(mut self, deadline: Duration) -> Self {
        self.stop_deadline = deadline;
        self
    }

    /// Forward frames until `shutdown` resolves, the source ends or forwarding fails, then close
    /// the source and stop the surfaces. Surface failures do not stop forwarding.
    ///
    /// # Errors
    /// The thread could not be spawned (the source is closed), the forwarding
    /// failure, the source's close failure, or [`Error::Faulted`] if the thread
    /// did not stop in time.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let Self {
            source,
            sink,
            surfaces,
            stop_deadline,
        } = self;
        let spawned = Worker::spawn(
            "prysm-forwarder",
            (source, sink),
            |(mut source, mut sink), flags| {
                // A source that ends finishes forwarding without a failure.
                let failure = flags
                    .pump(&mut *source, |frame| match frame {
                        Some(frame) => sink.send(&frame),
                        None => sink.tick(),
                    })
                    .failure();
                flags.set_phase(SessionPhase::ClosingSource);
                let released = source.close();
                failure.map_or(released, Err)
            },
        );
        let mut worker = match spawned {
            Ok(worker) => worker,
            Err((error, (source, _))) => {
                // Release the source anyway; the spawn failure is the cause.
                let _ = tokio::task::spawn_blocking(move || source.close()).await;
                return Err(error);
            }
        };
        let mut surfaces = Surfaces::spawn(surfaces, &());
        tokio::pin!(shutdown);
        let ended = loop {
            tokio::select! {
                () = &mut shutdown => break None,
                end = worker.finished() => break Some(end),
                // Nobody to report to in a forwarder; the surface is simply gone.
                _ = surfaces.failure() => {}
            }
        };
        let result = match ended {
            Some(end) => {
                worker.join().await;
                end.and_then(|released| released)
            }
            None => match worker.stop(stop_deadline).await {
                Ok(end) => end.and_then(|released| released),
                Err(phase) => Err(Error::Faulted(phase)),
            },
        };
        surfaces.finish(stop_deadline).await;
        result
    }
}
