//! Dedicated-thread processing, command execution, and state publication.
use crate::{
    CAPACITY,
    mailbox::{Command, Mailbox},
};
use futures::StreamExt;
use prysm_contract::{
    ControlError, Failure,
    identity::InstanceId,
    observation::{Cursor, Observation, Update, Versioned},
    output::{ColorFrame, ColorSink},
    processing::{Configured, ProcessingState, Quiescent},
};
use prysm_processor::PrysmProcessor;
use tokio::sync::{broadcast, mpsc};

pub(super) fn run<O: ColorSink>(
    mut processor: PrysmProcessor,
    colors: O,
    mut commands: mpsc::Receiver<Command>,
    mailbox: &Mailbox,
    instance: InstanceId,
) -> Result<(), ControlError> {
    mailbox.register_worker();
    let mut snapshot = Versioned {
        cursor: Cursor {
            instance,
            sequence: 0,
        },
        state: ProcessingState {
            configured_revision: None,
            applied_revision: None,
            failure: None,
        },
    };
    let (updates, _) = broadcast::channel(CAPACITY);
    let mut config = None;
    let mut epoch = None;
    loop {
        if mailbox.is_stopping() {
            return Ok(());
        }
        if let Ok(command) = commands.try_recv() {
            match command {
                Command::Configure(update, reply) => {
                    processor.configure(&update.config);
                    config = Some(update.config);
                    snapshot.state.configured_revision = Some(update.revision);
                    publish(&mut snapshot, &updates);
                    let _ = reply.send(Ok(Configured {
                        revision: update.revision,
                    }));
                }
                Command::Barrier(session, reply) => {
                    let result = if session.instance != instance {
                        Err(ControlError::Conflict(
                            "barrier belongs to another application instance",
                        ))
                    } else {
                        mailbox.seal(session);
                        Ok(Quiescent { session })
                    };
                    let _ = reply.send(result);
                }
                Command::Observe(reply) => {
                    let observation = Observation {
                        initial: snapshot.clone(),
                        updates: futures::stream::unfold(
                            Some(updates.subscribe()),
                            |receiver| async {
                                let mut receiver = receiver?;
                                match receiver.recv().await {
                                    Ok(update) => Some((Ok(update), Some(receiver))),
                                    Err(broadcast::error::RecvError::Lagged(_)) => {
                                        Some((Ok(Update::ResyncRequired), None))
                                    }
                                    Err(broadcast::error::RecvError::Closed) => None,
                                }
                            },
                        )
                        .boxed(),
                    };
                    let _ = reply.send(Ok(observation));
                }
            }
            continue;
        }
        if let Some(config) = config {
            let frame = mailbox.take_frame();
            if let Some(frame) = frame {
                if epoch != Some(frame.stamp.epoch) {
                    processor = PrysmProcessor::new(&config);
                    epoch = Some(frame.stamp.epoch);
                }
                let revision = snapshot
                    .state
                    .configured_revision
                    .expect("configured worker");
                let frame = ColorFrame {
                    stamp: frame.stamp,
                    revision,
                    colors: processor.process_frame(frame.frame),
                };
                let revision_changed = snapshot.state.applied_revision != Some(revision);
                snapshot.state.applied_revision = Some(revision);
                if let Err(error) = colors.offer(frame) {
                    snapshot.state.failure = Some(Failure {
                        message: error.to_string(),
                    });
                    publish(&mut snapshot, &updates);
                    return Err(error);
                }
                if revision_changed {
                    publish(&mut snapshot, &updates);
                }
                continue;
            }
        }
        // unpark retains a wake token, including a command arriving just before park.
        std::thread::park();
    }
}

fn publish(
    snapshot: &mut Versioned<ProcessingState>,
    updates: &broadcast::Sender<Update<ProcessingState, std::convert::Infallible>>,
) {
    snapshot.cursor.sequence = snapshot
        .cursor
        .sequence
        .checked_add(1)
        .expect("observation sequence exhausted");
    let _ = updates.send(Update::Snapshot(snapshot.clone()));
}
