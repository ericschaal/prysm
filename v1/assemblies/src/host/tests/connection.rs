use super::*;
use crate::host::connection::{Connection, Event};
use futures::{FutureExt, channel::mpsc as input};

#[derive(Clone)]
struct GatedControl {
    observations: mpsc::UnboundedSender<oneshot::Sender<ApplicationObservation<TestSettings>>>,
    submissions: mpsc::UnboundedSender<(
        Request<ApplicationCommand<TestSettings>>,
        oneshot::Sender<Accepted>,
    )>,
}

impl ApplicationControl for GatedControl {
    type Settings = TestSettings;

    async fn observe(&self) -> Result<ApplicationObservation<TestSettings>, ControlError> {
        let (reply, response) = oneshot::channel();
        self.observations.send(reply).unwrap();
        Ok(response.await.unwrap())
    }

    async fn submit(
        &self,
        request: Request<ApplicationCommand<TestSettings>>,
    ) -> Result<Accepted, ControlError> {
        let (reply, response) = oneshot::channel();
        self.submissions.send((request, reply)).unwrap();
        Ok(response.await.unwrap())
    }

    async fn operation(&self, _: OperationId) -> Result<OperationStatus, ControlError> {
        unreachable!()
    }

    async fn calibrate(
        &self,
        _: Request<CalibrationCommand>,
    ) -> Result<CalibrationReply, ControlError> {
        unreachable!()
    }
}

#[tokio::test]
async fn pending_calls_survive_poll_cancellation_and_resync_preserves_request_ids() {
    let (observations, mut observing) = mpsc::unbounded_channel();
    let (submissions, mut submitting) = mpsc::unbounded_channel();
    let (commands, input) = input::unbounded();
    let control = GatedControl {
        observations,
        submissions,
    };
    let mut connection = Connection::new(control, input);
    commands
        .unbounded_send(Ok(ApplicationCommand::SetTarget(LightingTarget::Processed)))
        .unwrap();

    assert!(connection.advance().now_or_never().is_none());
    let initial = observing.try_recv().unwrap();
    assert!(connection.advance().now_or_never().is_none());
    assert!(observing.try_recv().is_err());
    assert!(submitting.try_recv().is_err());
    let (updates, stream) = input::unbounded();
    assert!(
        initial
            .send(Observation {
                initial: snapshot(),
                updates: stream.map(Ok).boxed()
            })
            .is_ok()
    );
    assert!(matches!(
        connection.advance().await.unwrap(),
        Some(Event::SnapshotAcquired { .. })
    ));

    assert!(connection.advance().await.unwrap().is_none());
    assert!(connection.advance().now_or_never().is_none());
    let (first, accepted) = submitting.try_recv().unwrap();
    assert_eq!(first.id.instance, INSTANCE);
    assert_eq!(first.id.request.get(), 1);
    assert!(connection.advance().now_or_never().is_none());
    assert!(submitting.try_recv().is_err());

    updates.unbounded_send(Update::ResyncRequired).unwrap();
    assert!(connection.advance().await.unwrap().is_none());
    assert!(connection.advance().now_or_never().is_none());
    let resync = observing.try_recv().unwrap();
    accepted
        .send(Accepted {
            operation: first.id,
        })
        .unwrap();
    assert!(
        matches!(connection.advance().await.unwrap(), Some(Event::Accepted(value)) if value.operation == first.id)
    );
    commands
        .unbounded_send(Ok(ApplicationCommand::SetTarget(LightingTarget::Off)))
        .unwrap();
    assert!(connection.advance().await.unwrap().is_none());
    assert!(connection.advance().now_or_never().is_none());
    let (second, accepted) = submitting.try_recv().unwrap();
    assert_eq!(second.id.instance, INSTANCE);
    assert_eq!(second.id.request.get(), 2);
    assert!(
        resync
            .send(Observation {
                initial: snapshot(),
                updates: stream::pending().boxed()
            })
            .is_ok()
    );
    assert!(matches!(
        connection.advance().await.unwrap(),
        Some(Event::SnapshotAcquired { .. })
    ));

    connection.close();
    assert!(commands.is_closed());
    assert!(accepted.is_closed());
    assert!(connection.advance().now_or_never().is_none());
}

