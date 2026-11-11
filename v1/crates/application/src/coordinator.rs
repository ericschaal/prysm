//! Application intent, command acceptance and dispatch. Lighting sequencing is private
//! to Lifecycle; snapshots are projections, never mutable lifecycle storage.
use crate::{
    ApplicationWiring, Command,
    observations::{self, ComponentUpdate, Observed, Publisher},
    operations::Operations,
    policy::{ApplicationPolicy, FailureAction, UnavailableAction},
    session::{Lifecycle, Outcome, Progress, Requested},
    settings::{Saved, SettingsWorkflow},
};
use futures::StreamExt;
use prysm_contract::{
    ControlError, Failure, FailureKind, OwnerStopped,
    application::{
        Accepted, ApplicationCommand, ApplicationSnapshot, LightingTarget, Request, Settings,
    },
    calibration::CalibrationStatus,
    capture::CaptureControl,
    identity::{InstanceId, OperationId},
    output::OutputControl,
    processing::ProcessingControl,
    storage::SettingsRepository,
};
use std::future::Future;
use tokio::sync::mpsc;

type AppSettings<C, O> = Settings<<C as CaptureControl>::Settings, <O as OutputControl>::Settings>;

struct Coordinator<C: CaptureControl, P, O: OutputControl, R> {
    instance: InstanceId,
    desired: LightingTarget,
    lifecycle: Lifecycle<C, P, O>,
    observed: Observed,
    operations: Operations<AppSettings<C, O>>,
    lighting_operation: Option<OperationId>,
    settings: SettingsWorkflow<AppSettings<C, O>, R>,
    publication: Publisher<ApplicationSnapshot<AppSettings<C, O>>>,
    shutting_down: bool,
    fatal: Option<Failure>,
    shutdown_failure: Option<Failure>,
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
    C::Settings: PartialEq,
    O::Settings: PartialEq,
{
    let ApplicationWiring {
        capture,
        processing,
        output,
        settings: repository,
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
    let loaded = repository.load();
    let initialize = async {
        let settings = loaded.await?.unwrap_or(Settings {
            capture: None,
            output: None,
            processing: Default::default(),
            profiles: vec![],
            active_profile: None,
        });
        settings.validate()?;
        if let Some(value) = &settings.capture {
            capture.validate(value)?;
        }
        if let Some(value) = &settings.output {
            output.validate(value)?;
        }
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
            settings.clone(),
            waiting_colors,
        ),
        observed,
        operations: Operations::new(instance),
        lighting_operation: None,
        settings: SettingsWorkflow::new(settings, repository),
        publication: Publisher::new(instance),
        shutting_down: false,
        fatal: None,
        shutdown_failure: None,
    };
    let (mut commands_live, mut updates_live) = (true, true);
    loop {
        if owner.lifecycle.is_idle()
            && !owner.settings.busy()
            && (owner.shutting_down || owner.fatal.is_some())
        {
            return match owner.fatal.or(owner.shutdown_failure) {
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
            progress = owner.lifecycle.progress() => {
                owner.progressed(progress);
                owner.publish();
            },
            saved = owner.settings.progress() => owner.saved(saved),
            update = updates.next(), if updates_live => match update {
                Some(update) => owner.observed(update),
                None => updates_live = false,
            },
        }
    }
}

impl<
    C: CaptureControl,
    P: ProcessingControl,
    O: OutputControl,
    R: SettingsRepository<AppSettings<C, O>>,
> Coordinator<C, P, O, R>
where
    C::Settings: PartialEq,
    O::Settings: PartialEq,
{
    fn snapshot(&self) -> ApplicationSnapshot<AppSettings<C, O>> {
        ApplicationSnapshot {
            instance: self.instance,
            settings: self.settings.saved().clone(),
            saved_revision: self.settings.revision(),
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
        if let Some(id) = self.lighting_operation.take() {
            self.publication.event(self.operations.finish(id, failure));
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
        if self.operations.recognizes(request.id, &request.command)? {
            return Ok(Accepted {
                operation: request.id,
            });
        }
        let off = matches!(
            request.command,
            ApplicationCommand::SetTarget(LightingTarget::Off)
        );
        if (self.shutting_down || self.fatal.is_some()) && !off {
            return Err(ControlError::Unavailable("application is shutting down"));
        }
        match &request.command {
            ApplicationCommand::ReplaceSettings { expected, settings } => {
                let revision = self.settings.next_revision(*expected)?;
                self.lifecycle
                    .validate_replacement(settings, self.desired)?;
                self.settings.begin(request.id, settings.clone(), revision);
                self.publication
                    .event(self.operations.accept(request.id, request.command));
            }
            ApplicationCommand::SetTarget(target) => {
                let target = *target;
                let result = if self.settings.busy() {
                    if !off || self.lighting_operation.is_some() {
                        return Err(ControlError::Conflict(
                            "settings replacement is in progress",
                        ));
                    }
                    self.settings.interrupt(Failure::new(
                        FailureKind::Interrupted,
                        "settings application interrupted by Off",
                    ));
                    let progress = self.lifecycle.stop();
                    self.progressed(progress);
                    if self.lifecycle.is_idle() {
                        Requested::Complete
                    } else {
                        Requested::Pending
                    }
                } else {
                    self.lifecycle
                        .install_saved_if_off(self.settings.saved(), self.settings.revision());
                    self.lifecycle.request(target)?
                };
                self.finish_operation(Some(Failure::new(
                    FailureKind::Interrupted,
                    "superseded by stop",
                )));
                self.publication
                    .event(self.operations.accept(request.id, request.command));
                self.lighting_operation = Some(request.id);
                self.desired = target;
                if matches!(result, Requested::Complete) {
                    self.finish_operation(None);
                }
            }
        }
        self.publish();
        Ok(Accepted {
            operation: request.id,
        })
    }
    fn saved(&mut self, saved: Saved) {
        // Publish durability before any device application can begin.
        self.publish();
        match saved {
            Saved::Committed => {
                let progress = self.lifecycle.replace(
                    self.settings.saved().clone(),
                    self.settings.revision(),
                    self.desired,
                );
                self.progressed(progress);
            }
            Saved::Finished(id, failure) => {
                self.retain_shutdown_failure(&failure);
                if failure.kind == FailureKind::RecoveryRequired {
                    self.fatal = Some(failure.clone());
                    self.fault(failure.clone());
                }
                self.publication
                    .event(self.operations.finish(id, Some(failure)));
            }
        }
        self.publish();
    }
    fn shutdown(&mut self) {
        self.shutting_down = true;
        self.settings.interrupt(Failure::new(
            FailureKind::Interrupted,
            "settings application interrupted by shutdown",
        ));
        self.desired = LightingTarget::Off;
        self.finish_operation(Some(Failure::new(
            FailureKind::Interrupted,
            "interrupted by shutdown",
        )));
        let progress = self.lifecycle.stop();
        self.progressed(progress);
        self.publish();
    }
    fn retain_shutdown_failure(&mut self, failure: &Failure) {
        if self.shutting_down && failure.kind != FailureKind::Interrupted {
            self.shutdown_failure.get_or_insert_with(|| failure.clone());
        }
    }
    fn progressed(&mut self, progress: Progress) {
        match &progress.outcome {
            Outcome::Failed(failure)
            | Outcome::Blocked(failure)
            | Outcome::Stopped(Some(failure)) => {
                self.retain_shutdown_failure(failure);
            }
            _ => {}
        }
        if let Some(Err(failure)) = &progress.settings {
            self.retain_shutdown_failure(failure);
        }
        match progress.outcome {
            Outcome::Changed => {}
            Outcome::Ready => self.finish_operation(None),
            Outcome::Failed(failure) => {
                self.desired = LightingTarget::Off;
                self.settings.interrupt(failure);
            }
            Outcome::Stopped(failure) => {
                if let Some(failure) = &failure {
                    self.settings.interrupt(failure.clone());
                }
                self.finish_operation(failure);
            }
            Outcome::Blocked(failure) => {
                self.desired = LightingTarget::Off;
                self.settings.interrupt(failure.clone());
                self.finish_operation(Some(failure));
            }
        }
        if let Some(result) = progress.settings {
            if result.is_err() {
                self.desired = LightingTarget::Off;
            }
            let (id, failure) = self.settings.applied(result);
            self.publication.event(self.operations.finish(id, failure));
        }
    }
    fn fault(&mut self, failure: Failure) {
        let progress = self.lifecycle.fail(failure);
        self.progressed(progress);
    }
    fn observed(&mut self, update: ComponentUpdate) {
        match update {
            ComponentUpdate::Capture(state) => {
                let progress = self.lifecycle.capture_changed(&state);
                self.observed.capture = state;
                self.progressed(progress);
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
