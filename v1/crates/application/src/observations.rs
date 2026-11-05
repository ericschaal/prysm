//! Atomic application publication and recovery of component observation gaps.
use futures::{StreamExt, stream::BoxStream};
use prysm_contract::{
    ControlError,
    application::ApplicationEvent,
    capture::{CaptureCapabilities, CaptureControl, CaptureState},
    identity::InstanceId,
    observation::{Cursor, Observation, Update, Versioned},
    output::{OutputCapabilities, OutputControl, OutputState},
    processing::{ProcessingControl, ProcessingState},
};
use std::future::Future;
use tokio::sync::broadcast;

pub(super) struct Publisher<S> {
    cursor: Cursor,
    updates: broadcast::Sender<Update<S, ApplicationEvent>>,
}
impl<S: Clone + Send + 'static> Publisher<S> {
    pub fn new(instance: InstanceId) -> Self {
        Self {
            cursor: Cursor {
                instance,
                sequence: 0,
            },
            updates: broadcast::channel(crate::CAPACITY).0,
        }
    }
    fn advance(&mut self) {
        self.cursor.sequence = self
            .cursor
            .sequence
            .checked_add(1)
            .expect("observation sequence exhausted");
    }
    pub fn snapshot(&mut self, state: S) {
        self.advance();
        let _ = self.updates.send(Update::Snapshot(Versioned {
            cursor: self.cursor,
            state,
        }));
    }
    pub fn event(&mut self, event: ApplicationEvent) {
        self.advance();
        let _ = self.updates.send(Update::Event {
            cursor: self.cursor,
            event,
        });
    }
    pub fn observe(&self, state: S) -> Observation<S, ApplicationEvent> {
        Observation {
            initial: Versioned {
                cursor: self.cursor,
                state,
            },
            updates: futures::stream::unfold(Some(self.updates.subscribe()), |receiver| async {
                let mut receiver = receiver?;
                match receiver.recv().await {
                    Ok(update) => Some((Ok(update), Some(receiver))),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        Some((Ok(Update::ResyncRequired), None))
                    }
                    Err(broadcast::error::RecvError::Closed) => None,
                }
            })
            .boxed(),
        }
    }
}

pub(super) struct Observed {
    pub capture_capabilities: CaptureCapabilities,
    pub output_capabilities: OutputCapabilities,
    pub capture: CaptureState,
    pub output: OutputState,
    pub processing: ProcessingState,
}
pub(super) enum ComponentUpdate {
    Capture(CaptureState),
    Output(OutputState),
    Processing(ProcessingState),
    Lost(&'static str, ControlError),
}

pub(super) async fn connect<C: CaptureControl, P: ProcessingControl, O: OutputControl>(
    capture: C,
    processing: P,
    output: O,
    instance: InstanceId,
) -> Result<(Observed, BoxStream<'static, ComponentUpdate>), ControlError> {
    let capture_observation = capture.observe().await?;
    let output_observation = output.observe().await?;
    let processing_observation = processing.observe().await?;
    for cursor in [
        capture_observation.initial.cursor,
        output_observation.initial.cursor,
        processing_observation.initial.cursor,
    ] {
        if cursor.instance != instance {
            return Err(ControlError::Conflict(
                "component belongs to another application instance",
            ));
        }
    }
    let observed = Observed {
        capture_capabilities: capture.capabilities(),
        output_capabilities: output.capabilities(),
        capture: capture_observation.initial.state.clone(),
        output: output_observation.initial.state.clone(),
        processing: processing_observation.initial.state.clone(),
    };
    if let Some(failure) = observed
        .processing
        .failure
        .as_ref()
        .or(observed.output.failure.as_ref())
    {
        return Err(ControlError::Adapter(Box::new(std::io::Error::other(
            failure.message.clone(),
        ))));
    }
    let captures = states(capture_observation, instance, move || {
        let c = capture.clone();
        async move { c.observe().await }
    })
    .map(|result| {
        result.map_or_else(
            |error| ComponentUpdate::Lost("capture", error),
            ComponentUpdate::Capture,
        )
    })
    .boxed();
    let outputs = states(output_observation, instance, move || {
        let o = output.clone();
        async move { o.observe().await }
    })
    .map(|result| {
        result.map_or_else(
            |error| ComponentUpdate::Lost("output", error),
            ComponentUpdate::Output,
        )
    })
    .boxed();
    let processing = states(processing_observation, instance, move || {
        let p = processing.clone();
        async move { p.observe().await }
    })
    .map(|result| {
        result.map_or_else(
            |error| ComponentUpdate::Lost("processing", error),
            ComponentUpdate::Processing,
        )
    })
    .boxed();
    Ok((
        observed,
        futures::stream::select_all([captures, outputs, processing]).boxed(),
    ))
}

/// Gaps replace subscriptions; closure yields one error, then ends that input.
fn states<S: Clone + Send + 'static, F, Fut>(
    observation: Observation<S>,
    instance: InstanceId,
    observe: F,
) -> BoxStream<'static, Result<S, ControlError>>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<Observation<S>, ControlError>> + Send,
{
    futures::stream::try_unfold(
        (observation, observe),
        move |(mut observation, mut observe)| async move {
            let snapshot = match observation.updates.next().await {
                Some(Ok(Update::Snapshot(snapshot))) => snapshot,
                Some(Ok(Update::ResyncRequired)) => {
                    observation = observe().await?;
                    if observation.initial.cursor.instance != instance {
                        return Err(ControlError::Conflict("component instance changed"));
                    }
                    return Ok(Some((
                        observation.initial.state.clone(),
                        (observation, observe),
                    )));
                }
                Some(Ok(Update::Event { event, .. })) => match event {},
                Some(Err(error)) => return Err(error),
                None => return Err(ControlError::Unavailable("component observation closed")),
            };
            if snapshot.cursor.instance != instance {
                return Err(ControlError::Conflict("component instance changed"));
            }
            Ok(Some((snapshot.state, (observation, observe))))
        },
    )
    .boxed()
}
