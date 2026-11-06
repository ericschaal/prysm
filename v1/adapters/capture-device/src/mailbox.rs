//! One frame consumer and atomic observations; native work never holds this lock.
use crate::{CAPACITY, CLOSED};
use futures::StreamExt;
use prysm_contract::{
    ControlError,
    capture::{CaptureState, CapturedFrame},
    identity::InstanceId,
    observation::{Cursor, Observation, Update, Versioned},
};
use std::sync::Mutex;
use tokio::sync::{Notify, broadcast};

struct State {
    snapshot: Versioned<CaptureState>,
    frame: Option<CapturedFrame>,
    closed: bool,
}
pub(super) struct Mailbox {
    state: Mutex<State>,
    updates: broadcast::Sender<Option<Versioned<CaptureState>>>,
    wake: Notify,
}
impl Mailbox {
    pub fn new(instance: InstanceId) -> Self {
        Self {
            state: Mutex::new(State {
                snapshot: Versioned {
                    cursor: Cursor {
                        instance,
                        sequence: 0,
                    },
                    state: CaptureState::Idle,
                },
                frame: None,
                closed: false,
            }),
            updates: broadcast::channel(CAPACITY).0,
            wake: Notify::new(),
        }
    }
    pub fn observe(&self) -> Result<Observation<CaptureState>, ControlError> {
        let state = self.state.lock().expect("capture mailbox lock");
        if state.closed {
            return Err(CLOSED);
        }
        Ok(Observation {
            initial: state.snapshot.clone(),
            updates: futures::stream::unfold(Some(self.updates.subscribe()), |receiver| async {
                let mut receiver = receiver?;
                match receiver.recv().await {
                    Ok(Some(snapshot)) => Some((Ok(Update::Snapshot(snapshot)), Some(receiver))),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        Some((Ok(Update::ResyncRequired), None))
                    }
                    Ok(None) | Err(broadcast::error::RecvError::Closed) => None,
                }
            })
            .boxed(),
        })
    }
    pub fn publish(&self, value: CaptureState) {
        let mut state = self.state.lock().expect("capture mailbox lock");
        state.snapshot.cursor.sequence = state
            .snapshot
            .cursor
            .sequence
            .checked_add(1)
            .expect("observation sequence exhausted");
        state.snapshot.state = value;
        let _ = self.updates.send(Some(state.snapshot.clone()));
    }
    pub fn offer(&self, frame: CapturedFrame) {
        self.state.lock().expect("capture mailbox lock").frame = Some(frame);
        self.wake.notify_one();
    }
    pub fn discard_pending_frame(&self) {
        self.state.lock().expect("capture mailbox lock").frame = None;
    }
    pub async fn next(&self) -> Option<CapturedFrame> {
        loop {
            {
                let mut state = self.state.lock().expect("capture mailbox lock");
                if state.closed {
                    return None;
                }
                if let Some(frame) = state.frame.take() {
                    return Some(frame);
                }
            }
            self.wake.notified().await;
        }
    }
    pub fn close(&self) {
        let mut state = self.state.lock().expect("capture mailbox lock");
        state.closed = true;
        state.frame = None;
        let _ = self.updates.send(None);
        self.wake.notify_one();
    }
}
