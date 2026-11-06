//! Composition lifetime only. The application owns all session transitions.
use super::{
    lifecycle::Lifecycle,
    owners::{Owner, Owners},
};
use futures::{FutureExt, Stream, StreamExt, future::BoxFuture};
use prysm_contract::{
    ControlError, Failure, OwnerStopped,
    application::{
        Accepted, ApplicationCommand, ApplicationControl, ApplicationEvent, ApplicationObservation,
        ApplicationSnapshot, LightingTarget, Request,
    },
    identity::{InstanceId, OperationId},
    observation::Update,
};
use std::{
    future::{Future, pending},
    num::NonZeroU64,
};
use tokio::time::sleep_until;

#[derive(Debug)]
pub enum HostEvent {
    Ready(InstanceId),
    Stopping,
    Application(Box<Update<ApplicationSnapshot<()>, ApplicationEvent>>),
    Accepted(Accepted),
    CommandRejected(Failure),
    Failure(Failure),
    CleanupOverdue { pending: Vec<&'static str> },
}

#[derive(Debug)]
pub struct HostError {
    pub failures: Vec<Failure>,
}
impl HostError {
    pub(super) fn configuration(error: ControlError) -> Self {
        Self {
            failures: vec![error.into()],
        }
    }
}
impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, failure) in self.failures.iter().enumerate() {
            if index != 0 {
                write!(f, "; ")?;
            }
            write!(f, "{failure}")?;
        }
        Ok(())
    }
}
impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.failures.first().map(|failure| failure as _)
    }
}

pub(super) async fn run<C: ApplicationControl>(
    mut owners: Owners,
    control: C,
    mut commands: impl Stream<Item = LightingTarget> + Unpin + Send,
    shutdown: impl Future<Output = ()> + Send,
    mut report: impl FnMut(HostEvent),
) -> Result<OwnerStopped, HostError> {
    let mut lifecycle = Lifecycle::new();
    let mut observing = Some(observe(control.clone()));
    let mut observation: Option<ApplicationObservation<C::Settings>> = None;
    let mut request: Option<BoxFuture<'static, Result<Accepted, ControlError>>> = None;
    let mut instance = None;
    let mut next_id = Some(NonZeroU64::MIN);
    let mut commands_live = true;
    tokio::pin!(shutdown);
    loop {
        if owners.pending().is_empty() {
            return lifecycle.result();
        }
        let accepting = lifecycle.accepting();
        tokio::select! {
            biased;
            result = owners.application.completion() => lifecycle.exited(Owner::Application, result, &mut owners, &mut report),
            result = owners.capture.completion() => lifecycle.exited(Owner::Capture, result, &mut owners, &mut report),
            result = owners.flow.completion() => lifecycle.exited(Owner::Processing, result, &mut owners, &mut report),
            result = owners.output.completion() => lifecycle.exited(Owner::Output, result, &mut owners, &mut report),
            _ = &mut shutdown, if lifecycle.observing() => {
                report(HostEvent::Stopping);
                lifecycle.stop(&mut owners);
            }
            _ = async { match lifecycle.deadline() { Some(at) => sleep_until(at).await, None => pending().await } } => {
                lifecycle.timed_out(&mut owners, &mut report);
            }
            result = optional(&mut observing), if lifecycle.observing() => {
                observing = None;
                match result {
                    Ok(connected) => {
                        if instance.is_some_and(|expected| expected != connected.initial.state.instance) {
                            lifecycle.failure("application observation", ControlError::Conflict("application instance changed"), &mut report);
                            lifecycle.terminate(&mut owners);
                            continue;
                        }
                        instance = Some(connected.initial.state.instance);
                        report(HostEvent::Application(Box::new(project(Update::Snapshot(connected.initial.clone())))));
                        observation = Some(connected);
                        if lifecycle.initialized() {
                            report(HostEvent::Ready(instance.expect("initialized instance")));
                        }
                    }
                    Err(error) => {
                        lifecycle.failure("application observation", error, &mut report);
                        lifecycle.terminate(&mut owners);
                    }
                }
            }
            update = async { match &mut observation { Some(o) => o.updates.next().await, None => pending().await } }, if accepting => {
                match update {
                    Some(Ok(Update::ResyncRequired)) => {
                        observation = None;
                        observing = Some(observe(control.clone()));
                    }
                    Some(Ok(update)) => report(HostEvent::Application(Box::new(project(update)))),
                    failed => {
                        let error = failed.and_then(Result::err).unwrap_or(ControlError::Unavailable("application observation closed"));
                        lifecycle.failure("application observation", error, &mut report);
                        lifecycle.terminate(&mut owners);
                    }
                }
            }
            result = optional(&mut request), if accepting => {
                request = None;
                match result { Ok(accepted) => report(HostEvent::Accepted(accepted)), Err(error) => report(HostEvent::CommandRejected(error.into())) }
            }
            target = commands.next(), if accepting && commands_live && request.is_none() => {
                if let Some(target) = target {
                    if let Some(id) = next_id {
                        next_id = id.get().checked_add(1).and_then(NonZeroU64::new);
                        let control = control.clone();
                        let id = OperationId { instance: instance.expect("ready instance"), request: id };
                        request = Some(async move { control.submit(Request { id, command: ApplicationCommand::SetTarget(target) }).await }.boxed());
                    } else {
                        report(HostEvent::CommandRejected(ControlError::Unavailable("operation ids exhausted").into()));
                    }
                } else { commands_live = false; }
            }
        }
        if !lifecycle.observing() {
            request = None;
            observing = None;
            observation = None;
        }
    }
}

fn observe<C: ApplicationControl>(
    control: C,
) -> BoxFuture<'static, Result<ApplicationObservation<C::Settings>, ControlError>> {
    async move { control.observe().await }.boxed()
}

fn project<S>(
    update: Update<ApplicationSnapshot<S>, ApplicationEvent>,
) -> Update<ApplicationSnapshot<()>, ApplicationEvent> {
    match update {
        Update::Snapshot(snapshot) => {
            let s = snapshot.state;
            Update::Snapshot(prysm_contract::observation::Versioned {
                cursor: snapshot.cursor,
                state: ApplicationSnapshot {
                    instance: s.instance,
                    settings: (),
                    saved_revision: s.saved_revision,
                    configured_revision: s.configured_revision,
                    applied_revision: s.applied_revision,
                    desired: s.desired,
                    session: s.session,
                    capture: s.capture,
                    output: s.output,
                    observed_capture: s.observed_capture,
                    observed_output: s.observed_output,
                    calibration: s.calibration,
                },
            })
        }
        Update::Event { cursor, event } => Update::Event { cursor, event },
        Update::ResyncRequired => Update::ResyncRequired,
    }
}
async fn optional<T>(future: &mut Option<BoxFuture<'static, T>>) -> T {
    match future {
        Some(future) => future.await,
        None => pending().await,
    }
}

#[cfg(test)]
mod tests;
