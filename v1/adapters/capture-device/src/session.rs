//! One retained native lifetime. No device work is cancelled by dropping a waiter.
use crate::{
    Reply,
    backend::{Completion, Run},
    mailbox::Mailbox,
};
use prysm_contract::{
    ControlError, Failure, FailureKind,
    capture::{CaptureStarted, CaptureState, CaptureStopped, CapturedFrame},
    identity::{FrameStamp, SessionId, StreamEpoch},
};
use prysm_core::Frame;
use std::{num::NonZeroU32, sync::Arc, time::Duration};
use tokio::time::{Instant, sleep_until};

enum Phase {
    Starting { readiness_pending: bool },
    Waiting,
    Available { deadline: Instant },
    Stopping,
}

enum Event {
    Completed(Completion),
    Ready,
    ReadinessClosed,
    Silence,
    Frame(Option<Frame>),
    FramesClosed,
}

pub(super) struct Session {
    id: SessionId,
    run: Run,
    mailbox: Arc<Mailbox>,
    phase: Phase,
    start_reply: Option<Reply<CaptureStarted>>,
    stop_reply: Option<Reply<CaptureStopped>>,
    epoch: u64,
    sequence: u64,
    grace: Duration,
    failure: Option<Failure>,
}
impl Session {
    pub fn new(
        id: SessionId,
        rate: NonZeroU32,
        run: Run,
        mailbox: Arc<Mailbox>,
        reply: Reply<CaptureStarted>,
    ) -> Self {
        mailbox.publish(CaptureState::Starting(id));
        Self {
            id,
            run,
            mailbox,
            phase: Phase::Starting {
                readiness_pending: true,
            },
            start_reply: Some(reply),
            stop_reply: None,
            epoch: 0,
            sequence: 0,
            grace: Duration::from_secs(2).max(Duration::from_secs(3) / rate.get()),
            failure: None,
        }
    }
    pub fn id(&self) -> SessionId {
        self.id
    }
    fn epoch(&self) -> StreamEpoch {
        StreamEpoch {
            session: self.id,
            generation: self.epoch,
        }
    }
    fn advance_epoch(&mut self) {
        self.epoch = self.epoch.checked_add(1).expect("capture epoch exhausted");
    }
    pub fn stop(&mut self, reply: Option<Reply<CaptureStopped>>) {
        if matches!(self.phase, Phase::Stopping) {
            if let Some(reply) = reply {
                let _ = reply.send(Err(ControlError::CleanupPending(self.id)));
            }
            return;
        }
        self.phase = Phase::Stopping;
        self.stop_reply = reply;
        self.mailbox.discard_pending_frame();
        self.run.stop.cancel();
        self.mailbox.publish(CaptureState::Stopping(self.id));
        if let Some(reply) = self.start_reply.take() {
            let _ = reply.send(Err(Failure::new(
                FailureKind::Interrupted,
                "capture Start interrupted by Stop",
            )
            .into()));
        }
    }
    /// Cancel-safe: all native futures and partial delivery state remain in self.
    pub async fn progress(&mut self) -> Option<Completion> {
        // Apply the selected event without another await, before yielding to the owner.
        match self.next_event().await {
            Event::Completed(completion) => return Some(completion),
            Event::Ready => self.native_ready(),
            Event::ReadinessClosed => self.readiness_closed(),
            Event::Silence => self.delivery_expired(),
            Event::Frame(frame) => self.deliver_frame(frame),
            Event::FramesClosed => self.frame_source_closed(),
        }
        None
    }

