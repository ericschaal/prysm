//! Worker inputs, frame fences, and wake notifications.
use crate::{CAPACITY, CLOSED};
use prysm_contract::{
    ControlError,
    capture::CapturedFrame,
    identity::{FrameStamp, InstanceId, SessionId},
    observation::Observation,
    processing::{Configured, ProcessingState, ProcessingUpdate, Quiescent},
};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{mpsc, oneshot};

type Reply<T> = oneshot::Sender<Result<T, ControlError>>;
pub(super) enum Command {
    Configure(ProcessingUpdate, Reply<Configured>),
    Barrier(SessionId, Reply<Quiescent>),
    Observe(Reply<Observation<ProcessingState>>),
}

#[derive(Default)]
struct Inbox {
    pending: Option<CapturedFrame>,
    newest: Option<FrameStamp>,
    sealed_through: u64,
}

pub(super) struct Mailbox {
    commands: mpsc::Sender<Command>,
    inbox: Mutex<Inbox>,
    thread: OnceLock<std::thread::Thread>,
    stopping: AtomicBool,
}

fn order(stamp: FrameStamp) -> (u64, u64, u64) {
    (
        stamp.epoch.session.sequence.get(),
        stamp.epoch.generation,
        stamp.sequence,
    )
}

impl Mailbox {
    pub(super) fn new() -> (Self, mpsc::Receiver<Command>) {
        let (commands, receiver) = mpsc::channel(CAPACITY);
        (
            Self {
                commands,
                inbox: Mutex::default(),
                thread: OnceLock::new(),
                stopping: AtomicBool::new(false),
            },
            receiver,
        )
    }

    pub(super) async fn send(&self, command: Command) -> Result<(), ControlError> {
        self.commands.send(command).await.map_err(|_| CLOSED)?;
        self.wake();
        Ok(())
    }

    pub(super) fn register_worker(&self) {
        self.thread
            .set(std::thread::current())
            .expect("one worker per flow");
    }

    pub(super) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    pub(super) fn take_frame(&self) -> Option<CapturedFrame> {
        self.inbox.lock().expect("frame inbox lock").pending.take()
    }

    fn wake(&self) {
        if let Some(thread) = self.thread.get() {
            thread.unpark();
        }
    }

    pub(super) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.inbox.lock().expect("frame inbox lock").pending = None;
        self.wake();
    }

    pub(super) fn offer(&self, frame: CapturedFrame, instance: InstanceId) {
        let mut inbox = self.inbox.lock().expect("frame inbox lock");
        let stamp = frame.stamp;
        if self.stopping.load(Ordering::Acquire)
            || stamp.epoch.session.instance != instance
            || stamp.epoch.generation == 0
            || stamp.epoch.session.sequence.get() <= inbox.sealed_through
            || inbox
                .newest
                .is_some_and(|latest| order(stamp) <= order(latest))
        {
            return;
        }
        inbox.newest = Some(stamp);
        inbox.pending = Some(frame);
        drop(inbox);
        self.wake();
    }

    pub(super) fn seal(&self, session: SessionId) {
        let mut inbox = self.inbox.lock().expect("frame inbox lock");
        // Sessions increase within one application instance; no growing tombstone set.
        inbox.sealed_through = inbox.sealed_through.max(session.sequence.get());
        if inbox
            .pending
            .as_ref()
            .is_some_and(|frame| frame.stamp.epoch.session.sequence.get() <= inbox.sealed_through)
        {
            inbox.pending = None;
        }
    }
}
