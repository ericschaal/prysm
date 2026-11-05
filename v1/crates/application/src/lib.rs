//! Serialized lighting intent with owned, asynchronous device operations.
mod coordinator;
mod observations;
mod operations;
pub mod policy;
mod session;

use policy::ApplicationPolicy;
use prysm_contract::{
    ControlError, OwnerStopped,
    application::{
        Accepted, ApplicationCommand, ApplicationControl, ApplicationObservation, OperationStatus,
        Request, Settings,
    },
    calibration::{CalibrationCommand, CalibrationReply},
    capture::CaptureControl,
    identity::{InstanceId, OperationId},
    output::OutputControl,
    processing::ProcessingControl,
    storage::SettingsRepository,
};
use std::future::Future;
use tokio::sync::{mpsc, oneshot};

const CAPACITY: usize = 16;
const CLOSED: ControlError = ControlError::Unavailable("application owner closed");
type Reply<T> = oneshot::Sender<Result<T, ControlError>>;

enum Command<S> {
    Submit(Request<ApplicationCommand<S>>, Reply<Accepted>),
    Observe(Reply<ApplicationObservation<S>>),
    Operation(OperationId, Reply<OperationStatus>),
}

pub struct ApplicationWiring<C, P, O, R, Policy> {
    pub capture: C,
    pub processing: P,
    pub output: O,
    pub settings: R,
    pub policy: Policy,
}

pub struct ApplicationOwner<C: CaptureControl, P, O: OutputControl, R, Policy> {
    pub wiring: ApplicationWiring<C, P, O, R, Policy>,
    commands: mpsc::Receiver<Command<Settings<C::Settings, O::Settings>>>,
}

pub struct Application<C: CaptureControl, P, O: OutputControl, R, Policy> {
    pub owner: ApplicationOwner<C, P, O, R, Policy>,
    pub control: ApplicationHandle<Settings<C::Settings, O::Settings>>,
}

impl<C, P, O, R, Policy> Application<C, P, O, R, Policy>
where
    C: CaptureControl,
    P: ProcessingControl,
    O: OutputControl,
    R: SettingsRepository<Settings<C::Settings, O::Settings>>,
    Policy: ApplicationPolicy,
{
    pub fn new(capture: C, processing: P, output: O, settings: R, policy: Policy) -> Self {
        let (sender, commands) = mpsc::channel(CAPACITY);
        Self {
            owner: ApplicationOwner {
                wiring: ApplicationWiring {
                    capture,
                    processing,
                    output,
                    settings,
                    policy,
                },
                commands,
            },
            control: ApplicationHandle(sender),
        }
    }
}

impl<C, P, O, R, Policy> ApplicationOwner<C, P, O, R, Policy>
where
    C: CaptureControl,
    P: ProcessingControl,
    O: OutputControl,
    R: SettingsRepository<Settings<C::Settings, O::Settings>>,
    Policy: ApplicationPolicy,
{
    /// Use the host's instance identity, shared with the flow. Shutdown requests
    /// coordinated cleanup; the host retains this future and emergency controls.
    /// Unconfirmed cleanup keeps this owner alive; an explicit Off may retry it.
    pub async fn run(
        self,
        instance: InstanceId,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        coordinator::run(self.wiring, self.commands, instance, shutdown).await
    }
}

#[derive(Clone)]
pub struct ApplicationHandle<S>(mpsc::Sender<Command<S>>);

impl<S> ApplicationHandle<S> {
    async fn request<T>(
        &self,
        command: impl FnOnce(Reply<T>) -> Command<S>,
    ) -> Result<T, ControlError> {
        let (reply, result) = oneshot::channel();
        self.0.send(command(reply)).await.map_err(|_| CLOSED)?;
        result.await.map_err(|_| CLOSED)?
    }
}

impl<S: Clone + Send + Sync + 'static> ApplicationControl for ApplicationHandle<S> {
    type Settings = S;
    async fn submit(
        &self,
        request: Request<ApplicationCommand<S>>,
    ) -> Result<Accepted, ControlError> {
        self.request(|reply| Command::Submit(request, reply)).await
    }
    async fn observe(&self) -> Result<ApplicationObservation<S>, ControlError> {
        self.request(Command::Observe).await
    }
    async fn operation(&self, id: OperationId) -> Result<OperationStatus, ControlError> {
        self.request(|reply| Command::Operation(id, reply)).await
    }
    async fn calibrate(
        &self,
        _request: Request<CalibrationCommand>,
    ) -> Result<CalibrationReply, ControlError> {
        Err(ControlError::Unavailable(
            "skeleton: calibration is not implemented",
        ))
    }
}

#[cfg(test)]
mod tests;
