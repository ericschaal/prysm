//! Atomic admission/observation state shared with nonblocking color producers.
use crate::{CAPACITY, CLOSED};
use futures::StreamExt;
use prysm_contract::{
    ControlError,
    identity::{InstanceId, SessionId, StreamEpoch},
    observation::{Cursor, Observation, Update, Versioned},
    output::{ColorFrame, OfferResult, OutputState},
};
use prysm_output::Admission;
use std::sync::Mutex;
use tokio::sync::{Notify, broadcast};

struct State {
    admission: Admission,
    snapshot: Versioned<OutputState>,
    closed: bool,
    stopping: bool,
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
                stopping: false,
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
        let mut snapshot = snapshot;
        if state.stopping {
            snapshot.admitted = None;
            snapshot.held = false;
        }
        state.snapshot.cursor.sequence = state
            .snapshot
            .cursor
            .sequence
            .checked_add(1)
            .expect("observation sequence exhausted");
        state.snapshot.state = snapshot;
        let _ = self.updates.send(Some(state.snapshot.clone()));
    }
    pub fn request_shutdown(&self) {
        let mut state = self.state.lock().expect("output mailbox lock");
        if state.stopping || state.closed {
            return;
        }
        state.stopping = true;
        state.admission.suspend();
        state.snapshot.state.admitted = None;
        state.snapshot.state.held = false;
        state.snapshot.state.confirmation = None;
        state.snapshot.cursor.sequence = state
            .snapshot
            .cursor
            .sequence
            .checked_add(1)
            .expect("observation sequence exhausted");
        let _ = self.updates.send(Some(state.snapshot.clone()));
        self.wake.notify_one();
    }
    pub fn stopping(&self) -> bool {
        self.state.lock().expect("output mailbox lock").stopping
    }
    /// The single writer claims its next write under the terminal fence's lock.
    /// A claimed write remains owned even if shutdown races its first poll.
    pub fn authorize(&self) -> Result<(), ControlError> {
        self.state.lock().expect("output mailbox lock").accepting()
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
        let state = self.state.lock().expect("output mailbox lock");
        state.accepting()?;
        state.admission.can_configure()
    }
    pub fn hold(&self, epoch: StreamEpoch) -> Result<(), ControlError> {
        let mut state = self.state.lock().expect("output mailbox lock");
        state.accepting()?;
        state.admission.hold(epoch)
    }
    pub fn admit(&self, epoch: StreamEpoch) -> Result<(), ControlError> {
        let mut state = self.state.lock().expect("output mailbox lock");
        state.accepting()?;
        state.admission.admit(epoch)
    }
    pub fn finish(&self, session: SessionId) -> Result<(), ControlError> {
        let mut state = self.state.lock().expect("output mailbox lock");
        state.accepting()?;
        state.admission.finish(session)
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
        state.accepting()?;
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

impl State {
    fn accepting(&self) -> Result<(), ControlError> {
        if self.closed || self.stopping {
            Err(CLOSED)
        } else {
            Ok(())
        }
    }
}
