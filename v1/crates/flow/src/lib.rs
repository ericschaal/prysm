//! Passive processing: one owned worker, one pending frame, and ordered control replies.
mod mailbox;
mod preview;
mod worker;

use mailbox::{Command, Mailbox};
pub use preview::PreviewHandle;
use prysm_contract::{
    ControlError, OwnerStopped,
    capture::FrameSource,
    identity::{InstanceId, SessionId},
    observation::Observation,
    output::ColorSink,
    processing::{Configured, ProcessingControl, ProcessingState, ProcessingUpdate, Quiescent},
};
use prysm_processor::PrysmProcessor;
use std::{future::Future, sync::Arc, thread::JoinHandle};
use tokio::sync::{mpsc, oneshot};

const CLOSED: ControlError = ControlError::Unavailable("processing owner closed");
const CAPACITY: usize = 16;

#[derive(Debug, Clone, Copy)]
pub enum ProcessingExecution {
    DedicatedThread,
}

/// The host connects data endpoints once; starting capture reuses this wiring.
struct FlowWiring<F, O> {
    frames: F,
    processor: PrysmProcessor,
    colors: O,
    execution: ProcessingExecution,
}

pub struct ProcessingFlow<F, O> {
    pub owner: FlowOwner<F, O>,
    pub control: ProcessingHandle,
    pub preview: PreviewHandle,
}

/// Owns the processor and connected data ports; interaction goes through the returned handles.
pub struct FlowOwner<F, O> {
    wiring: FlowWiring<F, O>,
    commands: mpsc::Receiver<Command>,
    mailbox: Arc<Mailbox>,
    preview: preview::Publisher,
}

impl<F: FrameSource, O: ColorSink> ProcessingFlow<F, O> {
    pub fn connect(
        frames: F,
        processor: PrysmProcessor,
        colors: O,
        execution: ProcessingExecution,
    ) -> Self {
        let (mailbox, commands) = Mailbox::new();
        let mailbox = Arc::new(mailbox);
        let (publisher, preview) = preview::Publisher::new();
        Self {
            owner: FlowOwner {
                wiring: FlowWiring {
                    frames,
                    processor,
                    colors,
                    execution,
                },
                commands,
                mailbox: mailbox.clone(),
                preview: publisher,
            },
            control: ProcessingHandle { mailbox },
            preview,
        }
    }
}

impl<F: FrameSource, O: ColorSink> FlowOwner<F, O> {
    /// Drive on host Tokio, using the same instance identity as the application.
    /// Configure before processing; until then only the newest frame is retained.
    /// Shutdown discards pending work and joins in-flight processing. The host must
    /// await this future: dropping it also joins, but synchronously as a fallback.
    pub async fn run(
        self,
        instance: InstanceId,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        let FlowWiring {
            mut frames,
            processor,
            colors,
            execution,
        } = self.wiring;
        let (done, mut finished) = oneshot::channel();
        let mailbox = self.mailbox;
        let worker_mailbox = mailbox.clone();
        let join = match execution {
            ProcessingExecution::DedicatedThread => std::thread::Builder::new()
                .name("prysm-processing".into())
                .spawn(move || {
                    let result = worker::run(
                        processor,
                        colors,
                        self.commands,
                        &worker_mailbox,
                        instance,
                        self.preview,
                    );
                    // A panic drops this sender too, so the dispatcher still joins.
                    let _ = done.send(());
                    result
                })
                .map_err(|error| ControlError::Adapter(Box::new(error)))?,
        };
        let worker = Worker {
            join: Some(join),
            mailbox: mailbox.clone(),
        };
        tokio::pin!(shutdown);
        let (result, worker_finished) = loop {
            // Controls go directly to the worker, so they never cancel an input read.
            tokio::select! {
                biased;
                _ = &mut shutdown => break (Ok(()), false),
                _ = &mut finished => break (Ok(()), true),
                frame = frames.next_frame() => match frame {
                    Ok(Some(frame)) => {
                        mailbox.offer(frame, instance);
                        tokio::task::consume_budget().await;
                    }
                    Ok(None) => break (Err(ControlError::Unavailable("capture frame owner closed")), false),
                    Err(error) => break (Err(error), false),
                }
            }
        };
        mailbox.stop();
        if !worker_finished {
            let _ = finished.await;
        }
        worker.join()?;
        result?;
        Ok(OwnerStopped)
    }
}

/// Retains the OS thread even if the host drops/aborts its driving future.
struct Worker {
    join: Option<JoinHandle<Result<(), ControlError>>>,
    mailbox: Arc<Mailbox>,
}

impl Worker {
    fn join(mut self) -> Result<(), ControlError> {
        self.join
            .take()
            .expect("worker is joined once")
            .join()
            .map_err(|panic| {
                let message = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                ControlError::Adapter(Box::new(std::io::Error::other(format!(
                    "processing worker panicked: {message}"
                ))))
            })?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.mailbox.stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Clone)]
pub struct ProcessingHandle {
    mailbox: Arc<Mailbox>,
}

impl ProcessingHandle {
    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, ControlError>>) -> Command,
    ) -> Result<T, ControlError> {
        let (reply, result) = oneshot::channel();
        self.mailbox.send(command(reply)).await?;
        result.await.map_err(|_| CLOSED)?
    }
}

impl ProcessingControl for ProcessingHandle {
    async fn configure(&self, update: ProcessingUpdate) -> Result<Configured, ControlError> {
        self.request(|reply| Command::Configure(update, reply))
            .await
    }
    async fn barrier(&self, session: SessionId) -> Result<Quiescent, ControlError> {
        self.request(|reply| Command::Barrier(session, reply)).await
    }
    async fn observe(&self) -> Result<Observation<ProcessingState>, ControlError> {
        self.request(Command::Observe).await
    }
}

#[cfg(test)]
mod tests;
