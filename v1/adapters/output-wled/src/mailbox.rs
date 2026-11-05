//! Atomic admission/observation state shared with nonblocking color producers.
use crate::{CAPACITY, CLOSED, admission::Admission};
use futures::StreamExt;
use prysm_contract::{
    ControlError,
    identity::{InstanceId, SessionId, StreamEpoch},
    observation::{Cursor, Observation, Update, Versioned},
    output::{ColorFrame, OfferResult, OutputState},
};
use std::sync::Mutex;
use tokio::sync::{Notify, broadcast};

struct State {
    admission: Admission,
    snapshot: Versioned<OutputState>,
    closed: bool,
}
pub(super) struct Mailbox {
    state: Mutex<State>,
    updates: broadcast::Sender<Option<Versioned<OutputState>>>,
    wake: Notify,
}
impl Mailbox {
    pub fn new(instance: InstanceId) -> Self {
        Self {
            state: Mutex::new(State {
                admission: Admission::new(instance),
                closed: false,
                snapshot: Versioned {
                    cursor: Cursor {
                        instance,
                        sequence: 0,
                    },
                    state: Self::unknown(),
                },
            }),
            updates: broadcast::channel(CAPACITY).0,
            wake: Notify::new(),
        }
    }
    pub fn unknown() -> OutputState {
        OutputState {
            admitted: None,
            held: false,
            idle_mode: None,
            confirmation: None,
            failure: None,
        }
    }
    pub fn observe(&self) -> Result<Observation<OutputState>, ControlError> {
        let state = self.state.lock().expect("output mailbox lock");
        if state.closed {
            return Err(CLOSED);
        }
        Ok(Observation {
            initial: state.snapshot.clone(),
            updates: futures::stream::unfold(Some(self.updates.subscribe()), |receiver| async {
                let mut receiver = receiver?;
                match receiver.recv().await {
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        Some((Ok(Update::ResyncRequired), None))
                    }
                    Ok(Some(snapshot)) => Some((Ok(Update::Snapshot(snapshot)), Some(receiver))),
                    Ok(None) | Err(broadcast::error::RecvError::Closed) => None,
                }
            })
            .boxed(),
        })
    }
    pub fn publish(&self, snapshot: OutputState) {
        let mut state = self.state.lock().expect("output mailbox lock");
        state.snapshot.cursor.sequence = state
            .snapshot
            .cursor
            .sequence
            .checked_add(1)
            .expect("observation sequence exhausted");
        state.snapshot.state = snapshot;
        let _ = self.updates.send(Some(state.snapshot.clone()));
    }
    pub fn close(&self) {
        let mut state = self.state.lock().expect("output mailbox lock");
        state.closed = true;
        state.admission.suspend();
        // The shared sender outlives the owner. End subscriptions explicitly.
        let _ = self.updates.send(None);
        self.wake.notify_one();
    }
    pub fn can_configure(&self) -> Result<(), ControlError> {
        self.state
            .lock()
            .expect("output mailbox lock")
            .admission
            .can_configure()
    }
    pub fn hold(&self, epoch: StreamEpoch) -> Result<(), ControlError> {
        self.state
            .lock()
            .expect("output mailbox lock")
            .admission
            .hold(epoch)
    }
    pub fn admit(&self, epoch: StreamEpoch) -> Result<(), ControlError> {
        self.state
            .lock()
            .expect("output mailbox lock")
            .admission
            .admit(epoch)
    }
    pub fn finish(&self, session: SessionId) -> Result<(), ControlError> {
        self.state
            .lock()
            .expect("output mailbox lock")
            .admission
            .finish(session)
    }
    pub fn finished(&self, session: SessionId) {
        self.state
            .lock()
            .expect("output mailbox lock")
            .admission
            .finished(session);
    }
    pub fn suspend(&self) {
        self.state
            .lock()
            .expect("output mailbox lock")
            .admission
            .suspend();
    }
    pub fn offer(&self, frame: ColorFrame) -> Result<OfferResult, ControlError> {
        let mut state = self.state.lock().expect("output mailbox lock");
        if state.closed {
            return Err(CLOSED);
        }
        let result = state.admission.offer(frame);
        drop(state);
        if result == OfferResult::Queued {
            self.wake.notify_one();
        }
        Ok(result)
    }
    pub fn take(&self) -> Option<ColorFrame> {
        self.state
            .lock()
            .expect("output mailbox lock")
            .admission
            .take()
    }
    pub async fn notified(&self) {
        self.wake.notified().await;
    }
}