#[tokio::test]
async fn changed_instance_or_closed_observation_fails_the_connection() {
    for changed_instance in [false, true] {
        let (observations, mut observing) = mpsc::unbounded_channel();
        let (submissions, _submitting) = mpsc::unbounded_channel();
        let mut connection = Connection::new(
            GatedControl {
                observations,
                submissions,
            },
            stream::pending(),
        );
        assert!(connection.advance().now_or_never().is_none());
        assert!(
            observing
                .try_recv()
                .unwrap()
                .send(Observation {
                    initial: snapshot(),
                    updates: stream::iter(changed_instance.then_some(Ok(Update::ResyncRequired)))
                        .boxed(),
                })
                .is_ok()
        );
        assert!(matches!(
            connection.advance().await.unwrap(),
            Some(Event::SnapshotAcquired { .. })
        ));
        if changed_instance {
            assert!(connection.advance().await.unwrap().is_none());
            assert!(connection.advance().now_or_never().is_none());
            let mut initial = snapshot();
            initial.state.instance = InstanceId(NonZeroU64::new(2).unwrap());
            initial.cursor.instance = initial.state.instance;
            assert!(
                observing
                    .try_recv()
                    .unwrap()
                    .send(Observation {
                        initial,
                        updates: stream::pending().boxed()
                    })
                    .is_ok()
            );
            assert!(matches!(
                connection.advance().await,
                Err(ControlError::Conflict("application instance changed"))
            ));
        } else {
            assert!(matches!(
                connection.advance().await,
                Err(ControlError::Unavailable("application observation closed"))
            ));
        }
        connection.close();
    }
}

#[tokio::test]
async fn busy_observation_does_not_starve_command_submission_or_acceptance() {
    let (observations, mut observing) = mpsc::unbounded_channel();
    let (submissions, mut submitting) = mpsc::unbounded_channel();
    let mut connection = Connection::new(
        GatedControl {
            observations,
            submissions,
        },
        stream::iter([Ok(ApplicationCommand::SetTarget(LightingTarget::Off))]),
    );
    assert!(connection.advance().now_or_never().is_none());
    assert!(
        observing
            .try_recv()
            .unwrap()
            .send(Observation {
                initial: snapshot(),
                updates: stream::repeat(Update::Snapshot(snapshot())).map(Ok).boxed(),
            })
            .is_ok()
    );
    assert!(matches!(
        connection.advance().await.unwrap(),
        Some(Event::SnapshotAcquired { .. })
    ));
    within(async {
        loop {
            if matches!(
                connection.advance().await.unwrap(),
                Some(Event::Accepted(_))
            ) {
                break;
            }
            if let Ok((request, reply)) = submitting.try_recv() {
                reply
                    .send(Accepted {
                        operation: request.id,
                    })
                    .unwrap();
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    connection.close();
}

#[tokio::test]
async fn input_eof_keeps_application_observation_alive() {
    let (observations, mut observing) = mpsc::unbounded_channel();
    let (submissions, _submitting) = mpsc::unbounded_channel();
    let mut connection = Connection::new(
        GatedControl {
            observations,
            submissions,
        },
        stream::empty(),
    );
    assert!(connection.advance().now_or_never().is_none());
    let (updates, stream) = input::unbounded();
    assert!(
        observing
            .try_recv()
            .unwrap()
            .send(Observation {
                initial: snapshot(),
                updates: stream.map(Ok).boxed(),
            })
            .is_ok()
    );
    assert!(matches!(
        connection.advance().await.unwrap(),
        Some(Event::SnapshotAcquired { .. })
    ));
    assert!(connection.advance().await.unwrap().is_none());
    updates
        .unbounded_send(Update::Snapshot(snapshot()))
        .unwrap();
    assert!(matches!(
        connection.advance().await.unwrap(),
        Some(Event::Updated(_))
    ));
    connection.close();
}
