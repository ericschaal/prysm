//! Application intent, command acceptance and dispatch. Lighting sequencing is private
//! to Lifecycle; snapshots are projections, never mutable lifecycle storage.
use crate::{
    ApplicationWiring, Command,
    observations::{self, ComponentUpdate, Observed, Publisher},
    operations::Operations,
    policy::{ApplicationPolicy, FailureAction, UnavailableAction},
    session::{Lifecycle, Outcome, Requested},
};
use futures::StreamExt;
use prysm_contract::{
    ControlError, Failure, FailureKind, OwnerStopped,
    application::{
        Accepted, ApplicationCommand, ApplicationSnapshot, LightingTarget, Request, Settings,
    },
    calibration::CalibrationStatus,
    capture::CaptureControl,
    identity::{InstanceId, SettingsRevision},
    output::OutputControl,
    processing::ProcessingControl,
    storage::SettingsRepository,
};
use std::future::Future;
use tokio::sync::mpsc;

type AppSettings<C, O> = Settings<<C as CaptureControl>::Settings, <O as OutputControl>::Settings>;

struct Coordinator<C: CaptureControl, P, O: OutputControl> {
    instance: InstanceId,
    desired: LightingTarget,
    lifecycle: Lifecycle<C, P, O>,
    observed: Observed,
    operations: Operations,
    publication: Publisher<ApplicationSnapshot<AppSettings<C, O>>>,
    shutting_down: bool,
    fatal: Option<Failure>,
}

