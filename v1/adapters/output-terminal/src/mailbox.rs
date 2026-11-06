//! Atomic admission/observation state shared with nonblocking color producers.
use crate::{CAPACITY, CLOSED, Command};
use crate::{
    diagnostics::Info,
    preview::{Preview, Sample},
};
use futures::StreamExt;
use prysm_contract::{
    ControlError,
    identity::{InstanceId, SessionId, StreamEpoch},
    observation::{Cursor, Observation, Update, Versioned},
    output::{ColorFrame, OfferResult, OutputState},
    preview::PreviewFrame,
};
use prysm_output::Admission;
use std::{
    sync::{Mutex, OnceLock},
    thread::Thread,
};
use tokio::sync::{Notify, broadcast, mpsc};

struct State {
    admission: Admission,
    snapshot: Versioned<OutputState>,
    closed: bool,
    stopping: bool,
    info: Info,
    preview: Preview,
}
pub(super) struct Mailbox {
    state: Mutex<State>,
    updates: broadcast::Sender<Option<Versioned<OutputState>>>,
    worker: OnceLock<Thread>,
    commands: mpsc::Sender<Command>,
    preview_stopped: Notify,
}
impl Mailbox {
    pub fn new(instance: InstanceId) -> (Self, mpsc::Receiver<Command>) {
        let (commands, receiver) = mpsc::channel(CAPACITY);
        let mailbox = Self {
            state: Mutex::new(State {
                admission: Admission::new(instance),
                closed: false,
                stopping: false,
                info: Info::default(),
                preview: Preview::Disabled,
                snapshot: Versioned {
                    cursor: Cursor {
                        instance,
                        sequence: 0,
                    },
                    state: Self::unknown(),
                },
            }),
            updates: broadcast::channel(CAPACITY).0,
            worker: OnceLock::new(),
            commands,
            preview_stopped: Notify::new(),
        };
        (mailbox, receiver)
    }
    pub async fn send(&self, command: Command) -> Result<(), ControlError> {
        self.authorize()?;
        self.commands.send(command).await.map_err(|_| CLOSED)?;
        self.wake();
        Ok(())
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
        state.preview = Preview::Disabled;
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
        drop(state);
        self.preview_stopped.notify_one();
        self.wake();
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
        state.preview = Preview::Disabled;
        state.admission.suspend();
        // The shared sender outlives the owner. End subscriptions explicitly.
        let _ = self.updates.send(None);
        drop(state);
        self.preview_stopped.notify_one();
        self.wake();
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
    pub fn offer(&self, frame: ColorFrame) -> Result<OfferResult, ControlError> {
        let mut state = self.state.lock().expect("output mailbox lock");
        state.accepting()?;
        let result = state.admission.offer(frame);
        drop(state);
        if result == OfferResult::Queued {
            self.wake();
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
    pub fn register_worker(&self) {
        self.worker
            .set(std::thread::current())
            .expect("one terminal worker");
    }
    fn wake(&self) {
        if let Some(worker) = self.worker.get() {
            worker.unpark();
        }
    }
    pub fn preview_waiting(&self) {
        let mut state = self.state.lock().expect("output mailbox lock");
        if !state.closed && !state.stopping {
            state.preview = Preview::Waiting;
        }
    }
    pub fn offer_preview(&self, frame: PreviewFrame) -> bool {
        let mut state = self.state.lock().expect("output mailbox lock");
        if !matches!(state.preview, Preview::Waiting | Preview::Frame(_)) {
            return false;
        }
        state.preview = Preview::Frame(std::sync::Arc::new(Sample {
            frame,
            received_at: std::time::Instant::now(),
        }));
        self.wake();
        true
    }
    pub fn disable_preview(&self, reason: String) {
        let mut state = self.state.lock().expect("output mailbox lock");
        if !state.closed && !state.stopping {
            state.preview = Preview::Unavailable(
                reason
                    .chars()
                    .take(240)
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect(),
            );
        }
        drop(state);
        self.preview_stopped.notify_one();
        self.wake();
    }
    pub async fn preview_stopped(&self) {
        self.preview_stopped.notified().await;
    }
    pub fn preview(&self) -> Preview {
        self.state
            .lock()
            .expect("output mailbox lock")
            .preview
            .clone()
    }
    pub fn diagnostics(&self, update: impl FnOnce(&mut Info)) -> bool {
        let mut state = self.state.lock().expect("output mailbox lock");
        if state.closed {
            return false;
        }
        update(&mut state.info);
        self.wake();
        true
    }
    pub fn info(&self) -> Info {
        self.state.lock().expect("output mailbox lock").info.clone()
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
