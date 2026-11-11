//! Application observation and command submission; no owner lifecycle decisions.
use futures::{FutureExt, Stream, StreamExt, future::BoxFuture, stream::BoxStream};
use prysm_contract::{
    ControlError, Failure,
    application::{
        Accepted, ApplicationCommand, ApplicationControl, ApplicationEvent, ApplicationObservation,
        ApplicationSnapshot, Request,
    },
    identity::{InstanceId, OperationId},
    observation::{Update, Versioned},
};
use std::{future::pending, num::NonZeroU64};

pub(super) enum Event {
    SnapshotAcquired {
        instance: InstanceId,
        snapshot: Box<Update<ApplicationSnapshot<()>, ApplicationEvent>>,
    },
    Updated(Box<Update<ApplicationSnapshot<()>, ApplicationEvent>>),
    Accepted(Accepted),
    Rejected(Failure),
}

pub(super) struct Connection<C: ApplicationControl, S> {
    control: C,
    commands: Option<S>,
    observation: ObservationState<C::Settings>,
    request: Option<BoxFuture<'static, Result<Accepted, ControlError>>>,
    instance: Option<InstanceId>,
    next_id: Option<NonZeroU64>,
}

impl<C, S> Connection<C, S>
where
    C: ApplicationControl,
    S: Stream<Item = Result<ApplicationCommand<C::Settings>, ControlError>> + Unpin,
{
    pub fn new(control: C, commands: S) -> Self {
        Self {
            observation: ObservationState::Acquiring(observe(control.clone())),
            control,
            commands: Some(commands),
            request: None,
            instance: None,
            next_id: Some(NonZeroU64::MIN),
        }
    }

    /// Pending observe/submit calls survive cancellation when the supervisor handles an owner exit.
    /// Advance once so the host checks shutdown before polling again.
    /// None means a resync or submission started, or command input closed.
    pub async fn advance(&mut self) -> Result<Option<Event>, ControlError> {
        tokio::select! {
            // Fair selection keeps observation traffic from starving commands.
            event = self.observation.advance(&self.control, &mut self.instance) => return event,
            result = optional(&mut self.request) => {
                self.request = None;
                return Ok(Some(match result {
                    Ok(accepted) => Event::Accepted(accepted),
                    Err(error) => Event::Rejected(error.into()),
                }));
            }
            command = next_command(&mut self.commands), if self.instance.is_some() && self.request.is_none() => {
                match command {
                    Some(Ok(command)) => {
                        if let Err(error) = self.submit(command) {
                            return Ok(Some(Event::Rejected(error.into())));
                        }
                    }
                    Some(Err(error)) => return Ok(Some(Event::Rejected(error.into()))),
                    None => self.commands = None,
                }
            }
        }
        Ok(None)
    }

    fn submit(&mut self, command: ApplicationCommand<C::Settings>) -> Result<(), ControlError> {
        let request = self
            .next_id
            .ok_or(ControlError::Unavailable("operation ids exhausted"))?;
        self.next_id = request.get().checked_add(1).and_then(NonZeroU64::new);
        let id = OperationId {
            instance: self.instance.expect("commands wait for initialization"),
            request,
        };
        let control = self.control.clone();
        self.request = Some(async move { control.submit(Request { id, command }).await }.boxed());
        Ok(())
    }

    /// Close input immediately; accepted operations remain owned by the application.
    pub fn close(&mut self) {
        self.commands = None;
        self.request = None;
        self.observation = ObservationState::Closed;
    }
}

fn observe<C: ApplicationControl>(
    control: C,
) -> BoxFuture<'static, Result<ApplicationObservation<C::Settings>, ControlError>> {
    async move { control.observe().await }.boxed()
}

enum ObservationState<S> {
    Acquiring(BoxFuture<'static, Result<ApplicationObservation<S>, ControlError>>),
    Observing(
        BoxStream<'static, Result<Update<ApplicationSnapshot<S>, ApplicationEvent>, ControlError>>,
    ),
    Closed,
}

impl<S> ObservationState<S> {
    async fn advance<C: ApplicationControl<Settings = S>>(
        &mut self,
        control: &C,
        instance: &mut Option<InstanceId>,
    ) -> Result<Option<Event>, ControlError> {
        match self {
            Self::Acquiring(acquiring) => {
                let connected = acquiring.await;
                *self = Self::Closed;
                let connected = connected?;
                let observed = connected.initial.state.instance;
                if instance.is_some_and(|expected| expected != observed) {
                    return Err(ControlError::Conflict("application instance changed"));
                }
                *instance = Some(observed);
                let snapshot = Box::new(project(Update::Snapshot(connected.initial)));
                *self = Self::Observing(connected.updates);
                Ok(Some(Event::SnapshotAcquired {
                    instance: observed,
                    snapshot,
                }))
            }
            Self::Observing(observation) => match observation.next().await {
                Some(Ok(Update::ResyncRequired)) => {
                    *self = Self::Acquiring(observe(control.clone()));
                    Ok(None)
                }
                Some(Ok(update)) => Ok(Some(Event::Updated(Box::new(project(update))))),
                result => {
                    *self = Self::Closed;
                    Err(match result {
                        Some(Err(error)) => error,
                        _ => ControlError::Unavailable("application observation closed"),
                    })
                }
            },
            Self::Closed => pending().await,
        }
    }
}

async fn next_command<S: Stream + Unpin>(commands: &mut Option<S>) -> Option<S::Item> {
    match commands {
        Some(commands) => commands.next().await,
        None => pending().await,
    }
}

async fn optional<T>(future: &mut Option<BoxFuture<'static, T>>) -> T {
    match future {
        Some(future) => future.await,
        None => pending().await,
    }
}

fn project<S>(
    update: Update<ApplicationSnapshot<S>, ApplicationEvent>,
) -> Update<ApplicationSnapshot<()>, ApplicationEvent> {
    match update {
        Update::Snapshot(snapshot) => Update::Snapshot(project_snapshot(snapshot)),
        Update::Event { cursor, event } => Update::Event { cursor, event },
        Update::ResyncRequired => Update::ResyncRequired,
    }
}

fn project_snapshot<S>(
    snapshot: Versioned<ApplicationSnapshot<S>>,
) -> Versioned<ApplicationSnapshot<()>> {
    let s = snapshot.state;
    Versioned {
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
    }
}