pub(super) async fn run<C, P, O, R, Policy>(
    wiring: ApplicationWiring<C, P, O, R, Policy>,
    mut commands: mpsc::Receiver<Command<AppSettings<C, O>>>,
    instance: InstanceId,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<OwnerStopped, ControlError>
where
    C: CaptureControl,
    P: ProcessingControl,
    O: OutputControl,
    R: SettingsRepository<AppSettings<C, O>>,
    Policy: ApplicationPolicy,
{
    let ApplicationWiring {
        capture,
        processing,
        output,
        settings,
        policy,
    } = wiring;
    let UnavailableAction::WaitHolding(waiting_colors) = policy.input_unavailable() else {
        return Err(ControlError::Unavailable(
            "end-session availability policy is not implemented",
        ));
    };
    if policy.initial_target() != LightingTarget::Off
        || policy.terminal_failure() != FailureAction::AwaitCommand
    {
        return Err(ControlError::Unavailable(
            "automatic start/retry policy is not implemented",
        ));
    }
    let loaded = settings.load();
    let initialize = async {
        let settings = loaded.await?.unwrap_or(Settings {
            capture: None,
            output: None,
            processing: Default::default(),
            profiles: vec![],
            active_profile: None,
        });
        settings.validate()?;
        let (observed, updates) = observations::connect(
            capture.clone(),
            processing.clone(),
            output.clone(),
            instance,
        )
        .await?;
        Ok::<_, ControlError>((settings, observed, updates))
    };
    tokio::pin!(shutdown);
    let (settings, observed, mut updates) = tokio::select! {
        _ = &mut shutdown => return Ok(OwnerStopped),
        result = initialize => result?,
    };
    let mut owner = Coordinator {
        instance,
        desired: LightingTarget::Off,
        lifecycle: Lifecycle::new(
            instance,
            (capture, processing, output),
            settings,
            waiting_colors,
        ),
        observed,
        operations: Operations::new(instance),
        publication: Publisher::new(instance),
        shutting_down: false,
        fatal: None,
    };
    let (mut commands_live, mut updates_live) = (true, true);
    loop {
        if owner.lifecycle.is_idle() && (owner.shutting_down || owner.fatal.is_some()) {
            return match owner.fatal {
                Some(failure) => Err(failure.into()),
                None => Ok(OwnerStopped),
            };
        }
        tokio::select! {
            _ = &mut shutdown, if !owner.shutting_down => owner.shutdown(),
            command = commands.recv(), if commands_live => match command {
                Some(command) => owner.command(command),
                None => commands_live = false,
            },
            outcome = owner.lifecycle.progress() => owner.progressed(outcome),
            update = updates.next(), if updates_live => match update {
                Some(update) => owner.observed(update),
                None => updates_live = false,
            },
        }
    }
}

impl<C: CaptureControl, P: ProcessingControl, O: OutputControl> Coordinator<C, P, O> {
    fn snapshot(&self) -> ApplicationSnapshot<AppSettings<C, O>> {
        ApplicationSnapshot {
            instance: self.instance,
            settings: self.lifecycle.settings().clone(),
            saved_revision: SettingsRevision(0),
            configured_revision: self.observed.processing.configured_revision,
            applied_revision: self.observed.processing.applied_revision,
            desired: self.desired,
            session: self.lifecycle.status(),
            capture: self.observed.capture_capabilities.clone(),
            output: self.observed.output_capabilities.clone(),
            observed_capture: Some(self.observed.capture.clone()),
            observed_output: Some(self.observed.output.clone()),
            calibration: CalibrationStatus::Idle,
        }
    }
    fn publish(&mut self) {
        self.publication.snapshot(self.snapshot());
    }
    fn finish_operation(&mut self, failure: Option<Failure>) {
        if let Some(event) = self.operations.finish(failure) {
            self.publication.event(event);
        }
    }
    fn command(&mut self, command: Command<AppSettings<C, O>>) {
        match command {
            Command::Submit(request, reply) => {
                let _ = reply.send(self.submit(request));
            }
            Command::Observe(reply) => {
                let _ = reply.send(Ok(self.publication.observe(self.snapshot())));
            }
            Command::Operation(id, reply) => {
                let _ = reply.send(self.operations.status(id));
            }
        }
    }
    fn submit(
        &mut self,
        request: Request<ApplicationCommand<AppSettings<C, O>>>,
    ) -> Result<Accepted, ControlError> {
        let target = match request.command {
            ApplicationCommand::SetTarget(target) => target,
            ApplicationCommand::ReplaceSettings { settings, .. } => {
                settings.validate()?;
                return Err(ControlError::Unavailable(
                    "settings replacement is not implemented",
                ));
            }
        };
        if self.operations.recognizes(request.id, target)? {
            return Ok(Accepted {
                operation: request.id,
            });
        }
        if (self.shutting_down || self.fatal.is_some()) && target != LightingTarget::Off {
            return Err(ControlError::Unavailable("application is shutting down"));
        }
        let result = self.lifecycle.request(target)?;
        for event in self.operations.accept(request.id, target) {
            self.publication.event(event);
        }
        self.desired = target;
        if matches!(result, Requested::Complete) {
            self.finish_operation(None);
        }
        self.publish();
        Ok(Accepted {
            operation: request.id,
        })
    }
    fn shutdown(&mut self) {
        self.shutting_down = true;
        self.desired = LightingTarget::Off;
        self.finish_operation(Some(Failure::new(
            FailureKind::Interrupted,
            "interrupted by shutdown",
        )));
        self.lifecycle.shutdown();
        self.publish();
    }
    fn progressed(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Changed => {}
            Outcome::Ready => self.finish_operation(None),
            Outcome::Failed => self.desired = LightingTarget::Off,
            Outcome::Stopped(failure) => self.finish_operation(failure),
            Outcome::Blocked(failure) => self.finish_operation(Some(failure)),
        }
        self.publish();
    }
    fn fault(&mut self, failure: Failure) {
        self.desired = LightingTarget::Off;
        self.lifecycle.fail(failure);
    }
    fn observed(&mut self, update: ComponentUpdate) {
        match update {
            ComponentUpdate::Capture(state) => {
                if self.lifecycle.capture_changed(&state) {
                    self.desired = LightingTarget::Off;
                }
                self.observed.capture = state;
            }
            ComponentUpdate::Output(state) => {
                if let Some(failure) = &state.failure {
                    self.fault(failure.clone());
                }
                self.observed.output = state;
            }
            ComponentUpdate::Processing(state) => {
                if let Some(failure) = &state.failure {
                    self.fault(failure.clone());
                }
                self.observed.processing = state;
            }
            ComponentUpdate::Lost(component, error) => {
                let message = format!("{component}: {error}");
                let mut failure = Failure::from(error);
                failure.message = message;
                self.fatal = Some(failure.clone());
                self.fault(failure);
            }
        }
        self.publish();
    }
}