    async fn next_event(&mut self) -> Event {
        let silence = async {
            match self.phase {
                Phase::Available { deadline } => sleep_until(deadline).await,
                _ => std::future::pending().await,
            }
        };
        // Completion fences all delivery; readiness precedes frames; expired silence
        // precedes queued frames so a delayed poll cannot conceal a continuity break.
        tokio::select! {
            biased;
            completion = &mut self.run.finished => Event::Completed(completion),
            ready = &mut self.run.ready,
                if matches!(self.phase, Phase::Starting { readiness_pending: true }) => {
                match ready {
                    Ok(()) => Event::Ready,
                    Err(_) => Event::ReadinessClosed,
                }
            }
            _ = silence => Event::Silence,
            changed = self.run.frames.changed(),
                if matches!(self.phase, Phase::Waiting | Phase::Available { .. }) => {
                match changed {
                    Ok(()) => Event::Frame(self.run.frames.borrow_and_update().clone()),
                    Err(_) => Event::FramesClosed,
                }
            }
        }
    }

    fn native_ready(&mut self) {
        self.phase = Phase::Waiting;
        self.mailbox.publish(CaptureState::Waiting {
            epoch: self.epoch(),
            reason: "waiting for first frame".into(),
        });
        if let Some(reply) = self.start_reply.take() {
            let _ = reply.send(Ok(CaptureStarted { session: self.id }));
        }
    }

    fn readiness_closed(&mut self) {
        // The worker's completion carries the cause and proves whether cleanup finished.
        self.phase = Phase::Starting {
            readiness_pending: false,
        };
    }

    fn delivery_expired(&mut self) {
        self.advance_epoch();
        self.phase = Phase::Waiting;
        // Waiting must not deliver a pending frame from the previous epoch.
        self.mailbox.discard_pending_frame();
        self.mailbox.publish(CaptureState::Waiting {
            epoch: self.epoch(),
            reason: "no valid frame delivered".into(),
        });
    }

    fn deliver_frame(&mut self, frame: Option<Frame>) {
        let Some(frame) = frame else { return };
        let resumed = matches!(self.phase, Phase::Waiting);
        self.phase = Phase::Available {
            deadline: Instant::now() + self.grace,
        };
        if resumed {
            self.advance_epoch();
            self.mailbox.publish(CaptureState::Available(self.epoch()));
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("capture frame sequence exhausted");
        self.mailbox.offer(CapturedFrame {
            stamp: FrameStamp {
                epoch: self.epoch(),
                sequence: self.sequence,
            },
            frame,
        });
    }

    fn frame_source_closed(&mut self) {
        self.failure = Some(Failure::new(
            FailureKind::Unavailable,
            "native frame source closed",
        ));
        self.stop(None);
    }

    /// Returns a recovery obligation only when native release could not be certified.
    pub fn completed(mut self, completion: Completion) -> Option<Failure> {
        let expected_stop = matches!(self.phase, Phase::Stopping);
        let mut failure = completion.failure.or(self.failure.take());
        if !completion.cleanup_complete {
            failure = Some(failure.unwrap_or_else(|| {
                Failure::new(
                    FailureKind::RecoveryRequired,
                    "native release is unconfirmed",
                )
            }));
        } else if !expected_stop && failure.is_none() {
            failure = Some(Failure::new(
                FailureKind::Unavailable,
                "native capture ended unexpectedly",
            ));
        }
        self.mailbox.discard_pending_frame();
        if let Some(reply) = self.start_reply.take() {
            let _ = reply.send(Err(failure
                .clone()
                .unwrap_or_else(|| {
                    Failure::new(FailureKind::Interrupted, "capture ended before readiness")
                })
                .into()));
        }
        if let Some(reply) = self.stop_reply.take() {
            let result = if completion.cleanup_complete {
                Ok(CaptureStopped { session: self.id })
            } else {
                Err(failure
                    .clone()
                    .expect("unconfirmed cleanup has a failure")
                    .into())
            };
            let _ = reply.send(result);
        }
        match &failure {
            Some(failure) => self.mailbox.publish(CaptureState::Failed {
                session: self.id,
                failure: failure.clone(),
                cleanup_complete: completion.cleanup_complete,
            }),
            None => self.mailbox.publish(CaptureState::Idle),
        }
        if completion.cleanup_complete {
            None
        } else {
            failure
        }
    }
}
