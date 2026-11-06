//! Request acceptance and shutdown share one lock; accepted work drains after close.
use crate::worker::Request;
use prysm_contract::{ControlError, Failure};
use std::sync::{Arc, Mutex, mpsc};

pub(super) struct Admission<S> {
    state: Mutex<State<S>>,
}

struct State<S> {
    sender: Option<mpsc::SyncSender<Request<S>>>,
    failure: Option<Failure>,
}

impl<S> Admission<S> {
    pub fn channel() -> (Arc<Self>, mpsc::Receiver<Request<S>>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        let admission = Self {
            state: Mutex::new(State {
                sender: Some(sender),
                failure: None,
            }),
        };
        (Arc::new(admission), receiver)
    }

    pub fn send(&self, request: Request<S>) -> Result<(), ControlError> {
        let state = self.state.lock().expect("settings admission");
        if let Some(failure) = &state.failure {
            return Err(failure.clone().into());
        }
        let sender = state
            .sender
            .as_ref()
            .ok_or(ControlError::Unavailable("settings owner closed"))?;
        match sender.try_send(request) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => Err(ControlError::Conflict("settings I/O is busy")),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Err(ControlError::Unavailable("settings owner closed"))
            }
        }
    }

    pub fn close(&self) {
        self.state.lock().expect("settings admission").sender.take();
    }

    pub fn close_with_failure(&self, failure: Failure) {
        let mut state = self.state.lock().expect("settings admission");
        state.failure = Some(failure);
        state.sender.take();
    }
}
